// The bearer tokens of agents that propose. Each agent gets its own, bound to what it answers, and
// reaches the `gem-agent` MCP server with it; a token proposes only on its own subject. Tokens live
// in the database, so an agent keeps proposing across server restarts, and go when their subject
// does.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use gem_db::{Db, texts};
use serde::{Deserialize, Serialize};

// The --mcp-config each token's agent was started with, as `<token>.mcp.json` in this directory,
// relative to the server's working directory: T3 Code reads it from a file, again for as long as
// the token lasts.
pub const DIR: &str = "agent-tokens";

// In the database's moment, the hash map from each token to its Subject, as JSON.
pub const TOKENS: &str = "agent-tokens";

// What an agent answers, and so what it may propose on.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    // The comments waiting on the user in the cluster holding `pr`, whichever pull requests it
    // holds when the agent proposes.
    Cluster { pr: String },
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
            Subject::Cluster { pr } => pr,
            Subject::CheckFailure { id, .. } | Subject::Investigation { id, .. } => id,
        }
    }

    // The PRs it may propose on. A cluster's agent may also propose on the others its cluster
    // holds with `pr`.
    pub fn prs(&self) -> &[String] {
        match self {
            Subject::Cluster { pr } | Subject::CheckFailure { pr, .. } => {
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
        let json = self.db.read(|moment| match moment.get_map(TOKENS)? {
            Some(tokens) => tokens.get_string(token, None),
            None => Ok(None),
        });
        let json = json.ok()?;
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
        self.db.write(|moment| {
            let revoked: Vec<String> = (texts(moment, TOKENS)?.into_iter())
                .filter(|(_, subject)| {
                    serde_json::from_str::<Subject>(subject).is_ok_and(|subject| revoked(&subject))
                })
                .map(|(token, _)| token)
                .collect();
            let mut tokens = moment.put_map(TOKENS)?;
            for token in &revoked {
                tokens.remove(token)?;
            }
            if let Some((token, subject)) = issued {
                tokens.put(token, serde_json::to_string(subject).unwrap())?;
            }
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

    // An empty database.
    pub fn db() -> Db {
        Db::open_in_memory().unwrap()
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

    fn failure(id: &str) -> Subject {
        Subject::CheckFailure {
            pr: "https://github.com/o/r/pull/1".into(),
            id: id.into(),
        }
    }

    #[tokio::test]
    async fn an_issued_token_points_its_agent_at_the_server_with_its_subject() {
        let tokens = tokens("issue");
        let config = tokens
            .issue(&failure("IC_1"), "http://127.0.0.1:1/mcp")
            .await
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        let server = &json["mcpServers"][crate::mcp::AGENT_NAME];
        assert_eq!(server["type"], "http");
        assert_eq!(server["url"], "http://127.0.0.1:1/mcp");
        assert_eq!(tokens.subject(&token(&config)).await, Some(failure("IC_1")));
    }

    #[tokio::test]
    async fn a_new_agent_for_a_subject_revokes_the_earlier_ones_token() {
        let tokens = tokens("replace");
        let url = "http://127.0.0.1:1/mcp";
        let first = token(&tokens.issue(&failure("IC_1"), url).await.unwrap());
        let other = token(&tokens.issue(&failure("IC_2"), url).await.unwrap());
        let second = token(&tokens.issue(&failure("IC_1"), url).await.unwrap());
        assert_eq!(tokens.subject(&first).await, None);
        assert_eq!(tokens.subject(&second).await, Some(failure("IC_1")));
        assert_eq!(tokens.subject(&other).await, Some(failure("IC_2")));
    }

    #[tokio::test]
    async fn revoking_a_subject_removes_its_token_and_config() {
        let tokens = tokens("revoke");
        let config = tokens
            .issue(&failure("IC_1"), "http://127.0.0.1:1/mcp")
            .await
            .unwrap();
        let token = token(&config);
        tokens.revoke("IC_1").await;
        assert_eq!(tokens.subject(&token).await, None);
        assert!(!config.exists());
    }
}
