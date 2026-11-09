// Each PR's timeline, as GitHub's Conversation tab lists it, for the activity screen.

use std::{collections::HashMap, sync::LazyLock};

use pbjson_types::Timestamp;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::{SearchedPullRequest, gh, proto::Event};

// Fetched only for PRs whose updatedAt moved, which any comment, review, push or state change
// does. The last 50 items, and each review's first 50 comments: GitHub's node limit, not the
// user's history, caps a chunk.
const TIMELINES_QUERY: &str = r#"
query($ids: [ID!]!) {
  nodes(ids: $ids) {
    ... on PullRequest {
      url
      timelineItems(last: 50, itemTypes: [ISSUE_COMMENT, PULL_REQUEST_REVIEW, PULL_REQUEST_COMMIT, HEAD_REF_FORCE_PUSHED_EVENT, READY_FOR_REVIEW_EVENT, CONVERT_TO_DRAFT_EVENT, REVIEW_REQUESTED_EVENT, REVIEW_DISMISSED_EVENT, BASE_REF_CHANGED_EVENT, RENAMED_TITLE_EVENT, CLOSED_EVENT, REOPENED_EVENT]) {
        nodes {
          __typename
          ... on IssueComment { id author { login } createdAt body bodyHTML url }
          ... on PullRequestReview { id author { login } submittedAt state body bodyHTML url
            comments(first: 50) { nodes { id author { login } createdAt body bodyHTML url path } } }
          ... on PullRequestCommit { id commit { abbreviatedOid messageHeadline committedDate url author { user { login } name } } }
          ... on HeadRefForcePushedEvent { id actor { login } createdAt afterCommit { abbreviatedOid } }
          ... on ReadyForReviewEvent { id actor { login } createdAt }
          ... on ConvertToDraftEvent { id actor { login } createdAt }
          ... on ReviewRequestedEvent { id actor { login } createdAt requestedReviewer { ... on User { login } ... on Team { name } } }
          ... on ReviewDismissedEvent { id actor { login } createdAt dismissalMessage url }
          ... on BaseRefChangedEvent { id actor { login } createdAt currentRefName }
          ... on RenamedTitleEvent { id actor { login } createdAt currentTitle }
          ... on ClosedEvent { id actor { login } createdAt }
          ... on ReopenedEvent { id actor { login } createdAt }
        }
      }
    }
  }
}
"#;

