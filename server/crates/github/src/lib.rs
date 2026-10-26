// GitHub, through the gh and git CLIs and the account gh is authenticated as.

mod patch;

use std::{collections::HashSet, process::Stdio, sync::OnceLock};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt, process::Command};

pub use patch::apply_patch;

// Requires an authenticated `gh` on PATH. gh's default limit is 30, below the repo and PR counts.
// A caller that stops waiting, e.g. on a timeout, stops gh too.
pub async fn gh(args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("gh").args(args).kill_on_drop(true).output().await.ok()?;
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

// A pull request's head commit, and the branch it targets.
pub struct Head<'a> {
    pub owner: &'a str,
    pub name: &'a str,
    pub base: &'a str,
    pub oid: &'a str,
}

// The indices of the heads that lack commits on their base branch, in one request. A field's
// arguments cannot come from another field, so search cannot compare them itself.
pub async fn behind(heads: &[Head<'_>]) -> HashSet<usize> {
    let fields: String = (heads.iter().enumerate())
        .map(|(i, head)| {
            format!(
                "pr{i}: repository(owner: {}, name: {}) {{ ref(qualifiedName: {}) {{ compare(headRef: {}) {{ behindBy }} }} }}
",
                json!(head.owner),
                json!(head.name),
                json!(head.base),
                json!(head.oid),
            )
        })
        .collect();
    if fields.is_empty() {
        return HashSet::new();
    }
    let Some(data) = gh_json::<Value>(&[
        "api",
        "graphql",
        "-f",
        &format!(
            "query={{
{fields}}}"
        ),
    ])
    .await
    else {
        return HashSet::new();
    };
    (0..heads.len())
        .filter(|i| data["data"][format!("pr{i}")]["ref"]["compare"]["behindBy"].as_u64() > Some(0))
        .collect()
}
