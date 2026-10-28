// Writes a Value as JSON text, as serde_json does.

use crate::{Number, Value};

pub fn compact(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => number(*n, out),
        Value::String(s) => string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                compact(item, out);
            }
            out.push(']');
        }
        Value::Object(object) => {
            out.push('{');
            for (i, (key, item)) in object.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string(key, out);
                out.push(':');
                compact(item, out);
            }
            out.push('}');
        }
    }
}

// Two spaces an indent, as serde_json's `to_string_pretty`.
pub fn pretty(value: &Value, indent: usize, out: &mut String) {
    let newline = |out: &mut String, indent: usize| {
        out.push('\n');
        for _ in 0..indent {
            out.push_str("  ");
        }
    };
    match value {
        Value::Array(items) if !items.is_empty() => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, indent + 1);
                pretty(item, indent + 1, out);
            }
            newline(out, indent);
            out.push(']');
        }
        Value::Object(object) if !object.is_empty() => {
            out.push('{');
            for (i, (key, item)) in object.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, indent + 1);
                string(key, out);
                out.push_str(": ");
                pretty(item, indent + 1, out);
            }
            newline(out, indent);
            out.push('}');
        }
        other => compact(other, out),
    }
}

fn number(n: Number, out: &mut String) {
    match n {
        Number::U(n) => out.push_str(&n.to_string()),
        Number::I(n) => out.push_str(&n.to_string()),
        // serde_json writes non-finite floats as null, and a whole float with `.0`.
        Number::F(f) if !f.is_finite() => out.push_str("null"),
        Number::F(f) => {
            let text = f.to_string();
            out.push_str(&text);
            if !text.contains(['.', 'e', 'E']) {
                out.push_str(".0");
            }
        }
    }
}

pub fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let n = c as u32;
                out.push_str("\\u00");
                out.push(HEX[(n >> 4) as usize] as char);
                out.push(HEX[(n & 0xf) as usize] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
