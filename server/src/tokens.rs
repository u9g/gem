// The bearer tokens of agents that propose. Each agent gets its own, bound to what it answers, and
// reaches the `gem-agent` MCP server with it; a token proposes only on its own subject. Tokens live
// in the database, so an agent keeps proposing across server restarts, and go when their subject
// does.

use std::{
    hash::{BuildHasher, Hasher, RandomState},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use gem_db::{Db, OptionalExtension, params};
use gem_json::{Error, FromJson, ToJson, Value, json};
use gem_log::warn;

// The --mcp-config each token's agent was started with, as `<token>.mcp.json` in this directory,
// relative to the server's working directory: T3 Code reads it from a file, again for as long as
// the token lasts.
pub const DIR: &str = "agent-tokens";

// What an agent answers, and so what it may propose on. As JSON, it is tagged with its variant's
// snake_case name: `{"check_failure": {"pr": .., "id": ..}}`.
#[derive(Clone, Debug, PartialEq)]
pub enum Subject {
    // A comment on `pr`, in the review thread `thread` when it has one.
    Comment {
        pr: String,
        comment: String,
        thread: Option<String>,
    },
    // The failed checks on `pr` with that check failure ID.
    CheckFailure {
        pr: String,
        id: String,
    },
    // An investigation, proposing on any of `prs`.
    Investigation {
        id: String,
        prs: Vec<String>,
    },
}

impl ToJson for Subject {
    fn to_json(&self) -> Value {
        match self {
            Subject::Comment {
                pr,
                comment,
                thread,
            } => json!({ "comment": { "pr": pr, "comment": comment, "thread": thread } }),
            Subject::CheckFailure { pr, id } => json!({ "check_failure": { "pr": pr, "id": id } }),
            Subject::Investigation { id, prs } => {
                json!({ "investigation": { "id": id, "prs": prs } })
            }
        }
    }
}

impl FromJson for Subject {
    fn from_json(value: &Value) -> Result<Subject, Error> {
        let Some((variant, fields)) = (value.as_object())
            .filter(|object| object.len() == 1)
            .and_then(|object| object.iter().next())
        else {
            return Err(Error::new(format!(
                "expected a subject, found {}",
                value.kind()
            )));
        };
        let subject = match variant.as_str() {
            "comment" => Subject::Comment {
                pr: fields.field("pr")?,
                comment: fields.field("comment")?,
                thread: fields.field("thread")?,
            },
            "check_failure" => Subject::CheckFailure {
                pr: fields.field("pr")?,
                id: fields.field("id")?,
            },
            "investigation" => Subject::Investigation {
                id: fields.field("id")?,
                prs: fields.field("prs")?,
            },
            other => return Err(Error::new(format!("unknown subject {other}"))),
        };
        Ok(subject)
    }
}

impl Subject {
    // The ID of the conversation or investigation, which no other subject shares.
    pub fn id(&self) -> &str {
        match self {
            Subject::Comment { comment, .. } => comment,
            Subject::CheckFailure { id, .. } | Subject::Investigation { id, .. } => id,
        }
    }

    // The PRs it may propose on.
    pub fn prs(&self) -> &[String] {
        match self {
            Subject::Comment { pr, .. } | Subject::CheckFailure { pr, .. } => {
                std::slice::from_ref(pr)
            }
            Subject::Investigation { prs, .. } => prs,
        }
    }
}

#[derive(Clone)]
pub struct Tokens {
    db: Db,
    // Where the --mcp-configs go.
    dir: PathBuf,
}

impl Tokens {
    pub fn new(db: Db, dir: PathBuf) -> Self {
        Self { db, dir }
    }

    // The server's own, with its configs in its working directory.
    pub fn here() -> Self {
        Self::new(
            crate::db().clone(),
            std::env::current_dir().unwrap().join(DIR),
        )
    }

    // A new token for an agent answering `subject`, replacing any earlier agent's for it, with
    // the --mcp-config that points the agent at `url` with it. Returns the config's path.
    pub fn issue(&self, subject: &Subject, url: &str) -> Result<PathBuf, String> {
        let token = random_token();
        let config = json!({
            "mcpServers": {
                [crate::mcp::AGENT_NAME]: {
                    "type": "http",
                    "url": url,
                    "headers": { "Authorization": (format!("Bearer {token}")) },
                },
            },
        });

        let config_path = self.config_path(&token);
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        std::fs::write(&config_path, config.to_string_pretty())
            .map_err(|e| format!("{}: {e}", config_path.display()))?;
        let revoked = self.remove(|other| other.id() == subject.id(), Some((&token, subject)))?;
        self.remove_configs(revoked);
        Ok(config_path)
    }

    // The subject of `token`, or None when it is not one gem issued or it was revoked.
    pub fn subject(&self, token: &str) -> Option<Subject> {
        let json: Option<String> = self
            .db
            .with(|db| {
                db.query_row(
                    "SELECT subject FROM agent_tokens WHERE token = ?1",
                    params![token],
                    |row| row.get::<String>(0),
                )
                .optional()
            })
            .ok()?;
        gem_json::from_str(&json?).ok()
    }

    // Revokes the tokens whose subjects `keep` returns false for.
    pub fn retain(&self, keep: impl Fn(&Subject) -> bool) {
        match self.remove(|subject| !keep(subject), None) {
            Ok(revoked) => self.remove_configs(revoked),
            Err(error) => warn!("cannot revoke tokens", error = error),
        }
    }

    // Revokes the tokens of the investigation or conversation `id`.
    pub fn revoke(&self, id: &str) {
        self.retain(|subject| subject.id() != id);
    }

    // Deletes the tokens whose subjects are `revoked`, and then adds `issued`, at once. Returns the
    // deleted tokens.
    fn remove(
        &self,
        revoked: impl Fn(&Subject) -> bool,
        issued: Option<(&str, &Subject)>,
    ) -> Result<Vec<String>, String> {
        self.db.with(|db| {
            let tx = db.unchecked_transaction()?;
            let tokens: Vec<(String, String)> = tx
                .prepare("SELECT token, subject FROM agent_tokens")?
                .query_map(params![], |row| {
                    Ok((row.get::<String>(0)?, row.get::<String>(1)?))
                })?
                .collect::<Result<_, _>>()?;
            let revoked: Vec<String> = (tokens.into_iter())
                .filter(|(_, subject)| {
                    gem_json::from_str::<Subject>(subject).is_ok_and(|subject| revoked(&subject))
                })
                .map(|(token, _)| token)
                .collect();
            for token in &revoked {
                tx.execute("DELETE FROM agent_tokens WHERE token = ?1", params![token])?;
            }
            if let Some((token, subject)) = issued {
                tx.execute(
                    "INSERT INTO agent_tokens (token, subject) VALUES (?1, ?2)",
                    params![token, gem_json::to_string(subject)],
                )?;
            }
            tx.commit()?;
            Ok(revoked)
        })
    }

    fn remove_configs(&self, tokens: Vec<String>) {
        for token in tokens {
            remove_file(&self.config_path(&token));
        }
    }

    fn config_path(&self, token: &str) -> PathBuf {
        self.dir.join(format!("{token}.mcp.json"))
    }
}

// 32 random bytes, in hex, from the OS's random source: /dev/urandom where there is one, which
// std can read without the unsafe code a call to the OS needs.
fn random_token() -> String {
    use std::io::Read;

    let mut bytes = [0u8; 32];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if read.is_ok() {
        return bytes.iter().map(|b| format!("{b:02x}")).collect();
    }
    // Windows has none. std keys each thread's first RandomState's SipHash with random bits from
    // the OS, and each later one's with those plus one, so each hash also takes the time and the
    // process.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut token = String::new();
    for i in 0..4u64 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(i);
        hasher.write_u128(nanos);
        hasher.write_u32(std::process::id());
        token.push_str(&format!("{:016x}", hasher.finish()));
    }
    token
}

