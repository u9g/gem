// JSON without serde: a value, a reader and a writer, and the traits a type implements by hand to
// be read from and written as JSON. Std only, so lust (https://github.com/t3dotgg/lust-lang),
// which runs no proc macros, compiles everything that uses it. Objects keep their keys sorted, as
// serde_json's do without `preserve_order`, so the text written for a value is the same as before.

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    ops::{Index, IndexMut},
};

mod read;
mod write;

pub use read::{parse, parse_bytes, parse_stream};

pub type Map = BTreeMap<String, Value>;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Value>),
    Object(Map),
}

// Integers stay exact: an unsigned one is `U`, a negative one `I`, anything else `F`.
#[derive(Clone, Copy, Debug)]
pub enum Number {
    U(u64),
    I(i64),
    F(f64),
}

impl PartialEq for Number {
    fn eq(&self, other: &Number) -> bool {
        match (self.as_i128(), other.as_i128()) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.as_f64() == other.as_f64(),
            _ => false,
        }
    }
}

impl Number {
    fn as_i128(&self) -> Option<i128> {
        match *self {
            Number::U(n) => Some(n as i128),
            Number::I(n) => Some(n as i128),
            Number::F(_) => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Number::U(n) => Some(n),
            Number::I(n) => u64::try_from(n).ok(),
            Number::F(_) => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Number::U(n) => i64::try_from(n).ok(),
            Number::I(n) => Some(n),
            Number::F(_) => None,
        }
    }

    pub fn as_f64(&self) -> f64 {
        match *self {
            Number::U(n) => n as f64,
            Number::I(n) => n as f64,
            Number::F(n) => n,
        }
    }
}

// Why a text is not JSON, or not the JSON a type wants.
#[derive(Clone, Debug, PartialEq)]
pub struct Error(pub String);

impl Error {
    pub fn new(message: impl Into<String>) -> Error {
        Error(message.into())
    }

    // The same error, under the object field it was found in.
    pub fn at(self, field: &str) -> Error {
        Error(format!("{field}: {}", self.0))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

static NULL: Value = Value::Null;

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Value::String(_))
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Value::Array(_))
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Number(n) => n.as_u64(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Number(n) => n.as_i64(),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(n.as_f64()),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Map> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut Map> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object().and_then(|o| o.get(key))
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.as_object_mut().and_then(|o| o.get_mut(key))
    }

    // Leaves null in its place.
    pub fn take(&mut self) -> Value {
        std::mem::take(self)
    }

    // The field `name` of an object, read as `T`. A missing field reads as null, so it is an
    // error unless `T` accepts null, as an `Option` does.
    pub fn field<T: FromJson>(&self, name: &str) -> Result<T, Error> {
        if !self.is_object() {
            return Err(Error::new(format!("expected an object with {name}, found {}", self.kind())));
        }
        T::from_json(self.get(name).unwrap_or(&NULL)).map_err(|e| e.at(name))
    }

    // The field `name` read as `T`, or `T`'s default when it is missing or null.
    pub fn field_or_default<T: FromJson + Default>(&self, name: &str) -> Result<T, Error> {
        match self.get(name) {
            None | Some(Value::Null) => Ok(T::default()),
            Some(value) => T::from_json(value).map_err(|e| e.at(name)),
        }
    }

    // What the value is, for messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Number(_) => "a number",
            Value::String(_) => "a string",
            Value::Array(_) => "an array",
            Value::Object(_) => "an object",
        }
    }

    pub fn to_string_pretty(&self) -> String {
        let mut out = String::new();
        write::pretty(self, 0, &mut out);
        out
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut out = String::new();
        write::compact(self, &mut out);
        f.write_str(&out)
    }
}

impl Index<&str> for Value {
    type Output = Value;

    // Null for a missing field or a value that is not an object.
    fn index(&self, key: &str) -> &Value {
        self.get(key).unwrap_or(&NULL)
    }
}

