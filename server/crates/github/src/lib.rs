// GitHub, through the gh CLI and the account it is authenticated as.

use std::sync::OnceLock;

use serde::de::DeserializeOwned;
use tokio::process::Command;

static LOGIN: OnceLock<String> = OnceLock::new();

// Requires an authenticated `gh` on PATH. gh's default limit is 30, below the repo and PR counts.
pub async fn gh(args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("gh").args(args).output().await.ok()?;
    output.status.success().then_some(output.stdout)
}

pub async fn gh_json<T: DeserializeOwned>(args: &[&str]) -> Option<T> {
    serde_json::from_slice(&gh(args).await?).ok()
}

// Asks gh who it is authenticated as, for `login`. Errors when gh is not authenticated.
pub async fn sign_in() -> Result<(), String> {
    let login = (gh(&["api", "user", "--jq", ".login"]).await)
        .ok_or("gh is not authenticated")?;
    let _ = LOGIN.set(String::from_utf8_lossy(&login).trim().to_string());
    Ok(())
}

// The GitHub user gh is authenticated as: whose PRs are listed, and who proposals are run as.
// Panics before `sign_in`.
pub fn login() -> &'static str {
    LOGIN.get().expect("gem_github::sign_in runs first")
}
