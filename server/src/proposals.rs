use std::{
    collections::{HashMap, HashSet},
    future::Future,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use gem_db::{Db, map_mut, texts};
use serde::Deserialize;
use tracing::Instrument;

use crate::proto::{CheckFailure, Comment, NewPullRequest, Patch, Proposal, PullRequestCluster};
use crate::tokens::Subject;
use crate::{conversations, investigations};
use gem_github::{
    apply_patch, check_new_pull_request, command, graphql, open_pull_request, push_new_branch,
    render, repository,
};

// Agents make proposals with `gem.propose`, in scripts the `gem-agent` MCP server runs, which
// checks them first.

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
    load(crate::db()).into_iter().find(|proposal| proposal.id == id)
}

// Whether a proposal answers the conversation or investigation `id`.
pub async fn exist_for(id: &str) -> bool {
    (load(crate::db()).iter()).any(|proposal| subject(proposal) == Some(id))
}

// The proposals of each investigation, by its ID.
pub async fn of_investigations() -> HashMap<String, Vec<Proposal>> {
    let mut by_investigation: HashMap<String, Vec<Proposal>> = HashMap::new();
    for proposal in load(crate::db()) {
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
    for proposal in load(crate::db()) {
        if proposal.investigation_id == id {
            delete(&proposal.id);
        }
    }
}

// A proposal's review comment takes where it now sits from `waiting`, so a push since the
// proposal was made moves or outdates it.
pub async fn fill(clusters: &mut [PullRequestCluster], waiting: &HashMap<String, Option<Comment>>) {
    let mut by_pr: HashMap<String, Vec<Proposal>> = HashMap::new();
    for mut proposal in load(crate::db()) {
        let Some(current) = subject(&proposal).and_then(|id| waiting.get(id)) else {
            continue;
        };
        if let (Some(comment), Some(current)) = (&mut proposal.comment, current) {
            comment.line = current.line;
            comment.diff_hunk = current.diff_hunk.clone();
        }
        by_pr
            .entry(proposal.pull_request_url.clone())
            .or_default()
            .push(proposal);
    }
    for pr in clusters.iter_mut().flat_map(|c| c.pull_requests.iter_mut()) {
        pr.proposals = by_pr.remove(&pr.url).unwrap_or_default();
    }
}

// Returns the PR's URL when approving closed it.
// Everything logged while approving carries the proposal's ID and its PR.
pub async fn approve(
    id: &str,
    reply: &str,
    follow_up: &str,
    diff: &str,
) -> Result<Option<String>, String> {
    let span = tracing::info_span!(
        "approve",
        proposal = id,
        pull_request = tracing::field::Empty
    );
    approve_proposal(id, reply, follow_up, diff)
        .instrument(span)
        .await
}

async fn approve_proposal(
    id: &str,
    reply: &str,
    follow_up: &str,
    diff: &str,
) -> Result<Option<String>, String> {
    let proposals = load(crate::db());
    let Some(mut proposal) = proposals.iter().find(|p| p.id == id).cloned() else {
        return Err(format!("no proposal {id}"));
    };
    tracing::Span::current().record("pull_request", proposal.pull_request_url.as_str());
    if proposal.error == STALE {
        return Err(STALE.to_string());
    }
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
    match run(&mut proposal).await {
        // Nothing else on a closed PR needs answering.
        Ok(()) if closes => {
            remove_on(std::slice::from_ref(&proposal.pull_request_url)).await;
            Ok(Some(proposal.pull_request_url))
        }
        Ok(()) => {
            let answered = answers(&proposal);
            for other in &load(crate::db()) {
                if answers(other).iter().any(|a| answered.contains(a)) {
                    delete(&other.id);
                }
            }
            Ok(None)
        }
        Err(error) => {
            proposal.error = error.clone();
            save(crate::db(), &proposal);
            Err(error)
        }
    }
}

// Why a proposal answering a comment waits on its agent: set once its patches went stale.
pub const STALE: &str = "Its patch was made against an older head; the agent is updating it.";

// A proposal is stale once it has a patch written against a head commit its branch has since moved
// past, or for another pull request no longer open, keyed by open PR URL in `heads`. Failed checks
// are answered again against the new heads, so every proposal answering them goes, and their IDs
// are returned. A stale proposal answering a comment stays, marked STALE, for its cluster's agent
// to replace: the IDs of the comments with ones newly marked are returned too. An investigation's
// proposals are independent, so only its stale ones go.
pub async fn remove_stale(heads: &HashMap<&str, &str>) -> HashSet<String> {
    let moved = |pr: &str, patch: &Patch| {
        !patch.head_oid.is_empty() && heads.get(pr).is_some_and(|head| *head != patch.head_oid)
    };
    let proposals = load(crate::db());
    let stale: Vec<&Proposal> = (proposals.iter())
        .filter(|p| {
            p.patch
                .as_ref()
                .is_some_and(|patch| moved(&p.pull_request_url, patch))
                || p.other_patches.iter().any(|patch| {
                    !heads.contains_key(patch.pull_request_url.as_str())
                        || moved(&patch.pull_request_url, patch)
                })
        })
        .collect();
    let mut again: HashSet<String> = (stale.iter())
        .filter_map(|p| p.check_failure.as_ref().map(|f| f.id.clone()))
        .collect();
    for proposal in &proposals {
        let answers_stale =
            (proposal.check_failure.as_ref()).is_some_and(|f| again.contains(&f.id));
        let is_stale = stale.iter().any(|p| p.id == proposal.id);
        if let Some(comment) = &proposal.comment {
            if is_stale && proposal.error != STALE {
                let mut marked = proposal.clone();
                marked.error = STALE.to_string();
                save(crate::db(), &marked);
                again.insert(comment.id.clone());
            }
        } else if answers_stale || is_stale {
            delete(&proposal.id);
        }
    }
    again
}

// Deletes every proposal on the PRs at `urls`, except investigations'.
pub async fn remove_on(urls: &[String]) {
    for proposal in load(crate::db()) {
        if urls.contains(&proposal.pull_request_url) && proposal.investigation_id.is_empty() {
            delete(&proposal.id);
        }
    }
}

pub async fn dismiss(id: &str) {
    delete(id);
}

// Logs a step of an approve starting and how it ended. A step that succeeded is saved as done
// before the next starts, so the log shows how far a failed approve got.
async fn step<T>(
    name: &'static str,
    work: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tracing::info!(step = name, "step started");
    let started = Instant::now();
    let result = work.await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    match &result {
        Ok(_) => tracing::info!(step = name, elapsed_ms, "step done"),
        Err(error) => tracing::warn!(step = name, elapsed_ms, %error, "step failed"),
    }
    result
}

async fn run(proposal: &mut Proposal) -> Result<(), String> {
    if !proposal.follow_up.is_empty() {
        step(
            "follow_up",
            investigations::follow_up(
                &proposal.summary,
                &proposal.pull_request_url,
                &proposal.follow_up,
            ),
        )
        .await?;
        proposal.follow_up.clear();
        save(crate::db(), proposal);
    }
    if let Some(patch) = &proposal.patch {
        step(
            "patch",
            apply_patch(
                &proposal.pull_request_url,
                &patch.diff,
                Some(&patch.commit_message),
            ),
        )
        .await?;
        proposal.patch = None;
        save(crate::db(), proposal);
    }
    while let Some(patch) = proposal.other_patches.first() {
        step(
            "other_patch",
            apply_patch(
                &patch.pull_request_url,
                &patch.diff,
                Some(&patch.commit_message),
            ),
        )
        .instrument(tracing::info_span!("other", pull_request = %patch.pull_request_url))
        .await?;
        proposal.other_patches.remove(0);
        save(crate::db(), proposal);
    }
    // The pushed branch's patch is dropped, so a retry opens the pull request without pushing
    // again.
    if let Some(new) = &proposal.open_pull_request
        && let Some(patch) = &new.patch
    {
        step(
            "push_new_branch",
            push_new_branch(
                &new.repository,
                &new.base,
                &new.branch,
                &patch.diff,
                &patch.commit_message,
            ),
        )
        .instrument(tracing::info_span!("new", repository = %new.repository, branch = %new.branch))
        .await?;
        if let Some(new) = &mut proposal.open_pull_request {
            new.patch = None;
        }
        save(crate::db(), proposal);
    }
    if let Some(new) = &proposal.open_pull_request {
        let url = step(
            "open_pull_request",
            open_pull_request(
                &new.repository,
                &new.base,
                &new.branch,
                &new.title,
                &new.body,
            ),
        )
        .instrument(tracing::info_span!("new", repository = %new.repository, branch = %new.branch))
        .await?;
        tracing::info!(%url, "pull request opened");
        proposal.open_pull_request = None;
        save(crate::db(), proposal);
    }
    if !proposal.reply.is_empty() {
        if proposal.thread_id.is_empty() {
            step(
                "comment",
                command(
                    "gh",
                    &[
                        "pr",
                        "comment",
                        &proposal.pull_request_url,
                        "--body",
                        &proposal.reply,
                    ],
                    None,
                ),
            )
            .await?;
        } else {
            step(
                "reply",
                graphql(
                    "mutation($thread: ID!, $body: String!) { addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $thread, body: $body}) { comment { id } } }",
                    &[("thread", &proposal.thread_id), ("body", &proposal.reply)],
                ),
            )
            .await?;
        }
        proposal.reply.clear();
        save(crate::db(), proposal);
    }
    if proposal.resolve_thread {
        step(
            "resolve_thread",
            graphql(
                "mutation($thread: ID!) { resolveReviewThread(input: {threadId: $thread}) { thread { id } } }",
                &[("thread", &proposal.thread_id)],
            ),
        )
        .await?;
        proposal.resolve_thread = false;
        save(crate::db(), proposal);
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
            step(
                "rerun",
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
                ),
            )
            .instrument(tracing::info_span!("workflow_run", id = run))
            .await?;
        }
        proposal.rerun = false;
        save(crate::db(), proposal);
    }
    if proposal.close_pull_request {
        step(
            "close",
            command("gh", &["pr", "close", &proposal.pull_request_url], None),
        )
        .await?;
        proposal.close_pull_request = false;
        save(crate::db(), proposal);
    }
    Ok(())
}