impl Index<usize> for Value {
    type Output = Value;

    fn index(&self, i: usize) -> &Value {
        self.as_array().and_then(|a| a.get(i)).unwrap_or(&NULL)
    }
}

impl IndexMut<&str> for Value {
    // Makes null an object, and adds the field as null when it is missing.
    fn index_mut(&mut self, key: &str) -> &mut Value {
        if self.is_null() {
            *self = Value::Object(Map::new());
        }
        match self {
            Value::Object(o) => o.entry(key.to_string()).or_insert(Value::Null),
            other => panic!("cannot index {} with {key:?}", other.kind()),
        }
    }
}

impl PartialEq<str> for Value {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<&str> for Value {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

impl PartialEq<String> for Value {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == Some(other.as_str())
    }
}

impl PartialEq<bool> for Value {
    fn eq(&self, other: &bool) -> bool {
        self.as_bool() == Some(*other)
    }
}

impl PartialEq<i64> for Value {
    fn eq(&self, other: &i64) -> bool {
        self.as_i64() == Some(*other)
    }
}

impl PartialEq<u64> for Value {
    fn eq(&self, other: &u64) -> bool {
        self.as_u64() == Some(*other)
    }
}

// A type written as JSON.
pub trait ToJson {
    fn to_json(&self) -> Value;
}

// A type read from JSON.
pub trait FromJson: Sized {
    fn from_json(value: &Value) -> Result<Self, Error>;
}

// `value` as compact JSON text.
pub fn to_string<T: ToJson + ?Sized>(value: &T) -> String {
    value.to_json().to_string()
}

pub fn to_string_pretty<T: ToJson + ?Sized>(value: &T) -> String {
    value.to_json().to_string_pretty()
}

// `text` read as a `T`.
pub fn from_str<T: FromJson>(text: &str) -> Result<T, Error> {
    T::from_json(&parse(text)?)
}

pub fn from_slice<T: FromJson>(bytes: &[u8]) -> Result<T, Error> {
    T::from_json(&parse_bytes(bytes)?)
}

// Each of the JSON values `bytes` holds one after another, as jq prints them, read as a `T`.
pub fn from_stream<T: FromJson>(bytes: &[u8]) -> Result<Vec<T>, Error> {
    let text = std::str::from_utf8(bytes).map_err(|e| Error::new(e.to_string()))?;
    let mut out = Vec::new();
    for value in parse_stream(text)? {
        out.push(T::from_json(&value)?);
    }
    Ok(out)
}

impl ToJson for Value {
    fn to_json(&self) -> Value {
        self.clone()
    }
}

impl FromJson for Value {
    fn from_json(value: &Value) -> Result<Value, Error> {
        Ok(value.clone())
    }
}

impl ToJson for str {
    fn to_json(&self) -> Value {
        Value::String(self.to_string())
    }
}

impl ToJson for String {
    fn to_json(&self) -> Value {
        Value::String(self.clone())
    }
}

impl FromJson for String {
    fn from_json(value: &Value) -> Result<String, Error> {
        match value {
            Value::String(s) => Ok(s.clone()),
            other => Err(Error::new(format!("expected a string, found {}", other.kind()))),
        }
    }
}

impl ToJson for bool {
    fn to_json(&self) -> Value {
        Value::Bool(*self)
    }
}

impl FromJson for bool {
    fn from_json(value: &Value) -> Result<bool, Error> {
        match value {
            Value::Bool(b) => Ok(*b),
            other => Err(Error::new(format!("expected a boolean, found {}", other.kind()))),
        }
    }
}

macro_rules! unsigned {
    ($($t:ty),*) => {$(
        impl ToJson for $t {
            fn to_json(&self) -> Value {
                Value::Number(Number::U(*self as u64))
            }
        }

        impl FromJson for $t {
            fn from_json(value: &Value) -> Result<$t, Error> {
                (value.as_u64())
                    .and_then(|n| <$t>::try_from(n).ok())
                    .ok_or_else(|| Error::new(format!("expected an unsigned integer, found {value}")))
            }
        }
    )*};
}

macro_rules! signed {
    ($($t:ty),*) => {$(
        impl ToJson for $t {
            fn to_json(&self) -> Value {
                let n = *self as i64;
                Value::Number(if n < 0 { Number::I(n) } else { Number::U(n as u64) })
            }
        }

        impl FromJson for $t {
            fn from_json(value: &Value) -> Result<$t, Error> {
                (value.as_i64())
                    .and_then(|n| <$t>::try_from(n).ok())
                    .ok_or_else(|| Error::new(format!("expected an integer, found {value}")))
            }
        }
    )*};
}

unsigned!(u8, u16, u32, u64, usize);
signed!(i8, i16, i32, i64, isize);

impl ToJson for f64 {
    fn to_json(&self) -> Value {
        Value::Number(Number::F(*self))
    }
}

impl FromJson for f64 {
    fn from_json(value: &Value) -> Result<f64, Error> {
        value.as_f64().ok_or_else(|| Error::new(format!("expected a number, found {}", value.kind())))
    }
}

impl<T: ToJson> ToJson for Option<T> {
    fn to_json(&self) -> Value {
        match self {
            Some(value) => value.to_json(),
            None => Value::Null,
        }
    }
}

impl<T: FromJson> FromJson for Option<T> {
    fn from_json(value: &Value) -> Result<Option<T>, Error> {
        match value {
            Value::Null => Ok(None),
            value => T::from_json(value).map(Some),
        }
    }
}

impl<T: ToJson> ToJson for Vec<T> {
    fn to_json(&self) -> Value {
        Value::Array(self.iter().map(|v| v.to_json()).collect())
    }
}

impl<T: ToJson> ToJson for [T] {
    fn to_json(&self) -> Value {
        Value::Array(self.iter().map(|v| v.to_json()).collect())
    }
}

impl<T: FromJson> FromJson for Vec<T> {
    fn from_json(value: &Value) -> Result<Vec<T>, Error> {
        match value {
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for (i, item) in items.iter().enumerate() {
                    out.push(T::from_json(item).map_err(|e| e.at(&i.to_string()))?);
                }
                Ok(out)
            }
            other => Err(Error::new(format!("expected an array, found {}", other.kind()))),
        }
    }
}

