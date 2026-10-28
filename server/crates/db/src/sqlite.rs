// A SQLite connection held by a `gem-native sqlite` process, with the part of rusqlite's API the
// server uses. Each statement is one line of JSON to the process and one back:
//
//     {"op":"execute"|"batch"|"query","sql":"…","params":[…]}
//     {"changes":1} | {"rows":[[…],…]} | {"error":"…"}
//
// A value is JSON null, an integer, a float, a string, or {"blob":[bytes]}.

use std::{
    fmt,
    io::{BufRead, BufReader, Write},
    ops::Deref,
    process::{Child, ChildStdin, ChildStdout, Stdio},
    sync::Mutex,
};

use gem_json::{Map, Number, Value, json};

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    QueryReturnedNoRows,
    // SQLite refused the statement.
    Sql(String),
    // The helper process failed.
    Io(String),
    // A column held a value of another type than was asked for.
    Type(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::QueryReturnedNoRows => f.write_str("Query returned no rows"),
            Error::Sql(message) | Error::Io(message) | Error::Type(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

// rusqlite's: a query that finds no row is None rather than an error.
pub trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl SqlValue {
    fn to_json(&self) -> Value {
        match self {
            SqlValue::Null => Value::Null,
            SqlValue::Integer(n) => json!(*n),
            SqlValue::Real(n) => Value::Number(Number::F(*n)),
            SqlValue::Text(s) => Value::String(s.clone()),
            SqlValue::Blob(bytes) => {
                let mut blob = Map::new();
                blob.insert(
                    "blob".to_string(),
                    Value::Array(bytes.iter().map(|b| json!(*b)).collect()),
                );
                Value::Object(blob)
            }
        }
    }

    fn from_json(value: &Value) -> Result<SqlValue> {
        Ok(match value {
            Value::Null => SqlValue::Null,
            Value::Number(Number::F(n)) => SqlValue::Real(*n),
            Value::Number(n) => SqlValue::Integer(
                n.as_i64()
                    .ok_or_else(|| Error::Io(format!("integer {value} out of range")))?,
            ),
            Value::String(s) => SqlValue::Text(s.clone()),
            Value::Object(o) if o.contains_key("blob") => {
                let bytes = (value["blob"].as_array().into_iter().flatten())
                    .map(|b| {
                        b.as_u64()
                            .map(|b| b as u8)
                            .ok_or_else(|| Error::Io("bad blob".to_string()))
                    })
                    .collect::<Result<Vec<u8>>>()?;
                SqlValue::Blob(bytes)
            }
            other => return Err(Error::Io(format!("gem-native sent {other} as a value"))),
        })
    }

    fn kind(&self) -> &'static str {
        match self {
            SqlValue::Null => "NULL",
            SqlValue::Integer(_) => "INTEGER",
            SqlValue::Real(_) => "REAL",
            SqlValue::Text(_) => "TEXT",
            SqlValue::Blob(_) => "BLOB",
        }
    }
}

// A value bound to a statement's parameter.
pub trait ToSql {
    fn to_sql(&self) -> SqlValue;
}

impl ToSql for SqlValue {
    fn to_sql(&self) -> SqlValue {
        self.clone()
    }
}

impl ToSql for str {
    fn to_sql(&self) -> SqlValue {
        SqlValue::Text(self.to_string())
    }
}

impl ToSql for String {
    fn to_sql(&self) -> SqlValue {
        SqlValue::Text(self.clone())
    }
}

impl ToSql for bool {
    fn to_sql(&self) -> SqlValue {
        SqlValue::Integer(*self as i64)
    }
}

impl ToSql for f64 {
    fn to_sql(&self) -> SqlValue {
        SqlValue::Real(*self)
    }
}

impl ToSql for Vec<u8> {
    fn to_sql(&self) -> SqlValue {
        SqlValue::Blob(self.clone())
    }
}

macro_rules! integer_to_sql {
    ($($t:ty),*) => {$(
        impl ToSql for $t {
            fn to_sql(&self) -> SqlValue {
                SqlValue::Integer(*self as i64)
            }
        }
    )*};
}

integer_to_sql!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl<T: ToSql> ToSql for Option<T> {
    fn to_sql(&self) -> SqlValue {
        match self {
            Some(value) => value.to_sql(),
            None => SqlValue::Null,
        }
    }
}

impl<T: ToSql + ?Sized> ToSql for &T {
    fn to_sql(&self) -> SqlValue {
        (**self).to_sql()
    }
}

// A column's value read as a Rust type, as rusqlite converts it.
pub trait FromSql: Sized {
    fn from_sql(value: &SqlValue) -> Result<Self>;
}

fn mismatch<T>(value: &SqlValue, wanted: &str) -> Result<T> {
    Err(Error::Type(format!(
        "Invalid column type {} for {wanted}",
        value.kind()
    )))
}

impl FromSql for SqlValue {
    fn from_sql(value: &SqlValue) -> Result<SqlValue> {
        Ok(value.clone())
    }
}

impl FromSql for String {
    fn from_sql(value: &SqlValue) -> Result<String> {
        match value {
            SqlValue::Text(s) => Ok(s.clone()),
            other => mismatch(other, "String"),
        }
    }
}

impl FromSql for bool {
    fn from_sql(value: &SqlValue) -> Result<bool> {
        match value {
            SqlValue::Integer(n) => Ok(*n != 0),
            other => mismatch(other, "bool"),
        }
    }
}

impl FromSql for f64 {
    fn from_sql(value: &SqlValue) -> Result<f64> {
        match value {
            SqlValue::Integer(n) => Ok(*n as f64),
            SqlValue::Real(n) => Ok(*n),
            other => mismatch(other, "f64"),
        }
    }
}

impl FromSql for Vec<u8> {
    fn from_sql(value: &SqlValue) -> Result<Vec<u8>> {
        match value {
            SqlValue::Blob(b) => Ok(b.clone()),
            SqlValue::Text(s) => Ok(s.as_bytes().to_vec()),
            other => mismatch(other, "Vec<u8>"),
        }
    }
}

macro_rules! integer_from_sql {
    ($($t:ty),*) => {$(
        impl FromSql for $t {
            fn from_sql(value: &SqlValue) -> Result<$t> {
                match value {
                    SqlValue::Integer(n) => <$t>::try_from(*n)
                        .map_err(|_| Error::Type(format!("integer {n} out of range for {}", stringify!($t)))),
                    other => mismatch(other, stringify!($t)),
                }
            }
        }
    )*};
}

integer_from_sql!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl<T: FromSql> FromSql for Option<T> {
    fn from_sql(value: &SqlValue) -> Result<Option<T>> {
        match value {
            SqlValue::Null => Ok(None),
            other => T::from_sql(other).map(Some),
        }
    }
}

// The parameters of a statement, `?1` onwards: `params![a, b]`, or `params![]` for none.
#[macro_export]
macro_rules! params {
    () => { ::std::vec::Vec::<$crate::SqlValue>::new() };
    ($($param:expr),+ $(,)?) => { ::std::vec![$($crate::ToSql::to_sql(&$param)),+] };
}

pub struct Row {
    values: Vec<SqlValue>,
}

impl Row {
    // The value of the column at `index`, from 0.
    pub fn get<T: FromSql>(&self, index: usize) -> Result<T> {
        let value = (self.values.get(index))
            .ok_or_else(|| Error::Type(format!("Invalid column index: {index}")))?;
        T::from_sql(value)
    }
}

struct Io {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

pub struct Connection {
    io: Mutex<Io>,
}

impl Connection {
    // Opens the database file at `path`, or ":memory:" for one that lives as long as this.
    pub fn open(path: &str) -> Result<Connection> {
        let mut child = gem_native::command(&["sqlite", path])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Io(format!("cannot run {}: {e}", gem_native::path().display())))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let connection = Connection {
            io: Mutex::new(Io {
                child,
                stdin,
                stdout,
            }),
        };
        // gem-native answers once the file is open, or says why it is not.
        connection.request("batch", "", Vec::new())?;
        Ok(connection)
    }

    pub fn open_in_memory() -> Result<Connection> {
        Connection::open(":memory:")
    }

    fn request(&self, op: &str, sql: &str, params: Vec<SqlValue>) -> Result<Value> {
        let request = json!({
            "op": op,
            "sql": sql,
            "params": (Value::Array(params.iter().map(SqlValue::to_json).collect())),
        });
        let mut io = self
            .io
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let gone = |e: &dyn fmt::Display| Error::Io(format!("gem-native sqlite: {e}"));
        writeln!(io.stdin, "{request}").map_err(|e| gone(&e))?;
        io.stdin.flush().map_err(|e| gone(&e))?;
        let mut line = String::new();
        if io.stdout.read_line(&mut line).map_err(|e| gone(&e))? == 0 {
            return Err(gone(&"exited"));
        }
        let reply = gem_json::parse(&line).map_err(|e| gone(&e))?;
        match reply["error"].as_str() {
            Some(error) => Err(Error::Sql(error.to_string())),
            None => Ok(reply),
        }
    }

    pub fn execute(&self, sql: &str, params: Vec<SqlValue>) -> Result<usize> {
        let reply = self.request("execute", sql, params)?;
        Ok(reply["changes"].as_u64().unwrap_or_default() as usize)
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.request("batch", sql, Vec::new()).map(|_| ())
    }

    fn rows(&self, sql: &str, params: Vec<SqlValue>) -> Result<Vec<Row>> {
        let reply = self.request("query", sql, params)?;
        let mut rows = Vec::new();
        for row in reply["rows"].as_array().into_iter().flatten() {
            let values = (row.as_array().into_iter().flatten())
                .map(SqlValue::from_json)
                .collect::<Result<Vec<SqlValue>>>()?;
            rows.push(Row { values });
        }
        Ok(rows)
    }

    // The first row `sql` finds, read by `read`, or QueryReturnedNoRows.
    pub fn query_row<T, F>(&self, sql: &str, params: Vec<SqlValue>, read: F) -> Result<T>
    where
        F: FnOnce(&Row) -> Result<T>,
    {
        let rows = self.rows(sql, params)?;
        let row = rows.first().ok_or(Error::QueryReturnedNoRows)?;
        read(row)
    }

    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        Ok(Statement {
            connection: self,
            sql: sql.to_string(),
        })
    }

    // Begins a transaction, which rolls back unless committed.
    pub fn unchecked_transaction(&self) -> Result<Transaction<'_>> {
        self.execute_batch("BEGIN")?;
        Ok(Transaction {
            connection: self,
            done: false,
        })
    }

    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        self.unchecked_transaction()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let io = self
            .io
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // gem-native exits when its stdin closes; it is waited for so the file is closed after.
        let _ = writeln!(io.stdin, "{}", json!({"op": "close"}));
        let _ = io.child.wait();
    }
}