// What an agent proposes, as `gem.propose` of the `gem-agent` MCP server takes it. The
// subject it answers comes from the agent's token, not from here.
#[derive(Deserialize, Default, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
    pub pr: String,
    pub summary: String,
    // A diff, and the message to commit it with.
    pub patch: String,
    pub message: String,
    pub other_patches: Vec<OtherPatch>,
    pub open_pull_request: Option<OpenPullRequest>,
    // Markdown, as are the follow-up and the requested tool.
    pub reply: String,
    pub follow_up: String,
    pub resolve_thread: bool,
    pub rerun: bool,
    pub close_pull_request: bool,
    pub excerpts: Vec<Excerpt>,
    pub request_tool: String,
    // The comment it answers, and the review thread holding it, for an agent answering a cluster.
    pub comment: String,
    pub thread: String,
    // The ID of an earlier proposal of the cluster's that this one replaces.
    pub replaces: String,
}

// A diff for another pull request of the cluster, and the message to commit it with.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct OtherPatch {
    pub pr: String,
    pub patch: String,
    pub message: String,
}

// A pull request to open: a diff against `base` of `repository`, committed on `branch`.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct OpenPullRequest {
    pub repository: String,
    pub base: String,
    pub branch: String,
    pub title: String,
    // Markdown.
    pub body: String,
    pub patch: String,
    pub message: String,
}

