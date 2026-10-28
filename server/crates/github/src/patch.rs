// Applying an agent's diff to a pull request's branch.

use std::{
    env,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gem_json::{Error, FromJson, Value};

use crate::command;

struct Head {
    state: String,
    branch: String,
    // `owner/name` of the head repository.
    full_name: String,
}

impl FromJson for Head {
    fn from_json(value: &Value) -> Result<Head, Error> {
        let name: String = (value["headRepository"].field("name")).map_err(|e| e.at("headRepository"))?;
        let owner: String =
            (value["headRepositoryOwner"].field("login")).map_err(|e| e.at("headRepositoryOwner"))?;
        Ok(Head {
            state: value.field("state")?,
            branch: value.field("headRefName")?,
            full_name: format!("{owner}/{name}"),
        })
    }
}

// Applies the diff to the head of the PR's branch in a fresh worktree, then commits and pushes
// when given a commit message, or only checks that it applies when not. Returns the head commit
// the diff was applied to.
pub fn apply_patch(
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
    )?;
    let head: Head = gem_json::from_slice(&head).map_err(|e| e.to_string())?;
    if head.state != "OPEN" {
        return Err(format!("{pr} is {}", head.state.to_lowercase()));
    }
    let full_name = head.full_name;
    let refspec = format!("HEAD:refs/heads/{}", head.branch);
    on_branch(&full_name, &head.branch, diff, |worktree| {
        let Some(message) = commit_message else {
            return Ok(());
        };
        commit(&worktree, message)?;
        command("git", &["-C", &worktree, "push", "origin", &refspec], None)
            .map(drop)
    })
}

// Checks that the diff applies to the head of `base` in the repository `owner/name`, and that
// `branch` is a new branch where `push_new_branch` would push it, without making a fork. Returns
// the head commit of `base`.
pub fn check_new_pull_request(
    repository: &str,
    base: &str,
    branch: &str,
    diff: &str,
) -> Result<String, String> {
    command("git", &["check-ref-format", "--branch", branch], None)?;
    let mut pushes_to = vec![repository.to_string()];
    let can_push = command(
        "gh",
        &[
            "api",
            &format!("repos/{repository}"),
            "--jq",
            ".permissions.push",
        ],
        None,
    )?;
    if String::from_utf8_lossy(&can_push).trim() != "true" {
        let login = command("gh", &["api", "user", "--jq", ".login"], None)?;
        let name = repository.split('/').nth(1).unwrap_or_default();
        pushes_to.push(format!("{}/{name}", String::from_utf8_lossy(&login).trim()));
    }
    // A branch name can hold what a URL path cannot.
    let path = (branch.replace('%', "%25"))
        .replace('#', "%23")
        .replace('?', "%3F");
    for repo in pushes_to {
        let found = command(
            "gh",
            &["api", &format!("repos/{repo}/git/ref/heads/{path}")],
            None,
        );
        match found {
            Ok(_) => return Err(format!("{repo} already has a branch {branch}")),
            Err(e) if e.contains("HTTP 404") => {}
            Err(e) => return Err(e),
        }
    }
    on_branch(repository, base, diff, |_| Ok(()))
}

// Commits the diff on a new branch off `base` in the repository `owner/name`, and pushes it to
// the repository when the user can push to it, or to their fork of it otherwise. Fails rather than
// move a branch that already exists there.
pub fn push_new_branch(
    repository: &str,
    base: &str,
    branch: &str,
    diff: &str,
    message: &str,
) -> Result<(), String> {
    let head_repository = head_repository(repository)?;
    let lease = format!("--force-with-lease=refs/heads/{branch}:");
    let refspec = format!("HEAD:refs/heads/{branch}");
    on_branch(repository, base, diff, |worktree| {
        commit(&worktree, message)?;
        if head_repository == repository {
            return command(
                "git",
                &["-C", &worktree, "push", &lease, "origin", &refspec],
                None,
            )
            .map(drop);
        }
        // gh's token pushes to the fork, whichever protocol origin was cloned with. A fork GitHub
        // has only just made can take a while to accept pushes.
        let remote = format!("https://github.com/{head_repository}.git");
        let mut attempt = 0;
        loop {
            let pushed = command(
                "git",
                &[
                    "-C",
                    &worktree,
                    "-c",
                    "credential.helper=",
                    "-c",
                    "credential.helper=!gh auth git-credential",
                    "push",
                    &lease,
                    &remote,
                    &refspec,
                ],
                None,
            );
            match pushed {
                Ok(_) => return Ok(()),
                Err(e) if attempt == 5 || e.contains("[rejected]") => return Err(e),
                Err(_) => attempt += 1,
            }
            std::thread::sleep(Duration::from_secs(5 * attempt));
        }
    })
    .map(drop)
}

