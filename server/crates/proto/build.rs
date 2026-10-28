// Generates the Rust for protocol/gem.proto: a struct per message and an enum per enum, as prost
// shapes them, with their proto3 JSON as pbjson writes and reads it. Reads only the part of proto3
// gem.proto uses: messages and enums at the top level, scalar, message and enum fields, `repeated`,
// and `reserved`. Anything else stops the build, so the generated code never quietly misses one.

use std::{env, fmt::Write, fs, path::PathBuf};

const PROTO: &str = "../../../protocol/gem.proto";

struct Field {
    name: String,
    ty: String,
    repeated: bool,
}

enum Item {
    Message { name: String, fields: Vec<Field> },
    Enum { name: String, values: Vec<(String, i32)> },
}

fn main() {
    // Outside this crate, so Cargo would not otherwise rebuild when it changes.
    println!("cargo:rerun-if-changed={PROTO}");
    let text = fs::read_to_string(PROTO).unwrap();
    let items = parse(&tokens(&text));
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("gem.v1.rs");
    fs::write(out, generate(&items)).unwrap();
}

// Words and punctuation, without comments.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c.is_whitespace() {
            i += 1;
        } else if c == '"' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += 1;
            }
            i += 1;
            out.push(chars[start..i].iter().collect());
        } else if c.is_alphanumeric() || c == '_' || c == '.' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.') {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
        } else {
            out.push(c.to_string());
            i += 1;
        }
    }
    out
}

fn parse(tokens: &[String]) -> Vec<Item> {
    let mut items = Vec::new();
    let mut i = 0;
    let skip_statement = |i: &mut usize| {
        while tokens[*i] != ";" {
            *i += 1;
        }
        *i += 1;
    };
    while i < tokens.len() {
        match tokens[i].as_str() {
            "syntax" | "package" | "import" | "option" => skip_statement(&mut i),
            "message" => {
                let name = tokens[i + 1].clone();
                assert_eq!(tokens[i + 2], "{", "message {name}");
                i += 3;
                let mut fields = Vec::new();
                while tokens[i] != "}" {
                    match tokens[i].as_str() {
                        "reserved" | "option" => skip_statement(&mut i),
                        "message" | "enum" | "oneof" | "map" | "optional" => {
                            panic!("{name}: build.rs does not generate `{}` yet", tokens[i])
                        }
                        _ => {
                            let repeated = tokens[i] == "repeated";
                            if repeated {
                                i += 1;
                            }
                            let (ty, field) = (tokens[i].clone(), tokens[i + 1].clone());
                            assert_eq!(tokens[i + 2], "=", "{name}.{field}");
                            assert_eq!(tokens[i + 4], ";", "{name}.{field}: options are not generated");
                            fields.push(Field { name: field, ty, repeated });
                            i += 5;
                        }
                    }
                }
                i += 1;
                items.push(Item::Message { name, fields });
            }
            "enum" => {
                let name = tokens[i + 1].clone();
                assert_eq!(tokens[i + 2], "{", "enum {name}");
                i += 3;
                let mut values = Vec::new();
                while tokens[i] != "}" {
                    if tokens[i] == "reserved" || tokens[i] == "option" {
                        skip_statement(&mut i);
                        continue;
                    }
                    assert_eq!(tokens[i + 1], "=", "{name}.{}", tokens[i]);
                    let number: i32 = tokens[i + 2].parse().unwrap();
                    assert_eq!(tokens[i + 3], ";", "{name}.{}: options are not generated", tokens[i]);
                    values.push((tokens[i].clone(), number));
                    i += 4;
                }
                i += 1;
                items.push(Item::Enum { name, values });
            }
            other => panic!("build.rs does not read `{other}` in gem.proto"),
        }
    }
    items
}

