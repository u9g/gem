// Reads JSON text (RFC 8259) into a Value.

use crate::{Error, Map, Number, Value};

// Deeper nesting is rejected rather than recursed into, as serde_json does.
const MAX_DEPTH: usize = 128;

pub fn parse(text: &str) -> Result<Value, Error> {
    let mut reader = Reader { bytes: text.as_bytes(), at: 0 };
    let value = reader.value(0)?;
    reader.space();
    if reader.at != reader.bytes.len() {
        return Err(reader.error("trailing characters"));
    }
    Ok(value)
}

pub fn parse_bytes(bytes: &[u8]) -> Result<Value, Error> {
    let text = std::str::from_utf8(bytes).map_err(|e| Error::new(e.to_string()))?;
    parse(text)
}

// Every value in `text`, one after another, separated by whitespace or nothing.
pub fn parse_stream(text: &str) -> Result<Vec<Value>, Error> {
    let mut reader = Reader { bytes: text.as_bytes(), at: 0 };
    let mut values = Vec::new();
    loop {
        reader.space();
        if reader.at == reader.bytes.len() {
            return Ok(values);
        }
        values.push(reader.value(0)?);
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn error(&self, what: &str) -> Error {
        let line = 1 + self.bytes[..self.at.min(self.bytes.len())].iter().filter(|b| **b == b'\n').count();
        Error::new(format!("{what} at line {line}, byte {}", self.at))
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn space(&mut self) {
        while let Some(b' ' | b'\n' | b'\r' | b'\t') = self.peek() {
            self.at += 1;
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, Error> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("expected a value"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH {
            return Err(self.error("recursion limit exceeded"));
        }
        self.space();
        match self.peek() {
            None => Err(self.error("EOF while parsing a value")),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.peek() == Some(b']') {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.space();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Value::Array(items));
                        }
                        _ => return Err(self.error("expected `,` or `]`")),
                    }
                }
            }
            Some(b'{') => {
                self.at += 1;
                let mut object = Map::new();
                self.space();
                if self.peek() == Some(b'}') {
                    self.at += 1;
                    return Ok(Value::Object(object));
                }
                loop {
                    self.space();
                    if self.peek() != Some(b'"') {
                        return Err(self.error("expected a key"));
                    }
                    let key = self.string()?;
                    self.space();
                    if self.peek() != Some(b':') {
                        return Err(self.error("expected `:`"));
                    }
                    self.at += 1;
                    let value = self.value(depth + 1)?;
                    object.insert(key, value);
                    self.space();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Value::Object(object));
                        }
                        _ => return Err(self.error("expected `,` or `}`")),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("expected a value")),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while let Some(b'0'..=b'9') = self.peek() {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.at;
        let negative = self.peek() == Some(b'-');
        if negative {
            self.at += 1;
        }
        let int_start = self.at;
        if self.digits() == 0 {
            return Err(self.error("invalid number"));
        }
        if self.bytes[int_start] == b'0' && self.at - int_start > 1 {
            return Err(self.error("invalid number"));
        }
        let mut float = false;
        if self.peek() == Some(b'.') {
            self.at += 1;
            float = true;
            if self.digits() == 0 {
                return Err(self.error("invalid number"));
            }
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.at += 1;
            float = true;
            if let Some(b'+' | b'-') = self.peek() {
                self.at += 1;
            }
            if self.digits() == 0 {
                return Err(self.error("invalid number"));
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).unwrap();
        if !float {
            if negative {
                if let Ok(n) = text.parse::<i64>() {
                    return Ok(Value::Number(if n == 0 { Number::U(0) } else { Number::I(n) }));
                }
            } else if let Ok(n) = text.parse::<u64>() {
                return Ok(Value::Number(Number::U(n)));
            }
        }
        text.parse::<f64>()
            .map(|n| Value::Number(Number::F(n)))
            .map_err(|_| self.error("invalid number"))
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        let Some(digits) = self.bytes.get(self.at..self.at + 4) else {
            return Err(self.error("EOF while parsing a string"));
        };
        let mut n = 0;
        for &d in digits {
            let v = match d {
                b'0'..=b'9' => d - b'0',
                b'a'..=b'f' => d - b'a' + 10,
                b'A'..=b'F' => d - b'A' + 10,
                _ => return Err(self.error("invalid escape")),
            };
            n = n * 16 + v as u32;
        }
        self.at += 4;
        Ok(n)
    }

    fn string(&mut self) -> Result<String, Error> {
        self.at += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(b) = self.peek() else {
                return Err(self.error("EOF while parsing a string"));
            };
            self.at += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(e) = self.peek() else {
                        return Err(self.error("EOF while parsing a string"));
                    };
                    self.at += 1;
                    let c = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let high = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&high) {
                                if !self.bytes[self.at..].starts_with(b"\\u") {
                                    return Err(self.error("lone leading surrogate"));
                                }
                                self.at += 2;
                                let low = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&low) {
                                    return Err(self.error("invalid surrogate pair"));
                                }
                                0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
                            } else {
                                high
                            };
                            match char::from_u32(code) {
                                Some(c) => c,
                                None => return Err(self.error("invalid unicode code point")),
                            }
                        }
                        _ => return Err(self.error("invalid escape")),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                0..=0x1f => return Err(self.error("control character in a string")),
                _ => out.push(b),
            }
        }
        // The input was a str, and escapes write whole chars, so the bytes are UTF-8.
        String::from_utf8(out).map_err(|e| Error::new(e.to_string()))
    }
}
