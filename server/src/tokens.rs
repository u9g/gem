// The bearer tokens of agents that propose. Each agent gets its own, bound to what it answers, and
// reaches the `gem-agent` MCP server with it; a token proposes only on its own subject. Tokens live
// in the database, so an agent keeps proposing across server restarts, and go when their subject
// does.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use gem_db::{
    Db,
    rusqlite::{OptionalExtension, params},
};
use serde::{Deserialize, Serialize};

const MIGRATIONS: &[&str] = &["
    CREATE TABLE agent_tokens (
        token TEXT PRIMARY KEY,
        -- A Subject, as JSON.
        subject TEXT NOT NULL
    );
"];

// The --mcp-config each token's agent was started with, as `<token>.mcp.json` in this directory,
// relative to the server's working directory: T3 Code reads it from a file, again for as long as
// the token lasts.
pub const DIR: &str = "agent-tokens";

pub fn migrate(db: &Db) -> Result<(), String> {
    db.migrate("agent_tokens", MIGRATIONS)
}

// What an agent answers, and so what it may propose on.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
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
        Self::new(crate::db().clone(), std::env::current_dir().unwrap().join(DIR))
    }

    // A new token for an agent answering `subject`, replacing any earlier agent's for it, with
    // the --mcp-config that points the agent at `url` with it. Returns the config's path.
    pub async fn issue(&self, subject: &Subject, url: &str) -> Result<PathBuf, String> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| format!("cannot make a token: {e}"))?;
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let config = serde_json::json!({
            "mcpServers": {
                crate::mcp::AGENT_NAME: {
                    "type": "http",
                    "url": url,
                    "headers": { "Authorization": format!("Bearer {token}") },
                },
            },
        });

        let config_path = self.config_path(&token);
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|e| format!("{}: {e}", self.dir.display()))?;
        tokio::fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap())
            .await
            .map_err(|e| format!("{}: {e}", config_path.display()))?;
        let revoked = self.remove(|other| other.id() == subject.id(), Some((&token, subject)))?;
        self.remove_configs(revoked).await;
        Ok(config_path)
    }

    // The subject of `token`, or None when it is not one gem issued or it was revoked.
    pub async fn subject(&self, token: &str) -> Option<Subject> {
        let json: Option<String> = self
            .db
            .with(|db| {
                db.query_row(
                    "SELECT subject FROM agent_tokens WHERE token = ?1",
                    [token],
                    |row| row.get(0),
                )
                .optional()
            })
            .ok()?;
        serde_json::from_str(&json?).ok()
    }

    // Revokes the tokens whose subjects `keep` returns false for.
    pub async fn retain(&self, keep: impl Fn(&Subject) -> bool) {
        match self.remove(|subject| !keep(subject), None) {
            Ok(revoked) => self.remove_configs(revoked).await,
            Err(error) => tracing::warn!(%error, "cannot revoke tokens"),
        }
    }

    // Revokes the tokens of the investigation or conversation `id`.
    pub async fn revoke(&self, id: &str) {
        self.retain(|subject| subject.id() != id).await;
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
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            let revoked: Vec<String> = (tokens.into_iter())
                .filter(|(_, subject)| {
                    serde_json::from_str::<Subject>(subject).is_ok_and(|subject| revoked(&subject))
                })
                .map(|(token, _)| token)
                .collect();
            for token in &revoked {
                tx.execute("DELETE FROM agent_tokens WHERE token = ?1", [token])?;
            }
            if let Some((token, subject)) = issued {
                tx.execute(
                    "INSERT INTO agent_tokens (token, subject) VALUES (?1, ?2)",
                    params![token, serde_json::to_string(subject).unwrap()],
                )?;
            }
            tx.commit()?;
            Ok(revoked)
        })
    }

    async fn remove_configs(&self, tokens: Vec<String>) {
        for token in tokens {
            remove_file(&self.config_path(&token)).await;
        }
    }

    fn config_path(&self, token: &str) -> PathBuf {
        self.dir.join(format!("{token}.mcp.json"))
    }
}

async fn remove_file(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "cannot revoke token");
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
        let db = Db::open_in_memory().unwrap();
        crate::migrate(&db).unwrap();
        db
    }

    // Tokens in a fresh database, with their configs in a fresh directory.
    pub fn tokens(name: &str) -> Tokens {
        Tokens::new(db(), temp_dir(name))
    }

    // The token in a --mcp-config `issue` wrote.
    pub fn token(config: &Path) -> String {
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
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

    #[tokio::test]
    async fn an_issued_token_points_its_agent_at_the_server_with_its_subject() {
        let tokens = tokens("issue");
        let config = tokens
            .issue(&comment("IC_1"), "http://127.0.0.1:1/mcp")
            .await
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        let server = &json["mcpServers"][crate::mcp::AGENT_NAME];
        assert_eq!(server["type"], "http");
        assert_eq!(server["url"], "http://127.0.0.1:1/mcp");
        assert_eq!(tokens.subject(&token(&config)).await, Some(comment("IC_1")));
    }

    #[tokio::test]
    async fn a_new_agent_for_a_subject_revokes_the_earlier_ones_token() {
        let tokens = tokens("replace");
        let url = "http://127.0.0.1:1/mcp";
        let first = token(&tokens.issue(&comment("IC_1"), url).await.unwrap());
        let other = token(&tokens.issue(&comment("IC_2"), url).await.unwrap());
        let second = token(&tokens.issue(&comment("IC_1"), url).await.unwrap());
        assert_eq!(tokens.subject(&first).await, None);
        assert_eq!(tokens.subject(&second).await, Some(comment("IC_1")));
        assert_eq!(tokens.subject(&other).await, Some(comment("IC_2")));
    }

    #[tokio::test]
    async fn revoking_a_subject_removes_its_token_and_config() {
        let tokens = tokens("revoke");
        let config = tokens
            .issue(&comment("IC_1"), "http://127.0.0.1:1/mcp")
            .await
            .unwrap();
        let token = token(&config);
        tokens.revoke("IC_1").await;
        assert_eq!(tokens.subject(&token).await, None);
        assert!(!config.exists());
    }
}