// Each item as an `Event`, in GitHub's order, a review's comments right after it. A review the
// user has not submitted yet is theirs alone.
const TIMELINES_JQ: &str = r#"
def event(kind; fields): {
  id, kind: ("EVENT_KIND_" + kind), at: .createdAt, body: "", body_html: "", url: "", detail: "",
  actor: (.author.login // .actor.login // .commit.author.user.login // .commit.author.name // "ghost")
} + fields;
.data.nodes[] | {url, events: [.timelineItems.nodes[]
  | if .__typename == "IssueComment" then event("COMMENT"; {body, body_html: .bodyHTML, url})
    elif .__typename == "PullRequestReview" and .state != "PENDING" then
      event("REVIEW"; {at: .submittedAt, body, body_html: .bodyHTML, url, detail: .state}),
      (.comments.nodes[] | event("REVIEW_COMMENT"; {body, body_html: .bodyHTML, url, detail: .path}))
    elif .__typename == "PullRequestCommit" then
      event("COMMIT"; {at: .commit.committedDate, body: .commit.messageHeadline, url: .commit.url, detail: .commit.abbreviatedOid})
    elif .__typename == "HeadRefForcePushedEvent" then event("FORCE_PUSH"; {detail: (.afterCommit.abbreviatedOid // "")})
    elif .__typename == "ReadyForReviewEvent" then event("READY_FOR_REVIEW"; {})
    elif .__typename == "ConvertToDraftEvent" then event("DRAFT"; {})
    elif .__typename == "ReviewRequestedEvent" then event("REVIEW_REQUESTED"; {detail: (.requestedReviewer.login // .requestedReviewer.name // "")})
    elif .__typename == "ReviewDismissedEvent" then event("REVIEW_DISMISSED"; {body: (.dismissalMessage // ""), url: (.url // "")})
    elif .__typename == "BaseRefChangedEvent" then event("RETARGETED"; {detail: .currentRefName})
    elif .__typename == "RenamedTitleEvent" then event("RENAMED"; {detail: .currentTitle})
    elif .__typename == "ClosedEvent" then event("CLOSED"; {})
    elif .__typename == "ReopenedEvent" then event("REOPENED"; {})
    else empty end]}"#;

#[derive(Deserialize)]
struct Fetched {
    url: String,
    events: Vec<Event>,
}

// Each open PR's timeline, by URL, with the PR's updatedAt when it was fetched.
static KNOWN: LazyLock<Mutex<HashMap<String, (Timestamp, Vec<Event>)>>> =
    LazyLock::new(Default::default);

const CHUNK: usize = 20;

// Sets each PR's events: its timeline as last fetched, fetched again once the PR updated since. A
// PR whose fetch fails keeps the timeline it showed.
pub async fn fill(prs: &mut [SearchedPullRequest]) {
    let mut known = KNOWN.lock().await;
    let stale: Vec<&SearchedPullRequest> = (prs.iter())
        .filter(|pr| known.get(&pr.url).is_none_or(|(at, _)| *at != pr.updated_at))
        .collect();
    let mut requests = tokio::task::JoinSet::new();
    for chunk in stale.chunks(CHUNK) {
        let ids: Vec<String> = chunk.iter().map(|pr| pr.id.clone()).collect();
        requests.spawn(async move { fetch(&ids).await });
    }
    let updated: HashMap<&str, &Timestamp> =
        stale.iter().map(|pr| (pr.url.as_str(), &pr.updated_at)).collect();
    while let Some(Ok(fetched)) = requests.join_next().await {
        for pr in fetched {
            let Some(&at) = updated.get(pr.url.as_str()) else { continue };
            known.insert(pr.url, (at.clone(), pr.events));
        }
    }
    known.retain(|url, _| prs.iter().any(|pr| pr.url == *url));
    for pr in prs {
        pr.events = known.get(&pr.url).map(|(_, events)| events.clone()).unwrap_or_default();
    }
}

// The timelines of the PRs with node IDs `ids`. gh exits non-zero when any one node errors, e.g.
// a PR in a repository behind SAML enforcement the token is not authorized for, and then prints the
// raw response instead of applying --jq, so a chunk that fails is asked for one PR at a time.
async fn fetch(ids: &[String]) -> Vec<Fetched> {
    if let Some(fetched) = fetch_chunk(ids).await {
        return fetched;
    }
    let mut fetched = Vec::new();
    if ids.len() > 1 {
        for id in ids {
            fetched.extend(fetch_chunk(std::slice::from_ref(id)).await.into_iter().flatten());
        }
    }
    fetched
}

async fn fetch_chunk(ids: &[String]) -> Option<Vec<Fetched>> {
    let mut args = vec![
        "api".to_string(),
        "graphql".to_string(),
        "-f".to_string(),
        format!("query={TIMELINES_QUERY}"),
        "--jq".to_string(),
        TIMELINES_JQ.to_string(),
    ];
    for id in ids {
        args.extend(["-f".to_string(), format!("ids[]={id}")]);
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let stdout = gh("timelines", &args).await?;
    serde_json::Deserializer::from_slice(&stdout)
        .into_iter::<Fetched>()
        .collect::<Result<_, _>>()
        .inspect_err(|error| tracing::warn!(%error, "cannot read timelines"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::EventKind;

    // A line as the jq prints it reads as the proto's events: kinds by their names, times from
    // RFC 3339, and fields by their snake_case names.
    #[test]
    fn the_jq_output_reads_as_events() {
        let line = r#"{"url":"https://github.com/o/r/pull/1","events":[
          {"id":"c1","kind":"EVENT_KIND_COMMIT","at":"2026-11-09T18:32:16Z","body":"Fix it","body_html":"","url":"https://github.com/o/r/commit/df8e751","detail":"df8e751","actor":"u9g"},
          {"id":"r1","kind":"EVENT_KIND_REVIEW","at":"2026-11-09T18:40:00Z","body":"LGTM","body_html":"<p>LGTM</p>","url":"","detail":"APPROVED","actor":"theo"},
          {"id":"e1","kind":"EVENT_KIND_REOPENED","at":"2026-11-09T18:41:00Z","body":"","body_html":"","url":"","detail":"","actor":"ghost"}
        ]}"#;
        let fetched: Fetched = serde_json::from_str(line).unwrap();
        let kinds: Vec<EventKind> =
            fetched.events.iter().map(|e| EventKind::try_from(e.kind).unwrap()).collect();
        assert_eq!(kinds, [EventKind::Commit, EventKind::Review, EventKind::Reopened]);
        assert_eq!(fetched.events[0].at.as_ref().unwrap().seconds, 1_794_249_136);
        assert_eq!(fetched.events[1].body_html, "<p>LGTM</p>");
        assert_eq!(fetched.events[1].detail, "APPROVED");
    }
}