impl<T: ToJson> ToJson for BTreeMap<String, T> {
    fn to_json(&self) -> Value {
        Value::Object(self.iter().map(|(k, v)| (k.clone(), v.to_json())).collect())
    }
}

impl<T: FromJson> FromJson for BTreeMap<String, T> {
    fn from_json(value: &Value) -> Result<BTreeMap<String, T>, Error> {
        let object = value
            .as_object()
            .ok_or_else(|| Error::new(format!("expected an object, found {}", value.kind())))?;
        let mut out = BTreeMap::new();
        for (k, v) in object {
            out.insert(k.clone(), T::from_json(v).map_err(|e| e.at(k))?);
        }
        Ok(out)
    }
}

impl<T: ToJson> ToJson for HashMap<String, T> {
    fn to_json(&self) -> Value {
        Value::Object(self.iter().map(|(k, v)| (k.clone(), v.to_json())).collect())
    }
}

impl<T: FromJson> FromJson for HashMap<String, T> {
    fn from_json(value: &Value) -> Result<HashMap<String, T>, Error> {
        let object = value
            .as_object()
            .ok_or_else(|| Error::new(format!("expected an object, found {}", value.kind())))?;
        let mut out = HashMap::new();
        for (k, v) in object {
            out.insert(k.clone(), T::from_json(v).map_err(|e| e.at(k))?);
        }
        Ok(out)
    }
}

impl<T: ToJson + ?Sized> ToJson for &T {
    fn to_json(&self) -> Value {
        (**self).to_json()
    }
}

