// GitHub, through the gh CLI and the account it is authenticated as.

use std::sync::OnceLock;

use serde::de::DeserializeOwned;
use tokio::process::Command;

// Requires an authenticated `gh` on PATH. gh's default limit is 30, below the repo and PR counts.
pub async fn gh(args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("gh").args(args).output().await.ok()?;
    output.status.success().then_some(output.stdout)
}

pub async fn gh_json<T: DeserializeOwned>(args: &[&str]) -> Option<T> {
    serde_json::from_slice(&gh(args).await?).ok()
}

// The GitHub user gh is authenticated as: whose PRs are listed, and who proposals are run as.
// The first call asks gh, blocking, and panics when gh is not authenticated.
pub fn login() -> &'static str {
    static LOGIN: OnceLock<String> = OnceLock::new();
    LOGIN.get_or_init(|| {
        let output = std::process::Command::new("gh")
            .args(["api", "user", "--jq", ".login"])
            .output()
            .expect("cannot run gh");
        assert!(output.status.success(), "gh is not authenticated");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    })
}