// The log lines that show why a failed check failed.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Excerpt {
    pub check: String,
    pub lines: String,
}

// What checking a proposal asks of GitHub, so tests can answer without it.
pub trait GitHub: Send + Sync + 'static {
    fn comment(&self, id: &str) -> impl Future<Output = Result<FetchedComment, String>> + Send;
    // The failed checks on the PR at `pr` while `id` is still their ID.
    fn check_failure(
        &self,
        pr: &str,
        id: &str,
    ) -> impl Future<Output = Result<CheckFailure, String>> + Send;
    // The review thread's PR URL and its comments, oldest first.
    fn review_thread(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<(String, Vec<Comment>), String>> + Send;
    // Checks that the diff applies to the PR's head. Returns that head commit.
    fn check_patch(
        &self,
        pr: &str,
        diff: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;
    // Checks that the diff applies to the head of `base` in the repository `owner/name`, and that
    // `branch` is new. Returns that head commit.
    fn check_new_pull_request(
        &self,
        repository: &str,
        base: &str,
        branch: &str,
        diff: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;
    fn render(
        &self,
        pr: &str,
        markdown: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;
}

pub struct Live;

impl GitHub for Live {
    async fn comment(&self, id: &str) -> Result<FetchedComment, String> {
        fetch_comment(id).await
    }

    async fn check_failure(&self, pr: &str, id: &str) -> Result<CheckFailure, String> {
        let node = command(
            "gh",
            &["pr", "view", pr, "--json", "id", "--jq", ".id"],
            None,
        )
        .await?;
        conversations::check_failure(String::from_utf8_lossy(&node).trim(), id)
            .await
            .ok_or(format!("{id} is not the failed checks on {pr}"))
    }

    async fn review_thread(&self, id: &str) -> Result<(String, Vec<Comment>), String> {
        let value: serde_json::Value = serde_json::from_slice(
            &graphql(
                "query($id: ID!) { node(id: $id) { ... on PullRequestReviewThread { pullRequest { url } comments(first: 100) { nodes { id body bodyHTML url author { login } } } } } }",
                &[("id", id)],
            )
            .await?,
        )
        .map_err(|e| e.to_string())?;
        let node = &value["data"]["node"];
        let pr = node["pullRequest"]["url"]
            .as_str()
            .ok_or(format!("{id} is not a review thread"))?;
        let comments = (node["comments"]["nodes"].as_array().into_iter().flatten())
            .map(|c| Comment {
                id: c["id"].as_str().unwrap_or_default().to_string(),
                author: c["author"]["login"].as_str().unwrap_or("ghost").to_string(),
                body: c["body"].as_str().unwrap_or_default().to_string(),
                url: c["url"].as_str().unwrap_or_default().to_string(),
                body_html: c["bodyHTML"].as_str().unwrap_or_default().to_string(),
                ..Default::default()
            })
            .collect();
        Ok((pr.to_string(), comments))
    }

    async fn check_patch(&self, pr: &str, diff: &str) -> Result<String, String> {
        apply_patch(pr, diff, None).await
    }

    async fn check_new_pull_request(
        &self,
        repository: &str,
        base: &str,
        branch: &str,
        diff: &str,
    ) -> Result<String, String> {
        check_new_pull_request(repository, base, branch, diff).await
    }

    async fn render(&self, pr: &str, markdown: &str) -> Result<String, String> {
        render(pr, markdown).await
    }
}

// Checks what an agent answering `subject` proposes and saves it in `db`. `cluster` is the URLs
// of the pull requests in the cluster of the one it is about. Returns the proposal's ID, or why it
// cannot be approved, for the agent to fix and propose again.
pub async fn propose(
    github: &impl GitHub,
    db: &Db,
    subject: &Subject,
    cluster: &[String],
    request: Request,
) -> Result<String, String> {
    let Request {
        pr,
        summary,
        patch,
        message,
        other_patches,
        open_pull_request,
        reply,
        follow_up,
        resolve_thread,
        rerun,
        close_pull_request,
        excerpts,
        request_tool,
        comment,
        thread,
        replaces,
    } = request;
    let pr = pr.trim().trim_end_matches('/');
    if pr.is_empty() {
        return Err("`pr` is required: the URL of the pull request the proposal is about".into());
    }
    if summary.trim().is_empty() {
        return Err("`summary` is required".into());
    }
    let answers_on = match subject {
        Subject::Cluster { pr: key } => key == pr || cluster.contains(key),
        _ => subject.prs().iter().any(|p| p == pr),
    };
    if !answers_on {
        return Err(format!(
            "{pr} is not a pull request this thread answers on; propose on {}",
            match subject {
                Subject::Cluster { pr } => format!("a pull request of the cluster holding {pr}"),
                _ => subject.prs().join(" or "),
            }
        ));
    }
    let replaced = match replaces.trim() {
        "" => None,
        id => {
            let replaced = (load(db).into_iter())
                .find(|p| p.id == id && p.comment.is_some())
                .ok_or(format!("no proposal {id} answering a comment to replace"))?;
            let on = &replaced.pull_request_url;
            if !matches!(subject, Subject::Cluster { .. }) || !(on == pr || cluster.contains(on)) {
                return Err(format!("proposal {id} is not one of this cluster's"));
            }
            let answered = replaced.comment.as_ref().map(|c| c.id.as_str());
            if answered != Some(comment.trim()) {
                return Err(format!(
                    "proposal {id} answers {}, so its replacement must too",
                    answered.unwrap_or_default()
                ));
            }
            Some(replaced.id)
        }
    };

    let (comment, check_failure, thread_id, investigation_id) = match subject {
        Subject::Cluster { .. } => {
            if comment.trim().is_empty() {
                return Err("`comment` is required: the ID of the comment it answers".into());
            }
            let comment = github.comment(comment.trim()).await?;
            if comment.pull_request != pr {
                return Err(format!("comment is on {}, not {pr}", comment.pull_request));
            }
            let thread = Some(thread.trim().to_string()).filter(|t| !t.is_empty());
            if thread.is_none() && !comment.comment.path.is_empty() {
                return Err("`thread` is required: the review thread holding the comment".into());
            }
            (Some(comment.comment), None, thread, "")
        }
        Subject::CheckFailure { id, .. } => {
            let mut failure = github.check_failure(pr, id).await?;
            for excerpt in &excerpts {
                let check = (failure.checks.iter_mut())
                    .find(|c| c.id == excerpt.check)
                    .ok_or(format!("{} is not one of the failed checks", excerpt.check))?;
                check.excerpt = excerpt.lines.clone();
            }
            failure
                .checks
                .retain(|c| excerpts.iter().any(|excerpt| c.id == excerpt.check));
            if failure.checks.is_empty() {
                return Err(
                    "`excerpts` needs an excerpt for each check the proposal answers".into(),
                );
            }
            (None, Some(failure), None, "")
        }
        Subject::Investigation { id, .. } => (None, None, None, id.as_str()),
    };
    if !excerpts.is_empty() && check_failure.is_none() {
        return Err("`excerpts` is only for proposals that answer failed checks".into());
    }
    let thread_id = thread_id.unwrap_or_default();
    let mut earlier_comments = Vec::new();
    if !thread_id.is_empty() {
        let (on, comments) = github.review_thread(&thread_id).await?;
        if on != pr {
            return Err(format!("thread is on {on}, not {pr}"));
        }
        if let Some(comment) = &comment
            && !comments.iter().any(|c| c.id == comment.id)
        {
            return Err(format!("thread {thread_id} does not hold {}", comment.id));
        }
        earlier_comments = comments
            .into_iter()
            .take_while(|c| comment.as_ref().is_none_or(|comment| c.id != comment.id))
            .collect();
    }
    if resolve_thread && thread_id.is_empty() {
        return Err(
            "`resolve_thread` needs a review thread, and this comment is not in one".into(),
        );
    }
    let reruns = (check_failure.iter()).any(|f| f.checks.iter().any(|c| c.workflow_run_id != 0));
    if rerun && !reruns {
        return Err("`rerun` needs failed checks with a GitHub Actions run".into());
    }

    let steps = !patch.is_empty()
        || !other_patches.is_empty()
        || open_pull_request.is_some()
        || !reply.trim().is_empty()
        || resolve_thread
        || !follow_up.trim().is_empty()
        || rerun
        || close_pull_request;
    if steps != request_tool.trim().is_empty() {
        return Err(
            "propose either steps (`patch`, `other_patches`, `open_pull_request`, `reply`, `resolve_thread`, `follow_up`, `rerun`, `close_pull_request`) or `request_tool`"
                .into(),
        );
    }
    let patch = match patch.as_str() {
        "" => None,
        diff => {
            if message.trim().is_empty() {
                return Err("`message` is required with `patch`".into());
            }
            let head_oid = github.check_patch(pr, diff).await?;
            Some(Patch {
                diff: diff.to_string(),
                commit_message: message,
                head_oid,
                pull_request_url: String::new(),
            })
        }
    };
    let others: Vec<&String> = cluster.iter().filter(|p| *p != pr).collect();
    let mut checked = Vec::new();
    for other in other_patches {
        let url = other.pr.trim().trim_end_matches('/');
        if checked.iter().any(|p: &Patch| p.pull_request_url == url) {
            return Err(format!(
                "{url} has more than one patch; combine its changes into one"
            ));
        }
        if !others.iter().any(|p| *p == url) {
            let others: Vec<&str> = others.iter().map(|p| p.as_str()).collect();
            return Err(format!(
                "{url} is not another pull request in {pr}'s cluster, which has {}",
                if others.is_empty() {
                    "none".into()
                } else {
                    others.join(" and ")
                }
            ));
        }
        if other.message.trim().is_empty() {
            return Err(format!("`message` is required with the patch for {url}"));
        }
        let head_oid = (github.check_patch(url, &other.patch).await)
            .map_err(|e| format!("the patch for {url}: {e}"))?;
        checked.push(Patch {
            diff: other.patch,
            commit_message: other.message,
            head_oid,
            pull_request_url: url.to_string(),
        });
    }
    let open_pull_request = match open_pull_request {
        None => None,
        Some(new) => Some(check_open_pull_request(github, pr, new).await?),
    };
    // A top-level reply quotes the comment it answers in its own markdown, so the quote shows,
    // and is edited, with the rest of what is posted.
    let reply = match &comment {
        Some(comment) if thread_id.is_empty() && !reply.trim().is_empty() => {
            let quoted: String = (comment.body.lines())
                .map(|line| format!("> {line}\n"))
                .collect();
            format!("{quoted}\n{reply}")
        }
        _ => reply,
    };
    let summary_html = github.render(pr, &summary).await?;
    let reply_html = github.render(pr, &reply).await?;
    let follow_up_html = github.render(pr, &follow_up).await?;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let proposal = Proposal {
        id: format!("{nanos:x}"),
        summary,
        pull_request_url: pr.to_string(),
        comment,
        thread_id,
        patch,
        reply,
        resolve_thread,
        error: String::new(),
        requested_tool: request_tool,
        earlier_comments,
        reply_html,
        follow_up,
        follow_up_html,
        close_pull_request,
        check_failure,
        rerun,
        summary_html,
        investigation_id: investigation_id.to_string(),
        other_patches: checked,
        open_pull_request,
    };
    // The proposal it replaces may have been approved or dismissed meanwhile.
    let recorded = db.write(|moment| {
        let mut proposals = map_mut(moment, PROPOSALS)?;
        if let Some(replaced) = &replaced
            && !proposals.remove(replaced)?
        {
            return Ok(false);
        }
        proposals.put(&proposal.id, serde_json::to_string(&proposal).unwrap())?;
        Ok(true)
    })?;
    if !recorded {
        return Err(format!(
            "proposal {} is gone: the user approved or dismissed it",
            replaced.unwrap_or_default()
        ));
    }
    Ok(proposal.id)
}

// Checks a pull request to open, proposed about the pull request at `pr`, which its body must link.
async fn check_open_pull_request(
    github: &impl GitHub,
    pr: &str,
    new: OpenPullRequest,
) -> Result<NewPullRequest, String> {
    let repository = (new.repository.trim())
        .trim_start_matches("https://github.com/")
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let parts: Vec<&str> = repository.split('/').collect();
    if parts.len() != 2 || parts.iter().any(|part| matches!(*part, "" | "." | "..")) {
        return Err(format!(
            "`open_pull_request.repository` is `owner/name`, not `{}`",
            new.repository
        ));
    }
    let base = new.base.trim();
    let branch = new.branch.trim();
    for (field, value) in [
        ("base", base),
        ("branch", branch),
        ("title", new.title.trim()),
        ("patch", new.patch.as_str()),
        ("message", new.message.trim()),
    ] {
        if value.is_empty() {
            return Err(format!("`open_pull_request.{field}` is required"));
        }
    }
    if branch == base {
        return Err("`open_pull_request.branch` must be a new branch, not `base`".into());
    }
    // A link to the pull request, not one to a longer number that starts the same.
    let links = new
        .body
        .match_indices(pr)
        .any(|(at, _)| !(new.body[at + pr.len()..].starts_with(|c: char| c.is_ascii_digit())));
    if !links {
        return Err(format!(
            "`open_pull_request.body` must link {pr}, so gem groups the two pull requests"
        ));
    }
    let head_oid = (github
        .check_new_pull_request(repository, base, branch, &new.patch)
        .await)
        .map_err(|e| format!("the patch for the new pull request: {e}"))?;
    Ok(NewPullRequest {
        repository: repository.to_string(),
        base: base.to_string(),
        branch: branch.to_string(),
        title: new.title.trim().to_string(),
        // Rendered where it is posted, so `#123` links an issue of that repository.
        body_html: (github.render(&format!("https://github.com/{repository}"), &new.body)).await?,
        body: new.body,
        patch: Some(Patch {
            diff: new.patch,
            commit_message: new.message,
            head_oid,
            pull_request_url: String::new(),
        }),
    })
}

pub struct FetchedComment {
    pub comment: Comment,
    pub pull_request: String,
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
        #[serde(default)]
        path: String,
        line: Option<u32>,
        #[serde(rename = "diffHunk", default)]
        diff_hunk: String,
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
        &format!("query($id: ID!) {{ node(id: $id) {{ ... on IssueComment {{ {fields} }} ... on PullRequestReviewComment {{ {fields} path line diffHunk }} }} }}"),
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
            path: node.path,
            line: node.line.unwrap_or_default(),
            diff_hunk: node.diff_hunk,
        },
    })
}

// In the database's moment, the hash map from each proposal's ID to the Proposal, as JSON.
const PROPOSALS: &str = "proposals";

pub fn load(db: &Db) -> Vec<Proposal> {
    let proposals = db.read(|moment| texts(moment, PROPOSALS));
    match proposals {
        Ok(proposals) => (proposals.iter())
            .filter_map(|(_, json)| serde_json::from_str(json).ok())
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "cannot read proposals");
            Vec::new()
        }
    }
}

fn save(db: &Db, proposal: &Proposal) {
    let saved = db.write(|moment| {
        map_mut(moment, PROPOSALS)?.put(&proposal.id, serde_json::to_string(proposal).unwrap())
    });
    if let Err(error) = saved {
        tracing::warn!(proposal = %proposal.id, %error, "cannot save proposal");
    }
}

fn delete(id: &str) {
    let deleted = crate::db().write(|moment| map_mut(moment, PROPOSALS)?.remove(id));
    if let Err(error) = deleted {
        tracing::warn!(proposal = id, %error, "cannot delete proposal");
    }
}