impl<T: ToJson + ?Sized> ToJson for Box<T> {
    fn to_json(&self) -> Value {
        (**self).to_json()
    }
}

impl<A: ToJson, B: ToJson> ToJson for (A, B) {
    fn to_json(&self) -> Value {
        Value::Array(vec![self.0.to_json(), self.1.to_json()])
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::String(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Value {
        Value::String(s)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}

impl From<Map> for Value {
    fn from(o: Map) -> Value {
        Value::Object(o)
    }
}

impl From<Vec<Value>> for Value {
    fn from(a: Vec<Value>) -> Value {
        Value::Array(a)
    }
}

// A literal in `json!`. Literals skip ToJson, whose impl for references leaves lust unable to type
// an integer literal.
pub trait Literal {
    fn literal(self) -> Value;
}

impl Literal for &str {
    fn literal(self) -> Value {
        Value::String(self.to_string())
    }
}

impl Literal for bool {
    fn literal(self) -> Value {
        Value::Bool(self)
    }
}

macro_rules! literal {
    ($($t:ty),*) => {$(
        impl Literal for $t {
            fn literal(self) -> Value {
                self.to_json()
            }
        }
    )*};
}

literal!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, f64);

// Builds a Value from JSON-like syntax, as serde_json's `json!` does: `json!({"a": [1, x], "b":
// null})`. A value that is not a literal is any expression of a type that implements `ToJson`,
// written in parentheses when it is more than one token tree, e.g. `json!({"n": (a + b)})`. Keys
// are string literals, or an expression in brackets: `json!({[key]: 1})`.
#[macro_export]
macro_rules! json {
    (null) => { $crate::Value::Null };
    ([]) => { $crate::Value::Array(::std::vec::Vec::new()) };
    ([ $($tt:tt)+ ]) => {{
        let mut array: ::std::vec::Vec<$crate::Value> = ::std::vec::Vec::new();
        $crate::json_array!(array; $($tt)+);
        $crate::Value::Array(array)
    }};
    ({}) => { $crate::Value::Object($crate::Map::new()) };
    ({ $($tt:tt)+ }) => {{
        let mut object = $crate::Map::new();
        $crate::json_object!(object; $($tt)+);
        $crate::Value::Object(object)
    }};
    ($literal:literal) => { $crate::Literal::literal($literal) };
    ($other:expr) => { $crate::ToJson::to_json(&$other) };
}

#[doc(hidden)]
#[macro_export]
macro_rules! json_array {
    ($array:ident;) => {};
    ($array:ident; null $(, $($rest:tt)*)?) => {
        $array.push($crate::Value::Null);
        $crate::json_array!($array; $($($rest)*)?);
    };
    ($array:ident; [ $($inner:tt)* ] $(, $($rest:tt)*)?) => {
        $array.push($crate::json!([ $($inner)* ]));
        $crate::json_array!($array; $($($rest)*)?);
    };
    ($array:ident; { $($inner:tt)* } $(, $($rest:tt)*)?) => {
        $array.push($crate::json!({ $($inner)* }));
        $crate::json_array!($array; $($($rest)*)?);
    };
    ($array:ident; $value:literal $(, $($rest:tt)*)?) => {
        $array.push($crate::Literal::literal($value));
        $crate::json_array!($array; $($($rest)*)?);
    };
    ($array:ident; $value:expr $(, $($rest:tt)*)?) => {
        $array.push($crate::ToJson::to_json(&$value));
        $crate::json_array!($array; $($($rest)*)?);
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! json_object {
    ($object:ident;) => {};
    ($object:ident; [$key:expr] : $($rest:tt)*) => {
        $crate::json_value!($object; ::std::string::ToString::to_string(&$key); $($rest)*);
    };
    ($object:ident; $key:literal : $($rest:tt)*) => {
        $crate::json_value!($object; ::std::string::ToString::to_string($key); $($rest)*);
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! json_value {
    ($object:ident; $key:expr; null $(, $($rest:tt)*)?) => {
        $object.insert($key, $crate::Value::Null);
        $crate::json_object!($object; $($($rest)*)?);
    };
    ($object:ident; $key:expr; [ $($inner:tt)* ] $(, $($rest:tt)*)?) => {
        $object.insert($key, $crate::json!([ $($inner)* ]));
        $crate::json_object!($object; $($($rest)*)?);
    };
    ($object:ident; $key:expr; { $($inner:tt)* } $(, $($rest:tt)*)?) => {
        $object.insert($key, $crate::json!({ $($inner)* }));
        $crate::json_object!($object; $($($rest)*)?);
    };
    ($object:ident; $key:expr; $value:literal $(, $($rest:tt)*)?) => {
        $object.insert($key, $crate::Literal::literal($value));
        $crate::json_object!($object; $($($rest)*)?);
    };
    ($object:ident; $key:expr; $value:expr $(, $($rest:tt)*)?) => {
        $object.insert($key, $crate::ToJson::to_json(&$value));
        $crate::json_object!($object; $($($rest)*)?);
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let text = r#"{"a":[1,-2,3.5,true,false,null],"b":{"c":"d\n\"é\u0001"},"e":18446744073709551615}"#;
        let value = parse(text).unwrap();
        assert_eq!(value.to_string(), text.replace("\\u0001", "\\u0001"));
        assert_eq!(value["a"][1], -2i64);
        assert_eq!(value["b"]["c"], "d\n\"é\u{1}");
        assert_eq!(value["e"].as_u64(), Some(u64::MAX));
        assert!(value["missing"]["deeper"].is_null());
    }

    #[test]
    fn reads_surrogate_pairs() {
        assert_eq!(parse(r#""\ud83d\ude00""#).unwrap(), "😀");
    }

    #[test]
    fn reads_a_stream() {
        let values = parse_stream("{\"a\":1}\n{\"a\":2} 3").unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(values[1]["a"], 2u64);
    }

    #[test]
    fn rejects_bad_json() {
        assert!(parse("{").is_err());
        assert!(parse("[1,]").is_err());
        assert!(parse("1 2").is_err());
        assert!(parse("\"\\x\"").is_err());
    }

    #[test]
    fn builds_with_json() {
        let x = 3u32;
        let name = String::from("n");
        let value = json!({
            "a": [1, x, null, {"b": true}],
            "s": "text",
            [name]: (x + 1),
            "o": Some("y"),
        });
        assert_eq!(value.to_string(), r#"{"a":[1,3,null,{"b":true}],"n":4,"o":"y","s":"text"}"#);
        assert_eq!(json!([]).to_string(), "[]");
        assert_eq!(json!({}).to_string(), "{}");
    }

    #[test]
    fn writes_pretty() {
        let value = json!({"a": [1, 2], "b": {}});
        assert_eq!(value.to_string_pretty(), "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {}\n}");
    }

    #[test]
    fn writes_floats_as_serde_json_does() {
        assert_eq!(json!(1.0f64).to_string(), "1.0");
        assert_eq!(json!(0.5f64).to_string(), "0.5");
        assert_eq!(json!(f64::NAN).to_string(), "null");
    }

    #[test]
    fn indexes_mutably() {
        let mut value = Value::Null;
        value["a"]["b"] = json!(1);
        assert_eq!(value.to_string(), r#"{"a":{"b":1}}"#);
    }

    #[test]
    fn reads_fields() {
        let value = json!({"a": "x", "n": null});
        let a: String = value.field("a").unwrap();
        let n: Option<u32> = value.field("n").unwrap();
        let missing: Option<String> = value.field("missing").unwrap();
        let defaulted: u32 = value.field_or_default("missing").unwrap();
        assert_eq!((a.as_str(), n, missing, defaulted), ("x", None, None, 0));
        assert!(value.field::<String>("missing").is_err());
    }
}
