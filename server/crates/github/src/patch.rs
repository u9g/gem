// Applying an agent's diff to a pull request's branch.

use std::{
    env,
    future::Future,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

use crate::command;

#[derive(Deserialize)]
struct Head {
    state: String,
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
// the diff was applied to.
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
            "state,headRefName,headRepository,headRepositoryOwner",
        ],
        None,
    )
    .await?;
    let head: Head = serde_json::from_slice(&head).map_err(|e| e.to_string())?;
    if head.state != "OPEN" {
        return Err(format!("{pr} is {}", head.state.to_lowercase()));
    }
    let full_name = format!("{}/{}", head.owner.login, head.repository.name);
    let refspec = format!("HEAD:refs/heads/{}", head.branch);
    on_branch(&full_name, &head.branch, diff, |worktree| async move {
        let Some(message) = commit_message else {
            return Ok(());
        };
        commit(&worktree, message).await?;
        command("git", &["-C", &worktree, "push", "origin", &refspec], None)
            .await
            .map(drop)
    })
    .await
}

// Checks that the diff applies to the head of `base` in the repository `owner/name`. Returns that
// head commit.
pub async fn check_new_pull_request(
    repository: &str,
    base: &str,
    diff: &str,
) -> Result<String, String> {
    on_branch(repository, base, diff, |_| async { Ok(()) }).await
}

// Commits the diff on a new branch off `base` in the repository `owner/name` and opens a pull
// request from it. The branch goes to the repository when the user can push to it, and to their
// fork of it otherwise, which GitHub makes on first use. Returns the pull request's URL.
pub async fn open_pull_request(
    repository: &str,
    base: &str,
    branch: &str,
    diff: &str,
    message: &str,
    title: &str,
    body: &str,
) -> Result<String, String> {
    let can_push = command(
        "gh",
        &[
            "api",
            &format!("repos/{repository}"),
            "--jq",
            ".permissions.push",
        ],
        None,
    )
    .await?;
    let head_repository = if String::from_utf8_lossy(&can_push).trim() == "true" {
        repository.to_string()
    } else {
        let fork = command(
            "gh",
            &[
                "api",
                "-X",
                "POST",
                &format!("repos/{repository}/forks"),
                "--jq",
                ".full_name",
            ],
            None,
        )
        .await?;
        String::from_utf8_lossy(&fork).trim().to_string()
    };
    let remote = format!("https://github.com/{head_repository}.git");
    let refspec = format!("HEAD:refs/heads/{branch}");
    on_branch(repository, base, diff, |worktree| async move {
        commit(&worktree, message).await?;
        command("git", &["-C", &worktree, "push", &remote, &refspec], None)
            .await
            .map(drop)
    })
    .await?;
    let owner = head_repository.split('/').next().unwrap_or_default();
    let url = command(
        "gh",
        &[
            "api",
            &format!("repos/{repository}/pulls"),
            "-f",
            &format!("title={title}"),
            "-f",
            &format!("head={owner}:{branch}"),
            "-f",
            &format!("base={base}"),
            "-F",
            "body=@-",
            "--jq",
            ".html_url",
        ],
        Some(body),
    )
    .await?;
    Ok(String::from_utf8_lossy(&url).trim().to_string())
}

async fn commit(worktree: &str, message: &str) -> Result<(), String> {
    command("git", &["-C", worktree, "add", "-A"], None).await?;
    command("git", &["-C", worktree, "commit", "-m", message], None).await?;
    Ok(())
}

// Applies the diff to the head of `branch` of the repository `owner/name` in a fresh worktree, or
// only checks that it applies when `work` does nothing, then runs `work` in it. Returns the head
// commit the diff was applied to. Each repository is cloned once and kept outside any T3 Code
// project.
async fn on_branch<F, W>(
    full_name: &str,
    branch: &str,
    diff: &str,
    work: W,
) -> Result<String, String>
where
    W: FnOnce(String) -> F,
    F: Future<Output = Result<(), String>>,
{
    let checkouts = env::temp_dir().join("gem-checkouts");
    let clone = checkouts.join(full_name);
    let clone_str = clone.to_str().unwrap();
    if !clone.exists() {
        command(
            "gh",
            &[
                "repo",
                "clone",
                full_name,
                clone_str,
                "--",
                "--filter=blob:none",
                "--no-checkout",
            ],
            None,
        )
        .await?;
    }
    command("git", &["-C", clone_str, "fetch", "origin", branch], None).await?;

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
            &format!("origin/{branch}"),
        ],
        None,
    )
    .await?;
    let result = async {
        let oid = command("git", &["-C", worktree_str, "rev-parse", "HEAD"], None).await?;
        let oid = String::from_utf8_lossy(&oid).trim().to_string();
        command(
            "git",
            &["-C", worktree_str, "apply", "--check", "-"],
            Some(diff),
        )
        .await?;
        command("git", &["-C", worktree_str, "apply", "-"], Some(diff)).await?;
        work(worktree_str.to_string()).await?;
        Ok(oid)
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