fn camel(snake: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn pascal(snake: &str) -> String {
    let camel = camel(&snake.to_lowercase());
    let mut chars = camel.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

fn snake(pascal: &str) -> String {
    let mut out = String::new();
    for (i, c) in pascal.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

// The variant prost names a value: without the enum's name as a prefix, in PascalCase.
fn variant(enum_name: &str, value: &str) -> String {
    let prefix = snake(enum_name).to_uppercase() + "_";
    pascal(value.strip_prefix(&prefix).unwrap_or(value))
}

enum Kind {
    Scalar(&'static str),
    // A 64-bit integer, a string in JSON.
    Wide(&'static str),
    Message,
    Enum,
}

fn kind(ty: &str, enums: &[&str]) -> Kind {
    match ty {
        "string" => Kind::Scalar("String"),
        "bool" => Kind::Scalar("bool"),
        "uint32" | "fixed32" => Kind::Scalar("u32"),
        "int32" | "sint32" | "sfixed32" => Kind::Scalar("i32"),
        "uint64" | "fixed64" => Kind::Wide("u64"),
        "int64" | "sint64" | "sfixed64" => Kind::Wide("i64"),
        "double" | "float" | "bytes" => panic!("build.rs does not generate {ty} fields yet"),
        _ if enums.contains(&ty) => Kind::Enum,
        _ => Kind::Message,
    }
}

fn rust_type(ty: &str) -> String {
    match ty {
        "google.protobuf.Timestamp" => "Timestamp".to_string(),
        other => other.to_string(),
    }
}

fn generate(items: &[Item]) -> String {
    let enums: Vec<&str> = (items.iter())
        .filter_map(|item| match item {
            Item::Enum { name, .. } => Some(name.as_str()),
            Item::Message { .. } => None,
        })
        .collect();
    let mut out = String::from("// Generated by build.rs from protocol/gem.proto.\n\n");
    for item in items {
        match item {
            Item::Enum { name, values } => enum_code(&mut out, name, values),
            Item::Message { name, fields } => message_code(&mut out, name, fields, &enums),
        }
    }
    out
}

fn enum_code(out: &mut String, name: &str, values: &[(String, i32)]) {
    let w = &mut *out;
    writeln!(w, "#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]").unwrap();
    writeln!(w, "pub enum {name} {{").unwrap();
    for (i, (value, number)) in values.iter().enumerate() {
        if i == 0 {
            writeln!(w, "    #[default]").unwrap();
        }
        writeln!(w, "    {} = {number},", variant(name, value)).unwrap();
    }
    writeln!(w, "}}\n").unwrap();
    writeln!(w, "impl {name} {{").unwrap();
    writeln!(w, "    pub fn as_str_name(&self) -> &'static str {{\n        match self {{").unwrap();
    for (value, _) in values {
        writeln!(w, "            {name}::{} => {value:?},", variant(name, value)).unwrap();
    }
    writeln!(w, "        }}\n    }}\n").unwrap();
    writeln!(w, "    pub fn from_str_name(name: &str) -> Option<{name}> {{\n        match name {{").unwrap();
    for (value, _) in values {
        writeln!(w, "            {value:?} => Some({name}::{}),", variant(name, value)).unwrap();
    }
    writeln!(w, "            _ => None,\n        }}\n    }}\n").unwrap();
    writeln!(w, "    pub fn from_i32(number: i32) -> Option<{name}> {{\n        match number {{").unwrap();
    for (value, number) in values {
        writeln!(w, "            {number} => Some({name}::{}),", variant(name, value)).unwrap();
    }
    writeln!(w, "            _ => None,\n        }}\n    }}\n}}\n").unwrap();
    writeln!(w, "impl TryFrom<i32> for {name} {{\n    type Error = i32;\n").unwrap();
    writeln!(w, "    fn try_from(number: i32) -> Result<{name}, i32> {{\n        {name}::from_i32(number).ok_or(number)\n    }}\n}}\n").unwrap();
    writeln!(w, "impl From<{name}> for i32 {{\n    fn from(value: {name}) -> i32 {{\n        value as i32\n    }}\n}}\n").unwrap();
}

fn message_code(out: &mut String, name: &str, fields: &[Field], enums: &[&str]) {
    let w = &mut *out;
    writeln!(w, "#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]").unwrap();
    writeln!(w, "pub struct {name} {{").unwrap();
    for f in fields {
        let ty = match kind(&f.ty, enums) {
            Kind::Scalar(t) | Kind::Wide(t) => t.to_string(),
            Kind::Enum => "i32".to_string(),
            Kind::Message if f.repeated => rust_type(&f.ty),
            Kind::Message => format!("Option<{}>", rust_type(&f.ty)),
        };
        let ty = if f.repeated { format!("Vec<{ty}>") } else { ty };
        writeln!(w, "    pub {}: {ty},", f.name).unwrap();
    }
    writeln!(w, "}}\n").unwrap();

    // prost's getter for each enum field.
    let enum_fields: Vec<&Field> = (fields.iter())
        .filter(|f| !f.repeated && matches!(kind(&f.ty, enums), Kind::Enum))
        .collect();
    if !enum_fields.is_empty() {
        writeln!(w, "impl {name} {{").unwrap();
        for f in enum_fields {
            writeln!(
                w,
                "    pub fn {}(&self) -> {} {{\n        {}::from_i32(self.{}).unwrap_or_default()\n    }}\n",
                f.name, f.ty, f.ty, f.name
            )
            .unwrap();
        }
        writeln!(w, "}}\n").unwrap();
    }

    // pbjson leaves out every field at its default value.
    writeln!(w, "impl gem_json::ToJson for {name} {{").unwrap();
    writeln!(w, "    fn to_json(&self) -> gem_json::Value {{").unwrap();
    let object = if fields.is_empty() { "object" } else { "mut object" };
    writeln!(w, "        let {object} = gem_json::Map::new();").unwrap();
    for f in fields {
        let key = camel(&f.name);
        let field = &f.name;
        let (test, value) = match (kind(&f.ty, enums), f.repeated) {
            (Kind::Scalar("String"), false) => (format!("!self.{field}.is_empty()"), format!("gem_json::ToJson::to_json(&self.{field})")),
            (Kind::Scalar("bool"), false) => (format!("self.{field}"), "gem_json::Value::Bool(true)".to_string()),
            (Kind::Scalar(_), false) => (format!("self.{field} != 0"), format!("gem_json::ToJson::to_json(&self.{field})")),
            (Kind::Wide(_), false) => (format!("self.{field} != 0"), format!("gem_json::Value::String(self.{field}.to_string())")),
            (Kind::Enum, false) => (
                format!("self.{field} != 0"),
                format!(
                    "match {}::from_i32(self.{field}) {{ Some(v) => gem_json::Value::String(v.as_str_name().to_string()), None => gem_json::ToJson::to_json(&self.{field}) }}",
                    f.ty
                ),
            ),
            (Kind::Message, false) => (format!("self.{field}.is_some()"), format!("gem_json::ToJson::to_json(&self.{field})")),
            (Kind::Wide(_), true) => (
                format!("!self.{field}.is_empty()"),
                format!("gem_json::Value::Array(self.{field}.iter().map(|n| gem_json::Value::String(n.to_string())).collect())"),
            ),
            (Kind::Enum, true) => (
                format!("!self.{field}.is_empty()"),
                format!(
                    "gem_json::Value::Array(self.{field}.iter().map(|n| match {}::from_i32(*n) {{ Some(v) => gem_json::Value::String(v.as_str_name().to_string()), None => gem_json::ToJson::to_json(n) }}).collect())",
                    f.ty
                ),
            ),
            (_, true) => (format!("!self.{field}.is_empty()"), format!("gem_json::ToJson::to_json(&self.{field})")),
        };
        writeln!(w, "        if {test} {{\n            object.insert({key:?}.to_string(), {value});\n        }}").unwrap();
    }
    writeln!(w, "        gem_json::Value::Object(object)\n    }}\n}}\n").unwrap();

    // pbjson reads either name of a field, null as the default, and integers from strings too.
    writeln!(w, "impl gem_json::FromJson for {name} {{").unwrap();
    writeln!(w, "    fn from_json(value: &gem_json::Value) -> Result<{name}, gem_json::Error> {{").unwrap();
    writeln!(w, "        if !value.is_object() {{\n            return Err(gem_json::Error::new(format!(\"expected {name}, found {{}}\", value.kind())));\n        }}").unwrap();
    if fields.is_empty() {
        writeln!(w, "        Ok({name} {{}})\n    }}\n}}\n").unwrap();
        return;
    }
    writeln!(w, "        let get = |snake: &str, camel: &str| match value.get(camel) {{").unwrap();
    writeln!(w, "            Some(found) if !found.is_null() => Some(found),").unwrap();
    writeln!(w, "            _ => value.get(snake).filter(|found| !found.is_null()),").unwrap();
    writeln!(w, "        }};").unwrap();
    writeln!(w, "        Ok({name} {{").unwrap();
    for f in fields {
        let read = match (kind(&f.ty, enums), f.repeated) {
            (Kind::Scalar("String" | "bool"), _) | (Kind::Message, _) => "gem_json::FromJson::from_json(v)".to_string(),
            (Kind::Scalar(t) | Kind::Wide(t), false) => format!("crate::read_integer::<{t}>(v)"),
            (Kind::Scalar(t) | Kind::Wide(t), true) => format!("crate::read_each(v, crate::read_integer::<{t}>)"),
            (Kind::Enum, false) => format!("crate::read_enum(v, {}::from_str_name)", f.ty),
            (Kind::Enum, true) => format!("crate::read_each(v, |v| crate::read_enum(v, {}::from_str_name))", f.ty),
        };
        writeln!(
            w,
            "            {}: match get({:?}, {:?}) {{\n                Some(v) => {read}.map_err(|e| e.at({:?}))?,\n                None => Default::default(),\n            }},",
            f.name,
            f.name,
            camel(&f.name),
            camel(&f.name)
        )
        .unwrap();
    }
    writeln!(w, "        }})\n    }}\n}}\n").unwrap();
}