pub struct Statement<'a> {
    connection: &'a Connection,
    sql: String,
}

impl Statement<'_> {
    pub fn execute(&mut self, params: Vec<SqlValue>) -> Result<usize> {
        self.connection.execute(&self.sql, params)
    }

    pub fn query_row<T, F>(&mut self, params: Vec<SqlValue>, read: F) -> Result<T>
    where
        F: FnOnce(&Row) -> Result<T>,
    {
        self.connection.query_row(&self.sql, params, read)
    }

    // Every row, each read by `read`. The rows are all read before the first is returned.
    pub fn query_map<T, F>(
        &mut self,
        params: Vec<SqlValue>,
        read: F,
    ) -> Result<std::vec::IntoIter<Result<T>>>
    where
        F: Fn(&Row) -> Result<T>,
    {
        let rows = self.connection.rows(&self.sql, params)?;
        let read: Vec<Result<T>> = rows.iter().map(read).collect();
        Ok(read.into_iter())
    }
}

pub struct Transaction<'a> {
    connection: &'a Connection,
    done: bool,
}

impl Transaction<'_> {
    pub fn commit(mut self) -> Result<()> {
        self.done = true;
        self.connection.execute_batch("COMMIT")
    }

    pub fn rollback(mut self) -> Result<()> {
        self.done = true;
        self.connection.execute_batch("ROLLBACK")
    }
}

impl Deref for Transaction<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.connection
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.connection.execute_batch("ROLLBACK");
        }
    }
}
