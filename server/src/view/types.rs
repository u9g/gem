// What the view holds, as plain structs: exactly what frontends render and agents' scripts read,
// written with xitdb's `Structured` codec. Every field is always written, so the stored data
// describes itself and a reader needs no schema: a struct is a map from its camelCase field names,
// a `Vec` a list, a string bytes, a number a number, a bool `bl` text, and an enum its variant's
// name. Only an `Option` can be missing, when it's None. Times are milliseconds since the epoch.
//
// They're made from the protobuf messages the server fills, in `From` impls here, the one place
// the two meet; the protobuf types stay for the action requests and responses.
//
// The frontend's types, frontend/view-types.ts, are rendered from these by ts-rs, and a test fails
// when the committed file is stale. ts-rs reads the serde attributes, so the names are the stored
// ones; an `Option` field is an optional property, as the frontend reads an unset one as
// undefined.

use std::collections::HashMap;

use pbjson_types::Timestamp;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::proto;

#[derive(Serialize, Deserialize, TS, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum MergeBy {
    // It can't merge.
    #[default]
    Unspecified,
    // Every pull request is approved and its checks pass.
    Approval,
    // Its checks pass, and the user can force the unapproved ones in as an admin.
    Admin,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum AgentState {
    #[default]
    Working,
    Queued,
    Waiting,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct Cluster {
    // Oldest first.
    pub pull_requests: Vec<PullRequest>,
    pub updated_at: Option<i64>,
    pub name: String,
    pub merge_by: MergeBy,
    pub agents: Vec<Agent>,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct PullRequest {
    pub repository: Option<Repository>,
    pub number: u32,
    pub title: String,
    pub url: String,
    pub approved: bool,
    pub checks_passing: bool,
    pub checks_failing: bool,
    pub proposals: Vec<Proposal>,
    pub behind: bool,
    pub checks: Option<Checks>,
    pub head_oid: String,
    pub merge_by: MergeBy,
    pub files: Vec<FileDiff>,
    pub base: String,
    pub conflicts: bool,
    pub rebasing: bool,
    pub rebased: bool,
    pub merging: bool,
    pub no_proposal_reason: String,
    pub created_at: Option<i64>,
    // Missing from views written before events were kept.
    #[serde(default)]
    pub events: Vec<Event>,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum EventKind {
    #[default]
    Commented,
    ReviewCommented,
    Approved,
    ChangesRequested,
    Reviewed,
    ReviewDismissed,
    ReadyForReview,
    ConvertedToDraft,
    Retargeted,
    Renamed,
    Closed,
    Reopened,
    Merged,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct Event {
    pub id: String,
    pub kind: EventKind,
    pub actor: String,
    pub at: Option<i64>,
    pub url: String,
    pub body: String,
    pub body_html: String,
    pub subject: String,
    // A review's comments on lines. Missing from views written before they were kept.
    #[serde(default)]
    pub comments: Vec<Event>,
    #[serde(default)]
    pub answering: bool,
    #[serde(default)]
    pub trusted: bool,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Checks {
    pub total: u32,
    pub done: u32,
    pub failed: u32,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub owner: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct Agent {
    pub id: String,
    pub title: String,
    pub state: AgentState,
    pub pull_request_urls: Vec<String>,
    pub reply: String,
    pub reply_html: String,
    pub proposals: Vec<Proposal>,
    pub started_at: Option<i64>,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct Proposal {
    pub id: String,
    pub summary: String,
    pub pull_request_url: String,
    pub comment: Option<Comment>,
    pub thread_id: String,
    pub patch: Option<Patch>,
    pub reply: String,
    pub resolve_thread: bool,
    pub error: String,
    pub requested_tool: String,
    pub earlier_comments: Vec<Comment>,
    pub reply_html: String,
    pub follow_up: String,
    pub follow_up_html: String,
    pub close_pull_request: bool,
    pub check_failure: Option<CheckFailure>,
    pub rerun: bool,
    pub summary_html: String,
    pub investigation_id: String,
    pub other_patches: Vec<Patch>,
    pub open_pull_request: Option<NewPullRequest>,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct NewPullRequest {
    pub repository: String,
    pub base: String,
    pub branch: String,
    pub title: String,
    pub body: String,
    pub body_html: String,
    pub patch: Option<Patch>,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CheckFailure {
    pub id: String,
    pub head_oid: String,
    pub checks: Vec<FailedCheck>,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FailedCheck {
    pub id: String,
    pub name: String,
    pub conclusion: String,
    pub url: String,
    pub workflow_run_id: u64,
    pub excerpt: String,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub url: String,
    pub body_html: String,
    pub path: String,
    pub line: u32,
    pub diff_hunk: String,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub patch: String,
}

#[derive(Serialize, Deserialize, TS, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    pub diff: String,
    pub commit_message: String,
    pub head_oid: String,
    pub pull_request_url: String,
}

// A moment of the view, as the frontend reads it. The server writes each of its keys on its own,
// so it's never made here; it exists to say what the frontend reads.
#[derive(TS)]
#[allow(dead_code)]
pub struct Moment {
    // Each cluster by its key, the URL of its oldest pull request.
    pub clusters: HashMap<String, Cluster>,
    // The clusters' keys, most recently updated first.
    pub order: Vec<String>,
    pub login: String,
    pub search: String,
    // The agents acting on no open pull request.
    pub agents: Vec<Agent>,
    // The actions in progress, each by the key the frontend names it by, with its route.
    pub pending: HashMap<String, String>,
}

// What the UI shows, as the server fills it: the clusters in order, and the rest.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shown {
    pub clusters: Vec<Cluster>,
    pub login: String,
    pub search: String,
    pub agents: Vec<Agent>,
}

// Milliseconds since the epoch.
fn millis(time: &Option<Timestamp>) -> Option<i64> {
    time.as_ref().map(|time| time.seconds * 1000 + i64::from(time.nanos) / 1_000_000)
}

impl From<i32> for MergeBy {
    fn from(value: i32) -> MergeBy {
        match proto::MergeBy::try_from(value) {
            Ok(proto::MergeBy::Approval) => MergeBy::Approval,
            Ok(proto::MergeBy::Admin) => MergeBy::Admin,
            _ => MergeBy::Unspecified,
        }
    }
}

impl From<&proto::ListOpenPullRequestsResponse> for Shown {
    fn from(response: &proto::ListOpenPullRequestsResponse) -> Shown {
        Shown {
            clusters: response.clusters.iter().map(Cluster::from).collect(),
            login: response.login.clone(),
            search: response.search.clone(),
            agents: response.agents.iter().map(Agent::from).collect(),
        }
    }
}

impl From<&proto::PullRequestCluster> for Cluster {
    fn from(cluster: &proto::PullRequestCluster) -> Cluster {
        Cluster {
            pull_requests: cluster.pull_requests.iter().map(PullRequest::from).collect(),
            updated_at: millis(&cluster.updated_at),
            name: cluster.name.clone(),
            merge_by: cluster.merge_by.into(),
            agents: cluster.agents.iter().map(Agent::from).collect(),
        }
    }
}

impl From<&proto::PullRequest> for PullRequest {
    fn from(pr: &proto::PullRequest) -> PullRequest {
        PullRequest {
            repository: (pr.repository.as_ref())
                .map(|r| Repository { owner: r.owner.clone(), name: r.name.clone() }),
            number: pr.number,
            title: pr.title.clone(),
            url: pr.url.clone(),
            approved: pr.approved,
            checks_passing: pr.checks_passing,
            checks_failing: pr.checks_failing,
            proposals: pr.proposals.iter().map(Proposal::from).collect(),
            behind: pr.behind,
            checks: (pr.checks.as_ref())
                .map(|c| Checks { total: c.total, done: c.done, failed: c.failed }),
            head_oid: pr.head_oid.clone(),
            merge_by: pr.merge_by.into(),
            files: (pr.files.iter())
                .map(|f| FileDiff { path: f.path.clone(), patch: f.patch.clone() })
                .collect(),
            base: pr.base.clone(),
            conflicts: pr.conflicts,
            rebasing: pr.rebasing,
            rebased: pr.rebased,
            merging: pr.merging,
            no_proposal_reason: pr.no_proposal_reason.clone(),
            created_at: millis(&pr.created_at),
            events: pr.events.iter().map(Event::from).collect(),
        }
    }
}

impl From<&proto::Event> for Event {
    fn from(event: &proto::Event) -> Event {
        use proto::EventKind as K;
        Event {
            id: event.id.clone(),
            kind: match K::try_from(event.kind).unwrap_or(K::Commented) {
                K::Commented => EventKind::Commented,
                K::ReviewCommented => EventKind::ReviewCommented,
                K::Approved => EventKind::Approved,
                K::ChangesRequested => EventKind::ChangesRequested,
                K::Reviewed => EventKind::Reviewed,
                K::ReviewDismissed => EventKind::ReviewDismissed,
                K::ReadyForReview => EventKind::ReadyForReview,
                K::ConvertedToDraft => EventKind::ConvertedToDraft,
                K::Retargeted => EventKind::Retargeted,
                K::Renamed => EventKind::Renamed,
                K::Closed => EventKind::Closed,
                K::Reopened => EventKind::Reopened,
                K::Merged => EventKind::Merged,
            },
            actor: event.actor.clone(),
            at: millis(&event.at),
            url: event.url.clone(),
            body: event.body.clone(),
            body_html: event.body_html.clone(),
            subject: event.subject.clone(),
            comments: event.comments.iter().map(Event::from).collect(),
            answering: event.answering,
            trusted: event.trusted,
        }
    }
}

impl From<&proto::Agent> for Agent {
    fn from(agent: &proto::Agent) -> Agent {
        Agent {
            id: agent.id.clone(),
            title: agent.title.clone(),
            state: match proto::AgentState::try_from(agent.state) {
                Ok(proto::AgentState::Queued) => AgentState::Queued,
                Ok(proto::AgentState::Waiting) => AgentState::Waiting,
                _ => AgentState::Working,
            },
            pull_request_urls: agent.pull_request_urls.clone(),
            reply: agent.reply.clone(),
            reply_html: agent.reply_html.clone(),
            proposals: agent.proposals.iter().map(Proposal::from).collect(),
            started_at: millis(&agent.started_at),
        }
    }
}

impl From<&proto::Proposal> for Proposal {
    fn from(proposal: &proto::Proposal) -> Proposal {
        Proposal {
            id: proposal.id.clone(),
            summary: proposal.summary.clone(),
            pull_request_url: proposal.pull_request_url.clone(),
            comment: proposal.comment.as_ref().map(Comment::from),
            thread_id: proposal.thread_id.clone(),
            patch: proposal.patch.as_ref().map(Patch::from),
            reply: proposal.reply.clone(),
            resolve_thread: proposal.resolve_thread,
            error: proposal.error.clone(),
            requested_tool: proposal.requested_tool.clone(),
            earlier_comments: proposal.earlier_comments.iter().map(Comment::from).collect(),
            reply_html: proposal.reply_html.clone(),
            follow_up: proposal.follow_up.clone(),
            follow_up_html: proposal.follow_up_html.clone(),
            close_pull_request: proposal.close_pull_request,
            check_failure: proposal.check_failure.as_ref().map(|failure| CheckFailure {
                id: failure.id.clone(),
                head_oid: failure.head_oid.clone(),
                checks: (failure.checks.iter())
                    .map(|check| FailedCheck {
                        id: check.id.clone(),
                        name: check.name.clone(),
                        conclusion: check.conclusion.clone(),
                        url: check.url.clone(),
                        workflow_run_id: check.workflow_run_id,
                        excerpt: check.excerpt.clone(),
                    })
                    .collect(),
            }),
            rerun: proposal.rerun,
            summary_html: proposal.summary_html.clone(),
            investigation_id: proposal.investigation_id.clone(),
            other_patches: proposal.other_patches.iter().map(Patch::from).collect(),
            open_pull_request: proposal.open_pull_request.as_ref().map(|open| NewPullRequest {
                repository: open.repository.clone(),
                base: open.base.clone(),
                branch: open.branch.clone(),
                title: open.title.clone(),
                body: open.body.clone(),
                body_html: open.body_html.clone(),
                patch: open.patch.as_ref().map(Patch::from),
            }),
        }
    }
}

impl From<&proto::Comment> for Comment {
    fn from(comment: &proto::Comment) -> Comment {
        Comment {
            id: comment.id.clone(),
            author: comment.author.clone(),
            body: comment.body.clone(),
            url: comment.url.clone(),
            body_html: comment.body_html.clone(),
            path: comment.path.clone(),
            line: comment.line,
            diff_hunk: comment.diff_hunk.clone(),
        }
    }
}

impl From<&proto::Patch> for Patch {
    fn from(patch: &proto::Patch) -> Patch {
        Patch {
            diff: patch.diff.clone(),
            commit_message: patch.commit_message.clone(),
            head_oid: patch.head_oid.clone(),
            pull_request_url: patch.pull_request_url.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use ts_rs::{Config, TS};

    use super::*;

    // The frontend's types are these, as ts-rs renders them, in one file. A 64-bit integer is a
    // `number`, as the frontend's reader reads one. Run with UPDATE_VIEW_TYPES set, this writes
    // the file instead.
    #[test]
    fn the_frontend_view_types_are_current() {
        let cfg = Config::new().with_large_int("number");
        let types = [
            Moment::decl(&cfg),
            Cluster::decl(&cfg),
            PullRequest::decl(&cfg),
            Checks::decl(&cfg),
            Repository::decl(&cfg),
            Agent::decl(&cfg),
            AgentState::decl(&cfg),
            MergeBy::decl(&cfg),
            Proposal::decl(&cfg),
            Comment::decl(&cfg),
            Patch::decl(&cfg),
            CheckFailure::decl(&cfg),
            FailedCheck::decl(&cfg),
            NewPullRequest::decl(&cfg),
            FileDiff::decl(&cfg),
            Event::decl(&cfg),
            EventKind::decl(&cfg),
        ];
        let file = format!(
            "// Generated by ts-rs from server/src/view/types.rs; don't edit it. After changing\n\
             // those structs, regenerate it from server/ with:\n\
             //\n\
             //   UPDATE_VIEW_TYPES=1 cargo test view_types\n\
             \n{}",
            types.map(|decl| format!("export {decl}\n")).join("\n"),
        );
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../frontend/view-types.ts");
        if std::env::var_os("UPDATE_VIEW_TYPES").is_some() {
            std::fs::write(&path, &file).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        assert!(
            committed == file,
            "frontend/view-types.ts is stale: regenerate it from server/ with \
             `UPDATE_VIEW_TYPES=1 cargo test view_types`"
        );
    }
}
