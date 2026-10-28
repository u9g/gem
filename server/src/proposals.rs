use std::{
    collections::{HashMap, HashSet},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use gem_db::{Db, params};
use gem_json::{Error, FromJson, Value};
use gem_log::{Level, warn};

use crate::proto::{CheckFailure, Comment, NewPullRequest, Patch, Proposal, PullRequestCluster};
use crate::tokens::Subject;
use crate::{conversations, investigations};
use gem_github::{
    apply_patch, check_new_pull_request, command, graphql, open_pull_request, push_new_branch,
    render, repository,
};

// Agents make proposals with the `propose` tool of the `gem-agent` MCP server, which checks them
// first.

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

pub fn find(id: &str) -> Option<Proposal> {
    load(crate::db())
        .into_iter()
        .find(|proposal| proposal.id == id)
}

// Whether a proposal answers the conversation or investigation `id`.
pub fn exist_for(id: &str) -> bool {
    (load(crate::db()).iter()).any(|proposal| subject(proposal) == Some(id))
}

// The proposals of each investigation, by its ID.
pub fn of_investigations() -> HashMap<String, Vec<Proposal>> {
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

pub fn remove_of_investigation(id: &str) {
    for proposal in load(crate::db()) {
        if proposal.investigation_id == id {
            delete(&proposal.id);
        }
    }
}

// A proposal's review comment takes where it now sits from `waiting`, so a push since the
// proposal was made moves or outdates it.
pub fn fill(clusters: &mut [PullRequestCluster], waiting: &HashMap<String, Option<Comment>>) {
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
pub fn approve(
    id: &str,
    reply: &str,
    follow_up: &str,
    diff: &str,
) -> Result<Option<String>, String> {
    let proposals = load(crate::db());
    let Some(mut proposal) = proposals.iter().find(|p| p.id == id).cloned() else {
        return Err(format!("no proposal {id}"));
    };
    let closes = proposal.close_pull_request;
    if !reply.is_empty() && !proposal.reply.is_empty() {
        proposal.reply = reply.to_string();
        if let Ok(html) = render(&proposal.pull_request_url, reply) {
            proposal.reply_html = html;
        }
    }
    if !follow_up.is_empty() && !proposal.follow_up.is_empty() {
        proposal.follow_up = follow_up.to_string();
        if let Ok(html) = render(&proposal.pull_request_url, follow_up) {
            proposal.follow_up_html = html;
        }
    }
    if let Some(patch) = proposal.patch.as_mut().filter(|_| !diff.is_empty()) {
        patch.diff = diff.to_string();
    }
    match run(&mut proposal) {
        // Nothing else on a closed PR needs answering.
        Ok(()) if closes => {
            remove_on(std::slice::from_ref(&proposal.pull_request_url));
            Ok(Some(proposal.pull_request_url))
        }
        Ok(()) => {
            let answered = answers(&proposal);
            for other in &proposals {
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

// A proposal is stale once it has a patch written against a head commit its branch has since moved
// past, or for another pull request no longer open, keyed by open PR URL in `heads`. Deletes each
// stale proposal, and every proposal answering the same conversation. Returns those conversations'
// IDs, so they can be answered again against the new heads. An investigation's proposals are
// independent, so only its stale ones go.
pub fn remove_stale(heads: &HashMap<&str, &str>) -> HashSet<String> {
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
    let conversations: HashSet<String> = (stale.iter())
        .filter(|p| p.investigation_id.is_empty())
        .filter_map(|p| subject(p).map(str::to_string))
        .collect();
    for proposal in &proposals {
        let answers_stale = proposal.investigation_id.is_empty()
            && subject(proposal).is_some_and(|id| conversations.contains(id));
        if answers_stale || stale.iter().any(|p| p.id == proposal.id) {
            delete(&proposal.id);
        }
    }
    conversations
}

// Deletes every proposal on the PRs at `urls`, except investigations'.
pub fn remove_on(urls: &[String]) {
    for proposal in load(crate::db()) {
        if urls.contains(&proposal.pull_request_url) && proposal.investigation_id.is_empty() {
            delete(&proposal.id);
        }
    }
}

pub fn dismiss(id: &str) {
    delete(id);
}

// Logs a step of an approve starting and how it ended. A step that succeeded is saved as done
// before the next starts, so the log shows how far a failed approve got.
// `context` is what the step is about: the proposal, its PR, and what the step acts on.
fn step<T>(
    context: &[(&'static str, String)],
    name: &'static str,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let mut fields = context.to_vec();
    fields.push(("step", name.to_string()));
    gem_log::event(Level::Info, "step started", &fields);
    let started = Instant::now();
    let result = work();
    fields.push((
        "elapsed_ms",
        (started.elapsed().as_millis() as u64).to_string(),
    ));
    match &result {
        Ok(_) => gem_log::event(Level::Info, "step done", &fields),
        Err(error) => {
            fields.push(("error", error.clone()));
            gem_log::event(Level::Warn, "step failed", &fields);
        }
    }
    result
}

// What everything logged while approving `proposal` carries, with `more`.
fn context(proposal: &Proposal, more: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    let mut context = vec![
        ("proposal", proposal.id.clone()),
        ("pull_request", proposal.pull_request_url.clone()),
    ];
    for &(key, value) in more {
        // An inner span's field of the same name, such as another PR's, wins.
        context.retain(|(k, _)| *k != key);
        context.push((key, value.to_string()));
    }
    context
}

fn run(proposal: &mut Proposal) -> Result<(), String> {
    let approving = context(proposal, &[]);
    if !proposal.follow_up.is_empty() {
        step(&approving, "follow_up", || {
            investigations::follow_up(
                &proposal.summary,
                &proposal.pull_request_url,
                &proposal.follow_up,
            )
        })?;
        proposal.follow_up.clear();
        save(crate::db(), proposal);
    }
    if let Some(patch) = &proposal.patch {
        step(&approving, "patch", || {
            apply_patch(
                &proposal.pull_request_url,
                &patch.diff,
                Some(&patch.commit_message),
            )
        })?;
        proposal.patch = None;
        save(crate::db(), proposal);
    }
    while let Some(patch) = proposal.other_patches.first() {
        let other = context(proposal, &[("pull_request", &patch.pull_request_url)]);
        step(&other, "other_patch", || {
            apply_patch(
                &patch.pull_request_url,
                &patch.diff,
                Some(&patch.commit_message),
            )
        })?;
        proposal.other_patches.remove(0);
        save(crate::db(), proposal);
    }
    // The pushed branch's patch is dropped, so a retry opens the pull request without pushing
    // again.
    if let Some(new) = &proposal.open_pull_request
        && let Some(patch) = &new.patch
    {
        let on_new = context(
            proposal,
            &[("repository", &new.repository), ("branch", &new.branch)],
        );
        step(&on_new, "push_new_branch", || {
            push_new_branch(
                &new.repository,
                &new.base,
                &new.branch,
                &patch.diff,
                &patch.commit_message,
            )
        })?;
        if let Some(new) = &mut proposal.open_pull_request {
            new.patch = None;
        }
        save(crate::db(), proposal);
    }
    if let Some(new) = &proposal.open_pull_request {
        let mut on_new = context(
            proposal,
            &[("repository", &new.repository), ("branch", &new.branch)],
        );
        let url = step(&on_new, "open_pull_request", || {
            open_pull_request(
                &new.repository,
                &new.base,
                &new.branch,
                &new.title,
                &new.body,
            )
        })?;
        on_new.push(("url", url));
        gem_log::event(Level::Info, "pull request opened", &on_new);
        proposal.open_pull_request = None;
        save(crate::db(), proposal);
    }
    if !proposal.reply.is_empty() {
        if proposal.thread_id.is_empty() {
            step(&approving, "comment", || {
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
                )
            })?;
        } else {
            step(&approving, "reply", || {
                graphql(
                    "mutation($thread: ID!, $body: String!) { addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $thread, body: $body}) { comment { id } } }",
                    &[("thread", &proposal.thread_id), ("body", &proposal.reply)],
                )
            })?;
        }
        proposal.reply.clear();
        save(crate::db(), proposal);
    }
    if proposal.resolve_thread {
        step(&approving, "resolve_thread", || {
            graphql(
                "mutation($thread: ID!) { resolveReviewThread(input: {threadId: $thread}) { thread { id } } }",
                &[("thread", &proposal.thread_id)],
            )
        })?;
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
            let run = run.to_string();
            let workflow_run = context(proposal, &[("id", &run)]);
            step(&workflow_run, "rerun", || {
                command(
                    "gh",
                    &["run", "rerun", &run, "--failed", "-R", &repository],
                    None,
                )
            })?;
        }
        proposal.rerun = false;
        save(crate::db(), proposal);
    }
    if proposal.close_pull_request {
        step(&approving, "close", || {
            command("gh", &["pr", "close", &proposal.pull_request_url], None)
        })?;
        proposal.close_pull_request = false;
        save(crate::db(), proposal);
    }
    Ok(())
}

// What an agent proposes, as the `propose` tool of the `gem-agent` MCP server takes it. The
// subject it answers comes from the agent's token, not from here.
// Every field may be left out. No other field is allowed, so a misspelled one is not ignored.
#[derive(Default, Debug)]
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
}

// A diff for another pull request of the cluster, and the message to commit it with.
#[derive(Debug)]
pub struct OtherPatch {
    pub pr: String,
    pub patch: String,
    pub message: String,
}

// A pull request to open: a diff against `base` of `repository`, committed on `branch`.
#[derive(Debug)]
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
#[derive(Debug)]
pub struct Excerpt {
    pub check: String,
    pub lines: String,
}

// Fails on a field of `value` not in `fields`, as serde's `deny_unknown_fields` does.
fn known_fields(value: &Value, what: &str, fields: &[&str]) -> Result<(), Error> {
    let object = (value.as_object())
        .ok_or_else(|| Error::new(format!("invalid type: {}, expected {what}", value.kind())))?;
    for name in object.keys() {
        if !fields.contains(&name.as_str()) {
            let expected: Vec<String> = fields.iter().map(|f| format!("`{f}`")).collect();
            let expected = match expected.len() {
                1 => expected[0].clone(),
                2 => format!("{} or {}", expected[0], expected[1]),
                _ => format!("one of {}", expected.join(", ")),
            };
            return Err(Error::new(format!(
                "unknown field `{name}`, expected {expected}"
            )));
        }
    }
    Ok(())
}

// A field that must be there.
fn required<T: FromJson>(value: &Value, name: &str) -> Result<T, Error> {
    match value.get(name) {
        None => Err(Error::new(format!("missing field `{name}`"))),
        Some(_) => value.field(name),
    }
}

impl FromJson for Request {
    fn from_json(value: &Value) -> Result<Request, Error> {
        known_fields(
            value,
            "struct Request",
            &[
                "pr",
                "summary",
                "patch",
                "message",
                "other_patches",
                "open_pull_request",
                "reply",
                "follow_up",
                "resolve_thread",
                "rerun",
                "close_pull_request",
                "excerpts",
                "request_tool",
            ],
        )?;
        Ok(Request {
            pr: value.field_or_default("pr")?,
            summary: value.field_or_default("summary")?,
            patch: value.field_or_default("patch")?,
            message: value.field_or_default("message")?,
            other_patches: value.field_or_default("other_patches")?,
            open_pull_request: value.field("open_pull_request")?,
            reply: value.field_or_default("reply")?,
            follow_up: value.field_or_default("follow_up")?,
            resolve_thread: value.field_or_default("resolve_thread")?,
            rerun: value.field_or_default("rerun")?,
            close_pull_request: value.field_or_default("close_pull_request")?,
            excerpts: value.field_or_default("excerpts")?,
            request_tool: value.field_or_default("request_tool")?,
        })
    }
}

impl FromJson for OtherPatch {
    fn from_json(value: &Value) -> Result<OtherPatch, Error> {
        known_fields(value, "struct OtherPatch", &["pr", "patch", "message"])?;
        Ok(OtherPatch {
            pr: required(value, "pr")?,
            patch: required(value, "patch")?,
            message: required(value, "message")?,
        })
    }
}

impl FromJson for OpenPullRequest {
    fn from_json(value: &Value) -> Result<OpenPullRequest, Error> {
        known_fields(
            value,
            "struct OpenPullRequest",
            &[
                "repository",
                "base",
                "branch",
                "title",
                "body",
                "patch",
                "message",
            ],
        )?;
        Ok(OpenPullRequest {
            repository: required(value, "repository")?,
            base: required(value, "base")?,
            branch: required(value, "branch")?,
            title: required(value, "title")?,
            body: required(value, "body")?,
            patch: required(value, "patch")?,
            message: required(value, "message")?,
        })
    }
}

impl FromJson for Excerpt {
    fn from_json(value: &Value) -> Result<Excerpt, Error> {
        known_fields(value, "struct Excerpt", &["check", "lines"])?;
        Ok(Excerpt {
            check: required(value, "check")?,
            lines: required(value, "lines")?,
        })
    }
}

// What checking a proposal asks of GitHub, so tests can answer without it.
pub trait GitHub: Send + Sync + 'static {
    fn comment(&self, id: &str) -> Result<FetchedComment, String>;
    // The failed checks on the PR at `pr` while `id` is still their ID.
    fn check_failure(&self, pr: &str, id: &str) -> Result<CheckFailure, String>;
    // The review thread's PR URL and its comments, oldest first.
    fn review_thread(&self, id: &str) -> Result<(String, Vec<Comment>), String>;
    // Checks that the diff applies to the PR's head. Returns that head commit.
    fn check_patch(&self, pr: &str, diff: &str) -> Result<String, String>;
    // Checks that the diff applies to the head of `base` in the repository `owner/name`, and that
    // `branch` is new. Returns that head commit.
    fn check_new_pull_request(
        &self,
        repository: &str,
        base: &str,
        branch: &str,
        diff: &str,
    ) -> Result<String, String>;
    fn render(&self, pr: &str, markdown: &str) -> Result<String, String>;
}

pub struct Live;

impl GitHub for Live {
    fn comment(&self, id: &str) -> Result<FetchedComment, String> {
        fetch_comment(id)
    }

    fn check_failure(&self, pr: &str, id: &str) -> Result<CheckFailure, String> {
        let node = command(
            "gh",
            &["pr", "view", pr, "--json", "id", "--jq", ".id"],
            None,
        )?;
        conversations::check_failure(String::from_utf8_lossy(&node).trim(), id)
            .ok_or(format!("{id} is not the failed checks on {pr}"))
    }

    fn review_thread(&self, id: &str) -> Result<(String, Vec<Comment>), String> {
        let value = gem_json::parse_bytes(&graphql(
            "query($id: ID!) { node(id: $id) { ... on PullRequestReviewThread { pullRequest { url } comments(first: 100) { nodes { id body bodyHTML url author { login } } } } } }",
            &[("id", id)],
        )?)
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

    fn check_patch(&self, pr: &str, diff: &str) -> Result<String, String> {
        apply_patch(pr, diff, None)
    }

    fn check_new_pull_request(
        &self,
        repository: &str,
        base: &str,
        branch: &str,
        diff: &str,
    ) -> Result<String, String> {
        check_new_pull_request(repository, base, branch, diff)
    }

    fn render(&self, pr: &str, markdown: &str) -> Result<String, String> {
        render(pr, markdown)
    }
}

// Checks what an agent answering `subject` proposes and saves it in `db`. `cluster` is the URLs
// of the pull requests in the cluster of the one it is about. Returns the proposal's ID, or why it
// cannot be approved, for the agent to fix and propose again.
pub fn propose(
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
    } = request;
    let pr = pr.trim().trim_end_matches('/');
    if pr.is_empty() {
        return Err("`pr` is required: the URL of the pull request the proposal is about".into());
    }
    if summary.trim().is_empty() {
        return Err("`summary` is required".into());
    }
    if !subject.prs().iter().any(|p| p == pr) {
        return Err(format!(
            "{pr} is not a pull request this thread answers on; propose on {}",
            subject.prs().join(" or ")
        ));
    }

    let (comment, check_failure, thread_id, investigation_id) = match subject {
        Subject::Comment {
            comment, thread, ..
        } => {
            let comment = github.comment(comment)?;
            if comment.pull_request != pr {
                return Err(format!("comment is on {}, not {pr}", comment.pull_request));
            }
            (Some(comment.comment), None, thread.clone(), "")
        }
        Subject::CheckFailure { id, .. } => {
            let mut failure = github.check_failure(pr, id)?;
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
        let (on, comments) = github.review_thread(&thread_id)?;
        if on != pr {
            return Err(format!("thread is on {on}, not {pr}"));
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
            let head_oid = github.check_patch(pr, diff)?;
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
        let head_oid = github
            .check_patch(url, &other.patch)
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
        Some(new) => Some(check_open_pull_request(github, pr, new)?),
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
    let summary_html = github.render(pr, &summary)?;
    let reply_html = github.render(pr, &reply)?;
    let follow_up_html = github.render(pr, &follow_up)?;

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
    db.with(|db| {
        db.execute(
            "INSERT INTO proposals (id, proposal) VALUES (?1, ?2)",
            params![proposal.id, gem_json::to_string(&proposal)],
        )
    })?;
    Ok(proposal.id)
}

// Checks a pull request to open, proposed about the pull request at `pr`, which its body must link.
fn check_open_pull_request(
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
    let head_oid = github
        .check_new_pull_request(repository, base, branch, &new.patch)
        .map_err(|e| format!("the patch for the new pull request: {e}"))?;
    Ok(NewPullRequest {
        repository: repository.to_string(),
        base: base.to_string(),
        branch: branch.to_string(),
        title: new.title.trim().to_string(),
        // Rendered where it is posted, so `#123` links an issue of that repository.
        body_html: github.render(&format!("https://github.com/{repository}"), &new.body)?,
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

fn fetch_comment(id: &str) -> Result<FetchedComment, String> {
    let fields = "id body bodyHTML url author { login } pullRequest { url }";
    let output = graphql(
        &format!(
            "query($id: ID!) {{ node(id: $id) {{ ... on IssueComment {{ {fields} }} ... on PullRequestReviewComment {{ {fields} path line diffHunk }} }} }}"
        ),
        &[("id", id)],
    )?;
    let value = gem_json::parse_bytes(&output).map_err(|e| e.to_string())?;
    read_comment(&value["data"]["node"])
        .map_err(|_| format!("{id} is not a comment on a pull request"))
}

// A comment node, as `fetch_comment` asks for it. Only a review comment has a path, line and
// diff hunk; the author and pull request are null when deleted.
fn read_comment(node: &Value) -> Result<FetchedComment, Error> {
    let author: Option<Value> = node.field("author")?;
    let author = author.map(|a| a.field::<String>("login")).transpose()?;
    let pull_request: Option<Value> = node.field("pullRequest")?;
    let pull_request = (pull_request.map(|p| p.field::<String>("url"))).transpose()?;
    Ok(FetchedComment {
        pull_request: pull_request.unwrap_or_default(),
        comment: Comment {
            id: node.field("id")?,
            author: author.unwrap_or("ghost".into()),
            body: node.field("body")?,
            url: node.field("url")?,
            body_html: node.field("bodyHTML")?,
            path: node.field_or_default("path")?,
            line: node.field::<Option<u32>>("line")?.unwrap_or_default(),
            diff_hunk: node.field_or_default("diffHunk")?,
        },
    })
}

pub fn load(db: &Db) -> Vec<Proposal> {
    let proposals = db.with(|db| {
        db.prepare("SELECT proposal FROM proposals")?
            .query_map(params![], |row| row.get::<String>(0))?
            .collect::<Result<Vec<_>, _>>()
    });
    match proposals {
        Ok(proposals) => (proposals.iter())
            .filter_map(|json| gem_json::from_str(json).ok())
            .collect(),
        Err(error) => {
            warn!("cannot read proposals", error = error);
            Vec::new()
        }
    }
}

fn save(db: &Db, proposal: &Proposal) {
    let saved = db.with(|db| {
        db.execute(
            "INSERT OR REPLACE INTO proposals (id, proposal) VALUES (?1, ?2)",
            params![proposal.id, gem_json::to_string(proposal)],
        )
    });
    if let Err(error) = saved {
        warn!(
            "cannot save proposal",
            proposal = proposal.id,
            error = error
        );
    }
}

fn delete(id: &str) {
    let deleted =
        crate::db().with(|db| db.execute("DELETE FROM proposals WHERE id = ?1", params![id]));
    if let Err(error) = deleted {
        warn!("cannot delete proposal", proposal = id, error = error);
    }
}
