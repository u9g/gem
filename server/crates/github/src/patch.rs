// Applying an agent's diff to a pull request's branch.

use std::{
    env,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

use crate::command;

#[derive(Deserialize)]
struct Head {
    #[serde(rename = "headRefName")]
    branch: String,
    #[serde(rename = "headRepository")]
    repository: Name,
    #[serde(rename = "headRepositoryOwner")]
    owner: Login,
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

// Applies the diff to the head of the PR's branch in a fresh worktree, then commits and pushes
// when given a commit message, or only checks that it applies when not. Returns the head commit
// the diff was applied to. Each repository is cloned once and kept outside any T3 Code project.
pub async fn apply_patch(
    pr: &str,
    diff: &str,
    commit_message: Option<&str>,
) -> Result<String, String> {
    let head = command(
        "gh",
        &[
            "pr",
            "view",
            pr,
            "--json",
            "headRefName,headRepository,headRepositoryOwner",
        ],
        None,
    )
    .await?;
    let head: Head = serde_json::from_slice(&head).map_err(|e| e.to_string())?;
    let full_name = format!("{}/{}", head.owner.login, head.repository.name);

    let checkouts = env::temp_dir().join("gem-checkouts");
    let clone = checkouts.join(&full_name);
    let clone_str = clone.to_str().unwrap();
    if !clone.exists() {
        command(
            "gh",
            &[
                "repo",
                "clone",
                &full_name,
                clone_str,
                "--",
                "--filter=blob:none",
                "--no-checkout",
            ],
            None,
        )
        .await?;
    }
    command(
        "git",
        &["-C", clone_str, "fetch", "origin", &head.branch],
        None,
    )
    .await?;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let worktree = checkouts.join("worktrees").join(format!("{nanos:x}"));
    let worktree_str = worktree.to_str().unwrap();
    command(
        "git",
        &[
            "-C",
            clone_str,
            "worktree",
            "add",
            "--detach",
            worktree_str,
            &format!("origin/{}", head.branch),
        ],
        None,
    )
    .await?;
    let result = async {
        let oid = command("git", &["-C", worktree_str, "rev-parse", "HEAD"], None).await?;
        let oid = String::from_utf8_lossy(&oid).trim().to_string();
        match commit_message {
            None => {
                command(
                    "git",
                    &["-C", worktree_str, "apply", "--check", "-"],
                    Some(diff),
                )
                .await
            }
            Some(message) => {
                command("git", &["-C", worktree_str, "apply", "-"], Some(diff)).await?;
                command("git", &["-C", worktree_str, "add", "-A"], None).await?;
                command("git", &["-C", worktree_str, "commit", "-m", message], None).await?;
                command(
                    "git",
                    &[
                        "-C",
                        worktree_str,
                        "push",
                        "origin",
                        &format!("HEAD:refs/heads/{}", head.branch),
                    ],
                    None,
                )
                .await
            }
        }
        .map(|_| oid)
    }
    .await;
    let _ = command(
        "git",
        &[
            "-C",
            clone_str,
            "worktree",
            "remove",
            "--force",
            worktree_str,
        ],
        None,
    )
    .await;
    result
}
