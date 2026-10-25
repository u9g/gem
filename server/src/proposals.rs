use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

use crate::proto::{Comment, Patch, Proposal, PullRequestCluster};
use crate::{conversations, investigations};
use gem_github::{apply_patch, command, graphql, render, repository};

// One `<id>.json` file per proposal, relative to the server's working directory. Agents write
// them with `server propose`, which checks them first.
pub const DIR: &str = "proposals";

// The conversation or investigation a proposal answers.
fn subject(proposal: &Proposal) -> Option<&str> {
    (proposal.comment.as_ref().map(|c| c.id.as_str()))
        .or(proposal.check_failure.as_ref().map(|f| f.id.as_str()))
        .or(Some(proposal.investigation_id.as_str()).filter(|id| !id.is_empty()))
}

// Its comment, or each failed check it answers, or itself when an investigation proposed it.
// Proposals that share one are alternatives: approving one clears the others.
fn answers(proposal: &Proposal) -> Vec<&str> {
    match &proposal.check_failure {
        Some(failure) => failure.checks.iter().map(|c| c.id.as_str()).collect(),
        None if !proposal.investigation_id.is_empty() => vec![proposal.id.as_str()],
        None => subject(proposal).into_iter().collect(),
    }
}

pub async fn find(id: &str) -> Option<Proposal> {
    (load(Path::new(DIR)).await.into_iter())
        .map(|(_, proposal)| proposal)
        .find(|proposal| proposal.id == id)
}

// The proposals of each investigation, by its ID.
pub async fn of_investigations() -> HashMap<String, Vec<Proposal>> {
    let mut by_investigation: HashMap<String, Vec<Proposal>> = HashMap::new();
    for (_, proposal) in load(Path::new(DIR)).await {
        if !proposal.investigation_id.is_empty() {
            by_investigation
                .entry(proposal.investigation_id.clone())
                .or_default()
                .push(proposal);
        }
    }
    by_investigation
}

