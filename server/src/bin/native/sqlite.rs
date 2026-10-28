// Runs each statement the server sends on one connection, and answers with the changes or rows.

use std::io::{BufRead, Write};

use rusqlite::{
    Connection, params_from_iter,
    types::{Value as Sql, ValueRef},
};
use serde_json::{Value, json};

pub fn serve(path: &str) {
    let connection = Connection::open(path);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return };
        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                eprintln!("gem-native sqlite: unreadable request: {error}");
                return;
            }
        };
        if request["op"] == "close" {
            return;
        }
        let reply = match &connection {
            Ok(connection) => run(connection, &request)
                .unwrap_or_else(|error| json!({"error": error.to_string()})),
            Err(error) => json!({"error": error.to_string()}),
        };
        if writeln!(stdout, "{reply}")
            .and_then(|()| stdout.flush())
            .is_err()
        {
            return;
        }
    }
}

fn run(connection: &Connection, request: &Value) -> rusqlite::Result<Value> {
    let sql = request["sql"].as_str().unwrap_or_default();
    let params: Vec<Sql> = (request["params"].as_array().into_iter().flatten())
        .map(to_sql)
        .collect();
    match request["op"].as_str() {
        Some("execute") => {
            Ok(json!({"changes": connection.execute(sql, params_from_iter(params))?}))
        }
        Some("batch") => connection
            .execute_batch(sql)
            .map(|()| json!({"changes": 0})),
        Some("query") => {
            let mut statement = connection.prepare(sql)?;
            let columns = statement.column_count();
            let mut rows = statement.query(params_from_iter(params))?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                let values = (0..columns)
                    .map(|i| row.get_ref(i).map(from_sql))
                    .collect::<rusqlite::Result<Vec<Value>>>()?;
                out.push(Value::Array(values));
            }
            Ok(json!({"rows": out}))
        }
        other => Ok(json!({"error": format!("unknown op {other:?}")})),
    }
}

fn to_sql(value: &Value) -> Sql {
    match value {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(*b as i64),
        Value::Number(n) => match n.as_i64() {
            Some(n) => Sql::Integer(n),
            None => Sql::Real(n.as_f64().unwrap_or_default()),
        },
        Value::String(s) => Sql::Text(s.clone()),
        Value::Object(o) => Sql::Blob(
            (o.get("blob")
                .and_then(Value::as_array)
                .into_iter()
                .flatten())
            .map(|b| b.as_u64().unwrap_or_default() as u8)
            .collect(),
        ),
        Value::Array(_) => Sql::Null,
    }
}

fn from_sql(value: ValueRef) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => json!(n),
        // Written with a fraction, so the client reads it back as a float.
        ValueRef::Real(n) => serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number),
        ValueRef::Text(s) => Value::String(String::from_utf8_lossy(s).into_owned()),
        ValueRef::Blob(b) => json!({"blob": b}),
    }
}