fn remove_file(path: &Path) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != ErrorKind::NotFound
    {
        warn!("cannot revoke token", path = path.display(), error = error);
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    // A fresh directory under the system's temporary one.
    pub fn temp_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("gem-test-{name}-{nanos:x}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // A database with every table the server's state needs, and nothing in them.
    pub fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    // Tokens in a fresh database, with their configs in a fresh directory.
    pub fn tokens(name: &str) -> Tokens {
        Tokens::new(db(), temp_dir(name))
    }

    // The token in a --mcp-config `issue` wrote.
    pub fn token(config: &Path) -> String {
        let config: Value = gem_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
        let header = &config["mcpServers"][crate::mcp::AGENT_NAME]["headers"]["Authorization"];
        header
            .as_str()
            .unwrap()
            .strip_prefix("Bearer ")
            .unwrap()
            .to_string()
    }

    fn comment(id: &str) -> Subject {
        Subject::Comment {
            pr: "https://github.com/o/r/pull/1".into(),
            comment: id.into(),
            thread: None,
        }
    }

    #[test]
    fn an_issued_token_points_its_agent_at_the_server_with_its_subject() {
        let tokens = tokens("issue");
        let config = tokens
            .issue(&comment("IC_1"), "http://127.0.0.1:1/mcp")
            .unwrap();
        let json: Value = gem_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        let server = &json["mcpServers"][crate::mcp::AGENT_NAME];
        assert_eq!(server["type"], "http");
        assert_eq!(server["url"], "http://127.0.0.1:1/mcp");
        assert_eq!(tokens.subject(&token(&config)), Some(comment("IC_1")));
    }

    #[test]
    fn a_new_agent_for_a_subject_revokes_the_earlier_ones_token() {
        let tokens = tokens("replace");
        let url = "http://127.0.0.1:1/mcp";
        let first = token(&tokens.issue(&comment("IC_1"), url).unwrap());
        let other = token(&tokens.issue(&comment("IC_2"), url).unwrap());
        let second = token(&tokens.issue(&comment("IC_1"), url).unwrap());
        assert_eq!(tokens.subject(&first), None);
        assert_eq!(tokens.subject(&second), Some(comment("IC_1")));
        assert_eq!(tokens.subject(&other), Some(comment("IC_2")));
    }

    #[test]
    fn revoking_a_subject_removes_its_token_and_config() {
        let tokens = tokens("revoke");
        let config = tokens
            .issue(&comment("IC_1"), "http://127.0.0.1:1/mcp")
            .unwrap();
        let token = token(&config);
        tokens.revoke("IC_1");
        assert_eq!(tokens.subject(&token), None);
        assert!(!config.exists());
    }

    // Subjects keep the JSON serde gave them, so tokens issued before still read.
    #[test]
    fn subjects_read_and_write_their_tagged_json() {
        let json =
            r#"{"comment":{"comment":"IC_1","pr":"https://github.com/o/r/pull/1","thread":null}}"#;
        assert_eq!(gem_json::to_string(&comment("IC_1")), json);
        assert_eq!(gem_json::from_str::<Subject>(json), Ok(comment("IC_1")));
        // serde wrote fields in declaration order, and read a missing thread as none.
        let serde = r#"{"comment":{"pr":"https://github.com/o/r/pull/1","comment":"IC_1"}}"#;
        assert_eq!(gem_json::from_str::<Subject>(serde), Ok(comment("IC_1")));
        let failure = Subject::CheckFailure {
            pr: "p".into(),
            id: "abc,CR_1".into(),
        };
        let json = r#"{"check_failure":{"id":"abc,CR_1","pr":"p"}}"#;
        assert_eq!(gem_json::to_string(&failure), json);
        assert_eq!(gem_json::from_str::<Subject>(json), Ok(failure));
    }
}
