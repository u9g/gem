// GitHub, through the gh and git CLIs and the account gh is authenticated as.

mod patch;

use std::{process::Stdio, sync::OnceLock};

use serde::de::DeserializeOwned;
use tokio::{io::AsyncWriteExt, process::Command};

pub use patch::apply_patch;

// Requires an authenticated `gh` on PATH. gh's default limit is 30, below the repo and PR counts.
// A caller that stops waiting, e.g. on a timeout, stops gh too.
pub async fn gh(what: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("gh")
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .inspect_err(|error| tracing::warn!(%error, what, "cannot run gh"))
        .ok()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(stderr = %stderr.trim(), what, "gh failed");
        return None;
    }
    Some(output.stdout)
}

pub async fn gh_json<T: DeserializeOwned>(what: &str, args: &[&str]) -> Option<T> {
    serde_json::from_slice(&gh(what, args).await?)
        .inspect_err(|error| tracing::warn!(%error, what, "cannot read gh's output"))
        .ok()
}

// gh's output parsed whatever its exit status. `gh api graphql` exits non-zero when any one field
// errors, e.g. a repository behind SAML enforcement the token is not authorized for, but still
// prints the other fields' data, which a query over many PRs wants rather than nothing.
pub async fn gh_partial_json<T: DeserializeOwned>(what: &str, args: &[&str]) -> Option<T> {
    let output = Command::new("gh")
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .inspect_err(|error| tracing::warn!(%error, what, "cannot run gh"))
        .ok()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(stderr = %stderr.trim(), what, "gh failed; reading what it printed");
    }
    serde_json::from_slice(&output.stdout)
        .inspect_err(|error| tracing::warn!(%error, what, "cannot read gh's output"))
        .ok()
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

// The repository's name and the pull request's number in `pr`'s URL.
pub fn short(pr: &str) -> (&str, &str) {
    let mut parts = pr.trim_end_matches('/').rsplit('/');
    let number = parts.next().unwrap_or_default();
    let repository = parts.nth(1).unwrap_or_default();
    (repository, number)
}

// `owner/name` of the pull request at `pr`.
pub fn repository(pr: &str) -> String {
    let parts: Vec<&str> = pr
        .trim_start_matches("https://github.com/")
        .split('/')
        .take(2)
        .collect();
    parts.join("/")
}

// Markdown as GitHub renders it in a comment on the pull request at `pr`.
pub async fn render(pr: &str, markdown: &str) -> Result<String, String> {
    if markdown.is_empty() {
        return Ok(String::new());
    }
    let context = format!("context={}", repository(pr));
    let html = command(
        "gh",
        &[
            "api", "markdown", "-f", "mode=gfm", "-f", &context, "-F", "text=@-",
        ],
        Some(markdown),
    )
    .await?;
    Ok(String::from_utf8_lossy(&html).into_owned())
}

pub async fn graphql(query: &str, variables: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    let mut args = vec![
        "api".to_string(),
        "graphql".to_string(),
        "-f".to_string(),
        format!("query={query}"),
    ];
    for (name, value) in variables {
        args.extend(["-f".to_string(), format!("{name}={value}")]);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    command("gh", &args, None).await
}

// Fails with the command's stderr, which is what the user sees on a failed approval.
pub async fn command(program: &str, args: &[&str], stdin: Option<&str>) -> Result<Vec<u8>, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    if let Some(input) = stdin {
        let mut pipe = child.stdin.take().unwrap();
        pipe.write_all(input.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
    }
    let output = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "{program}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
