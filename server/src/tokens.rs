// The bearer tokens of agents that propose. Each agent gets its own, bound to what it answers, and
// reaches the `gem-agent` MCP server with it; a token proposes only on its own subject. Tokens live
// in the state directory, so an agent keeps proposing across server restarts, and go when their
// subject does.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

// Two files per token in this directory, relative to the server's working directory:
// `<token>.json` holds its subject, and `<token>.mcp.json` the --mcp-config its agent was started
// with, kept for as long as the token in case T3 Code reads it again.
pub const DIR: &str = "agent-tokens";

// Held across each change to the directory, so a token issued while others are revoked survives.
static FILES: Mutex<()> = Mutex::const_new(());

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
pub struct Tokens(PathBuf);

impl Tokens {
    pub fn new(dir: PathBuf) -> Self {
        Self(dir)
    }

    // The server's own, in its working directory.
    pub fn here() -> Self {
        Self(std::env::current_dir().unwrap().join(DIR))
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

        let _files = FILES.lock().await;
        self.remove(|other| other.id() == subject.id()).await;
        let write = async |path: PathBuf, json: Vec<u8>| {
            tokio::fs::write(&path, json)
                .await
                .map_err(|e| format!("{}: {e}", path.display()))
        };
        tokio::fs::create_dir_all(&self.0)
            .await
            .map_err(|e| format!("{}: {e}", self.0.display()))?;
        write(
            self.subject_path(&token),
            serde_json::to_vec(subject).unwrap(),
        )
        .await?;
        let config_path = self.0.join(format!("{token}.mcp.json"));
        write(
            config_path.clone(),
            serde_json::to_vec_pretty(&config).unwrap(),
        )
        .await?;
        Ok(config_path)
    }

    // The subject of `token`, or None when it is not one gem issued or it was revoked.
    pub async fn subject(&self, token: &str) -> Option<Subject> {
        // Only hex reads a file, so a token cannot name one outside the directory.
        if token.is_empty() || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let json = tokio::fs::read(self.subject_path(token)).await.ok()?;
        serde_json::from_slice(&json).ok()
    }

    // Revokes the tokens whose subjects `keep` returns false for.
    pub async fn retain(&self, keep: impl Fn(&Subject) -> bool) {
        let _files = FILES.lock().await;
        self.remove(|subject| !keep(subject)).await;
    }

    // Revokes the tokens of the investigation or conversation `id`.
    pub async fn revoke(&self, id: &str) {
        self.retain(|subject| subject.id() != id).await;
    }

    async fn remove(&self, revoked: impl Fn(&Subject) -> bool) {
        let Ok(mut entries) = tokio::fs::read_dir(&self.0).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(token) = name.strip_suffix(".json").filter(|t| !t.ends_with(".mcp")) else {
                continue;
            };
            let subject = tokio::fs::read(&path)
                .await
                .ok()
                .and_then(|json| serde_json::from_slice::<Subject>(&json).ok());
            if subject.as_ref().is_some_and(&revoked) {
                remove_file(&path).await;
                remove_file(&self.0.join(format!("{token}.mcp.json"))).await;
            }
        }
    }

    fn subject_path(&self, token: &str) -> PathBuf {
        self.0.join(format!("{token}.json"))
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
        let tokens = Tokens::new(temp_dir("issue"));
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
        let tokens = Tokens::new(temp_dir("replace"));
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
        let tokens = Tokens::new(temp_dir("revoke"));
        let config = tokens
            .issue(&comment("IC_1"), "http://127.0.0.1:1/mcp")
            .await
            .unwrap();
        let token = token(&config);
        tokens.revoke("IC_1").await;
        assert_eq!(tokens.subject(&token).await, None);
        assert!(!config.exists());
    }

    #[tokio::test]
    async fn a_token_that_is_not_hex_names_no_file() {
        let dir = temp_dir("traversal");
        let tokens = Tokens::new(dir.join(DIR));
        std::fs::write(
            dir.join("x.json"),
            serde_json::to_vec(&comment("IC_1")).unwrap(),
        )
        .unwrap();
        assert_eq!(tokens.subject("../x").await, None);
        assert_eq!(tokens.subject("").await, None);
    }
}