// Opens a pull request into `base` of the repository `owner/name` from `branch`, which
// `push_new_branch` pushed, or finds the one an earlier try opened. Returns its URL.
pub fn open_pull_request(
    repository: &str,
    base: &str,
    branch: &str,
    title: &str,
    body: &str,
) -> Result<String, String> {
    let head_repository = head_repository(repository)?;
    let owner = head_repository.split('/').next().unwrap_or_default();
    let head = format!("head={owner}:{branch}");
    let base = format!("base={base}");
    let pulls = format!("repos/{repository}/pulls");
    let opened = command(
        "gh",
        &[
            "api",
            "-X",
            "GET",
            &pulls,
            "-f",
            &head,
            "-f",
            &base,
            "--jq",
            ".[0].html_url // empty",
        ],
        None,
    )?;
    let opened = String::from_utf8_lossy(&opened).trim().to_string();
    if !opened.is_empty() {
        return Ok(opened);
    }
    let url = command(
        "gh",
        &[
            "api",
            &pulls,
            "-f",
            &format!("title={title}"),
            "-f",
            &head,
            "-f",
            &base,
            "-F",
            "body=@-",
            "--jq",
            ".html_url",
        ],
        Some(body),
    )?;
    Ok(String::from_utf8_lossy(&url).trim().to_string())
}

// The repository a new branch for the repository `owner/name` goes to: that one when the user can
// push to it, and their fork of it otherwise, which GitHub makes on first use and returns after.
fn head_repository(repository: &str) -> Result<String, String> {
    let can_push = command(
        "gh",
        &[
            "api",
            &format!("repos/{repository}"),
            "--jq",
            ".permissions.push",
        ],
        None,
    )?;
    if String::from_utf8_lossy(&can_push).trim() == "true" {
        return Ok(repository.to_string());
    }
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
    )?;
    Ok(String::from_utf8_lossy(&fork).trim().to_string())
}

fn commit(worktree: &str, message: &str) -> Result<(), String> {
    command("git", &["-C", worktree, "add", "-A"], None)?;
    command("git", &["-C", worktree, "commit", "-m", message], None)?;
    Ok(())
}

// Applies the diff to the head of `branch` of the repository `owner/name` in a fresh worktree, or
// only checks that it applies when `work` does nothing, then runs `work` in it. Returns the head
// commit the diff was applied to. Each repository is cloned once and kept outside any T3 Code
// project.
fn on_branch<W>(
    full_name: &str,
    branch: &str,
    diff: &str,
    work: W,
) -> Result<String, String>
where
    W: FnOnce(String) -> Result<(), String>,
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
        )?;
    }
    command("git", &["-C", clone_str, "fetch", "origin", branch], None)?;

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
    )?;
    let result = (|| {
        let oid = command("git", &["-C", worktree_str, "rev-parse", "HEAD"], None)?;
        let oid = String::from_utf8_lossy(&oid).trim().to_string();
        command(
            "git",
            &["-C", worktree_str, "apply", "--check", "-"],
            Some(diff),
        )?;
        command("git", &["-C", worktree_str, "apply", "-"], Some(diff))?;
        work(worktree_str.to_string())?;
        Ok(oid)
    })();
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
    );
    result
}