pub async fn remove_of_investigation(id: &str) {
    for (path, proposal) in load(Path::new(DIR)).await {
        if proposal.investigation_id == id {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
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

// Returns the PR's URL when approving closed it.
pub async fn approve(
    id: &str,
    reply: &str,
    follow_up: &str,
    diff: &str,
) -> Result<Option<String>, String> {
    let proposals = load(Path::new(DIR)).await;
    let Some((path, mut proposal)) = proposals.iter().find(|(_, p)| p.id == id).cloned() else {
        return Err(format!("no proposal {id}"));
    };
    let closes = proposal.close_pull_request;
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
    if let Some(patch) = proposal.patch.as_mut().filter(|_| !diff.is_empty()) {
        patch.diff = diff.to_string();
    }
    match run(&path, &mut proposal).await {
        // Nothing else on a closed PR needs answering.
        Ok(()) if closes => {
            remove_on(std::slice::from_ref(&proposal.pull_request_url)).await;
            Ok(Some(proposal.pull_request_url))
        }
        Ok(()) => {
            let answered = answers(&proposal);
            for (path, other) in &proposals {
                if answers(other).iter().any(|a| answered.contains(a)) {
                    let _ = tokio::fs::remove_file(path).await;
                }
            }
            Ok(None)
        }
        Err(error) => {
            proposal.error = error.clone();
            save(&path, &proposal).await;
            Err(error)
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

// Deletes every proposal on the PRs at `urls`, except investigations'.
pub async fn remove_on(urls: &[String]) {
    for (path, proposal) in load(Path::new(DIR)).await {
        if urls.contains(&proposal.pull_request_url) && proposal.investigation_id.is_empty() {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
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
        investigations::follow_up(
            &proposal.summary,
            &proposal.pull_request_url,
            &proposal.follow_up,
        )
        .await?;
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
    if proposal.rerun {
        let mut runs: Vec<u64> = (proposal.check_failure.iter())
            .flat_map(|f| f.checks.iter().map(|c| c.workflow_run_id))
            .filter(|&id| id != 0)
            .collect();
        runs.sort();
        runs.dedup();
        let repository = repository(&proposal.pull_request_url);
        for run in runs {
            command(
                "gh",
                &[
                    "run",
                    "rerun",
                    &run.to_string(),
                    "--failed",
                    "-R",
                    &repository,
                ],
                None,
            )
            .await?;
        }
        proposal.rerun = false;
        save(path, proposal).await;
    }
    if proposal.close_pull_request {
        command("gh", &["pr", "close", &proposal.pull_request_url], None).await?;
        proposal.close_pull_request = false;
        save(path, proposal).await;
    }
    Ok(())
}

// `server propose --dir <dir> --pr <url> (--comment <id> [--thread <id>] | --check-failure <id> |
// --investigation <id>)
// --summary <text> [--patch <diff file> --message <text>] [--reply <markdown file>]
// [--resolve-thread] [--follow-up <markdown file>] [--rerun] [--close-pull-request]
// [--excerpt <check id> <file>]... [--request-tool <markdown file>]`
pub async fn propose(args: &[String]) -> Result<String, String> {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut resolve_thread = false;
    let mut rerun = false;
    let mut close_pull_request = false;
    let mut excerpts = Vec::new();
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        if flag == "--resolve-thread" {
            resolve_thread = true;
            continue;
        }
        if flag == "--excerpt" {
            let (Some(check), Some(file)) = (args.next(), args.next()) else {
                return Err("--excerpt needs a check ID and a file".into());
            };
            excerpts.push((check, file));
            continue;
        }
        if flag == "--rerun" {
            rerun = true;
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

    let investigation_id = flags.get("investigation").copied().unwrap_or_default();
    let subjects = ["comment", "check-failure", "investigation"];
    if subjects.iter().filter(|s| flags.contains_key(*s)).count() != 1 {
        return Err("propose for one of --comment, --check-failure or --investigation".into());
    }
    let (comment, check_failure) = match (flags.get("comment"), flags.get("check-failure")) {
        (None, None) => (None, None),
        (Some(id), None) => {
            let comment = fetch_comment(id).await?;
            if comment.pull_request != pr {
                return Err(format!("comment is on {}, not {pr}", comment.pull_request));
            }
            (Some(comment.comment), None)
        }
        (None, Some(id)) => {
            let node = command(
                "gh",
                &["pr", "view", pr, "--json", "id", "--jq", ".id"],
                None,
            )
            .await?;
            let mut failure =
                conversations::check_failure(String::from_utf8_lossy(&node).trim(), id)
                    .await
                    .ok_or(format!("{id} is not the failed checks on {pr}"))?;
            for (check, file) in &excerpts {
                let check = (failure.checks.iter_mut())
                    .find(|c| c.id == **check)
                    .ok_or(format!("{check} is not one of the failed checks"))?;
                check.excerpt = read(file).await?;
            }
            failure
                .checks
                .retain(|c| excerpts.iter().any(|(check, _)| c.id == **check));
            if failure.checks.is_empty() {
                return Err("--check-failure needs --excerpt for each check it answers".into());
            }
            (None, Some(failure))
        }
        _ => unreachable!(),
    };
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
            if comment
                .as_ref()
                .is_some_and(|comment| c["id"] == comment.id.as_str())
            {
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
    let reruns = (check_failure.iter()).any(|f| f.checks.iter().any(|c| c.workflow_run_id != 0));
    if rerun && !reruns {
        return Err("--rerun needs --check-failure with a GitHub Actions run".into());
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
    let summary_html = render(pr, summary).await?;
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
        || rerun
        || close_pull_request;
    if steps == !requested_tool.trim().is_empty() {
        return Err(
            "propose either steps (--patch, --reply, --resolve-thread, --follow-up, --rerun, --close-pull-request) or --request-tool"
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
        comment,
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
        check_failure,
        rerun,
        summary_html,
        investigation_id: investigation_id.to_string(),
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
