// The server's configuration variables: the real environment, then what the server set itself
// from .env and 1Password. Setting a real environment variable is unsafe in Rust 2024, and lust
// compiles no unsafe code, so those live here instead, and each child process the server starts
// is given them with `apply`.

use std::{
    collections::BTreeMap,
    path::Path,
    process::Command,
    sync::{Mutex, OnceLock},
};

fn set_here() -> &'static Mutex<BTreeMap<String, String>> {
    static SET: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    SET.get_or_init(Mutex::default)
}

// The variable from the real environment, or as the server set it.
pub fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().or_else(|| set_here().lock().unwrap().get(name).cloned())
}

pub fn is_set(name: &str) -> bool {
    var(name).is_some()
}

// Sets the variable unless the real environment has it, which wins.
pub fn set(name: &str, value: &str) {
    if std::env::var_os(name).is_none() {
        set_here().lock().unwrap().insert(name.to_string(), value.to_string());
    }
}

// The variables the server set, by name.
pub fn set_vars() -> BTreeMap<String, String> {
    set_here().lock().unwrap().clone()
}

// Gives `command` the variables the server set.
pub fn apply(command: &mut Command) -> &mut Command {
    command.envs(set_vars())
}

// Reads a .env file: `NAME=value` lines, with `export ` before the name, `#` comments, and values
// in single or double quotes allowed, as dotenvy reads them. None when the file does not exist.
pub fn read_dotenv(path: &Path) -> Option<Result<Vec<(String, String)>, String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => return Some(Err(error.to_string())),
    };
    Some(parse_dotenv(&text))
}

pub fn parse_dotenv(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut vars = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((name, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected NAME=value", i + 1));
        };
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
            return Err(format!("line {}: invalid name {name:?}", i + 1));
        }
        let value = value.trim();
        let value = if let Some(quoted) = value.strip_prefix('\'') {
            quoted.split('\'').next().unwrap_or_default().to_string()
        } else if let Some(quoted) = value.strip_prefix('"') {
            let mut out = String::new();
            let mut chars = quoted.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => out.push('\n'),
                        Some(other) => out.push(other),
                        None => break,
                    },
                    c => out.push(c),
                }
            }
            out
        } else {
            // An unquoted value ends at a comment.
            value.split(" #").next().unwrap_or_default().trim().to_string()
        };
        vars.push((name.to_string(), value));
    }
    Ok(vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_dotenv() {
        let vars = parse_dotenv("# x\nA=1\nexport B = 'two words'\nC=\"q\\\"d\\n\" \nD=v # comment\n\n").unwrap();
        let expected = [("A", "1"), ("B", "two words"), ("C", "q\"d\n"), ("D", "v")];
        assert_eq!(vars, expected.map(|(n, v)| (n.to_string(), v.to_string())));
        assert!(parse_dotenv("nope").is_err());
    }

    #[test]
    fn the_real_environment_wins() {
        set("GEM_ENV_TEST_ONLY_HERE", "here");
        assert_eq!(var("GEM_ENV_TEST_ONLY_HERE").as_deref(), Some("here"));
        let path = std::env::var("PATH").unwrap();
        set("PATH", "nope");
        assert_eq!(var("PATH"), Some(path));
        assert!(!set_vars().contains_key("PATH"));
    }
}
