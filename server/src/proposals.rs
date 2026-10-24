use std::{
    collections::{HashMap, HashSet},
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use tokio::{io::AsyncWriteExt, process::Command};

use crate::proto::{Comment, Patch, Proposal, PullRequestCluster};
use crate::t3;

// One `<id>.json` file per proposal, relative to the server's working directory. Agents write
// them with `server propose`, which checks them first.
pub const DIR: &str = "proposals";

// The conversation a proposal answers. Proposals answering the same one are alternatives.
fn subject(proposal: &Proposal) -> Option<&str> {
    proposal.comment.as_ref().map(|c| c.id.as_str())
}

pub async fn fill(clusters: &mut [PullRequestCluster], waiting: &HashSet<String>) {
    let mut by_pr: HashMap<String, Vec<Proposal>> = HashMap::new();
    for (_, proposal) in load(Path::new(DIR)).await {
        if subject(&proposal).is_some_and(|id| waiting.contains(id)) {
            by_pr
                .entry(proposal.pull_request_url.clone())
                .or_default()
                .push(proposal);
        }
    }
    for pr in clusters.iter_mut().flat_map(|c| c.pull_requests.iter_mut()) {
        pr.proposals = by_pr.remove(&pr.url).unwrap_or_default();
    }
}

pub async fn approve(id: &str, reply: &str, follow_up: &str) {
    let proposals = load(Path::new(DIR)).await;
    let Some((path, mut proposal)) = proposals.iter().find(|(_, p)| p.id == id).cloned() else {
        return;
    };
    if !reply.is_empty() && !proposal.reply.is_empty() {
        proposal.reply = reply.to_string();
        if let Ok(html) = render(&proposal.pull_request_url, reply).await {
            proposal.reply_html = html;
        }
    }
    if !follow_up.is_empty() && !proposal.follow_up.is_empty() {
        proposal.follow_up = follow_up.to_string();
        if let Ok(html) = render(&proposal.pull_request_url, follow_up).await {
            proposal.follow_up_html = html;
        }
    }
    match run(&path, &mut proposal).await {
        // The other proposals answered the same conversation.
        Ok(()) => {
            for (path, other) in &proposals {
                if subject(other) == subject(&proposal) {
                    let _ = tokio::fs::remove_file(path).await;
                }
            }
        }
        Err(error) => {
            proposal.error = error;
            save(&path, &proposal).await;
        }
    }
}

// Deletes every proposal answering a conversation whose patch was written against a head commit its
// branch has since moved past, keyed by PR URL in `heads`. Returns those conversations' IDs, so
// they can be answered again against the new head.
pub async fn remove_stale(heads: &HashMap<&str, &str>) -> HashSet<String> {
    let proposals = load(Path::new(DIR)).await;
    let stale: HashSet<String> = proposals
        .iter()
        .filter(|(_, p)| {
            p.patch.as_ref().is_some_and(|patch| {
                !patch.head_oid.is_empty()
                    && heads
                        .get(p.pull_request_url.as_str())
                        .is_some_and(|head| *head != patch.head_oid)
            })
        })
        .filter_map(|(_, p)| subject(p).map(str::to_string))
        .collect();
    for (path, proposal) in &proposals {
        if subject(proposal).is_some_and(|id| stale.contains(id)) {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
    stale
}

pub async fn dismiss(id: &str) {
    for (path, proposal) in load(Path::new(DIR)).await {
        if proposal.id == id {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

async fn run(path: &Path, proposal: &mut Proposal) -> Result<(), String> {
    if !proposal.follow_up.is_empty() {
        t3::start(&proposal.summary, &proposal.follow_up).await?;
        proposal.follow_up.clear();
        save(path, proposal).await;
    }
    if let Some(patch) = &proposal.patch {
        apply_patch(
            &proposal.pull_request_url,
            &patch.diff,
            Some(&patch.commit_message),
        )
        .await?;
        proposal.patch = None;
        save(path, proposal).await;
    }
    if !proposal.reply.is_empty() {
        if proposal.thread_id.is_empty() {
            let quoted: String = (proposal.comment.as_ref().map_or("", |c| c.body.as_str()))
                .lines()
                .map(|line| format!("> {line}\n"))
                .collect();
            let body = format!("{quoted}\n{}", proposal.reply);
            command(
                "gh",
                &["pr", "comment", &proposal.pull_request_url, "--body", &body],
                None,
            )
            .await?;
        } else {
            graphql(
                "mutation($thread: ID!, $body: String!) { addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $thread, body: $body}) { comment { id } } }",
                &[("thread", &proposal.thread_id), ("body", &proposal.reply)],
            )
            .await?;
        }
        proposal.reply.clear();
        save(path, proposal).await;
    }
    if proposal.resolve_thread {
        graphql(
            "mutation($thread: ID!) { resolveReviewThread(input: {threadId: $thread}) { thread { id } } }",
            &[("thread", &proposal.thread_id)],
        )
        .await?;
        proposal.resolve_thread = false;
        save(path, proposal).await;
    }
    if proposal.close_pull_request {
        command("gh", &["pr", "close", &proposal.pull_request_url], None).await?;
        proposal.close_pull_request = false;
        save(path, proposal).await;
    }
    Ok(())
}

// `server propose --dir <dir> --pr <url> --comment <id> [--thread <id>] --summary <text>
// [--patch <diff file> --message <text>] [--reply <markdown file>] [--resolve-thread]
// [--follow-up <markdown file>] [--close-pull-request]
// [--request-tool <markdown file>]`
pub async fn propose(args: &[String]) -> Result<String, String> {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut resolve_thread = false;
    let mut close_pull_request = false;
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        if flag == "--resolve-thread" {
            resolve_thread = true;
            continue;
        }
        if flag == "--close-pull-request" {
            close_pull_request = true;
            continue;
        }
        let name = flag
            .strip_prefix("--")
            .ok_or(format!("unexpected argument {flag}"))?;
        let value = args.next().ok_or(format!("{flag} needs a value"))?;
        flags.insert(name, value);
    }
    let required = |name: &str| {
        flags
            .get(name)
            .copied()
            .ok_or(format!("--{name} is required"))
    };
    let dir = PathBuf::from(required("dir")?);
    let pr = required("pr")?;
    let summary = required("summary")?;
    let thread_id = flags.get("thread").copied().unwrap_or_default();

    let comment = fetch_comment(required("comment")?).await?;
    if comment.pull_request != pr {
        return Err(format!("comment is on {}, not {pr}", comment.pull_request));
    }
    let mut earlier_comments = Vec::new();
    if !thread_id.is_empty() {
        let value: serde_json::Value = serde_json::from_slice(
            &graphql(
                "query($id: ID!) { node(id: $id) { ... on PullRequestReviewThread { pullRequest { url } comments(first: 100) { nodes { id body bodyHTML url author { login } } } } } }",
                &[("id", thread_id)],
            )
            .await?,
        )
        .map_err(|e| e.to_string())?;
        let node = &value["data"]["node"];
        let on = node["pullRequest"]["url"]
            .as_str()
            .ok_or(format!("{thread_id} is not a review thread"))?;
        if on != pr {
            return Err(format!("thread is on {on}, not {pr}"));
        }
        for c in node["comments"]["nodes"].as_array().into_iter().flatten() {
            if c["id"] == comment.comment.id.as_str() {
                break;
            }
            earlier_comments.push(Comment {
                id: c["id"].as_str().unwrap_or_default().to_string(),
                author: c["author"]["login"].as_str().unwrap_or("ghost").to_string(),
                body: c["body"].as_str().unwrap_or_default().to_string(),
                url: c["url"].as_str().unwrap_or_default().to_string(),
                body_html: c["bodyHTML"].as_str().unwrap_or_default().to_string(),
            });
        }
    }
    if resolve_thread && thread_id.is_empty() {
        return Err("--resolve-thread needs --thread".into());
    }

    let patch = match flags.get("patch") {
        Some(file) => {
            let diff = read(file).await?;
            let head_oid = apply_patch(pr, &diff, None).await?;
            let commit_message = required("message")?.to_string();
            Some(Patch {
                diff,
                commit_message,
                head_oid,
            })
        }
        None => None,
    };
    let reply = match flags.get("reply") {
        Some(file) => read(file).await?,
        None => String::new(),
    };
    let reply_html = render(pr, &reply).await?;
    let follow_up = match flags.get("follow-up") {
        Some(file) => read(file).await?,
        None => String::new(),
    };
    let follow_up_html = render(pr, &follow_up).await?;
    let requested_tool = match flags.get("request-tool") {
        Some(file) => read(file).await?,
        None => String::new(),
    };
    let steps = patch.is_some()
        || !reply.trim().is_empty()
        || resolve_thread
        || !follow_up.trim().is_empty()
        || close_pull_request;
    if steps == !requested_tool.trim().is_empty() {
        return Err(
            "propose either steps (--patch, --reply, --resolve-thread, --follow-up, --close-pull-request) or --request-tool"
                .into(),
        );
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let proposal = Proposal {
        id: format!("{nanos:x}"),
        summary: summary.to_string(),
        pull_request_url: pr.to_string(),
        comment: Some(comment.comment),
        thread_id: thread_id.to_string(),
        patch,
        reply,
        resolve_thread,
        error: String::new(),
        requested_tool,
        earlier_comments,
        reply_html,
        follow_up,
        follow_up_html,
        close_pull_request,
    };
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    save(&dir.join(format!("{}.json", proposal.id)), &proposal).await;
    Ok(proposal.id)
}

struct FetchedComment {
    comment: Comment,
    pull_request: String,
}

async fn fetch_comment(id: &str) -> Result<FetchedComment, String> {
    #[derive(Deserialize)]
    struct Node {
        id: String,
        body: String,
        #[serde(rename = "bodyHTML")]
        body_html: String,
        url: String,
        author: Option<Author>,
        #[serde(rename = "pullRequest")]
        pull_request: Option<Url>,
    }
    #[derive(Deserialize)]
    struct Author {
        login: String,
    }
    #[derive(Deserialize)]
    struct Url {
        url: String,
    }
    let fields = "id body bodyHTML url author { login } pullRequest { url }";
    let output = graphql(
        &format!("query($id: ID!) {{ node(id: $id) {{ ... on IssueComment {{ {fields} }} ... on PullRequestReviewComment {{ {fields} }} }} }}"),
        &[("id", id)],
    )
    .await?;
    let value: serde_json::Value = serde_json::from_slice(&output).map_err(|e| e.to_string())?;
    let node: Node = serde_json::from_value(value["data"]["node"].clone())
        .map_err(|_| format!("{id} is not a comment on a pull request"))?;
    Ok(FetchedComment {
        pull_request: node.pull_request.map(|p| p.url).unwrap_or_default(),
        comment: Comment {
            id: node.id,
            author: node.author.map_or("ghost".into(), |a| a.login),
            body: node.body,
            url: node.url,
            body_html: node.body_html,
        },
    })
}

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
async fn apply_patch(pr: &str, diff: &str, commit_message: Option<&str>) -> Result<String, String> {
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

// Markdown as GitHub renders it in a comment on the pull request at `pr`.
async fn render(pr: &str, markdown: &str) -> Result<String, String> {
    if markdown.is_empty() {
        return Ok(String::new());
    }
    let repository: Vec<&str> = pr
        .trim_start_matches("https://github.com/")
        .split('/')
        .take(2)
        .collect();
    let context = format!("context={}", repository.join("/"));
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

async fn graphql(query: &str, variables: &[(&str, &str)]) -> Result<Vec<u8>, String> {
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
async fn command(program: &str, args: &[&str], stdin: Option<&str>) -> Result<Vec<u8>, String> {
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

async fn read(file: &str) -> Result<String, String> {
    tokio::fs::read_to_string(file)
        .await
        .map_err(|e| format!("{file}: {e}"))
}

async fn load(dir: &Path) -> Vec<(PathBuf, Proposal)> {
    let mut proposals = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return proposals;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if let Ok(bytes) = tokio::fs::read(&path).await
            && let Ok(proposal) = serde_json::from_slice(&bytes)
        {
            proposals.push((path, proposal));
        }
    }
    proposals
}

async fn save(path: &Path, proposal: &Proposal) {
    let _ = tokio::fs::write(path, serde_json::to_vec_pretty(proposal).unwrap()).await;
}
