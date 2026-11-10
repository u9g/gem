// Each open PR's recent events from GitHub's timeline: its reviews, comments and changes of state,
// for the activity screen that shows them across every PR. A PR's events are fetched again
// only when GitHub says it was updated since, so a refresh costs a request per CHUNK PRs that
// changed, and nothing when none did.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use pbjson_types::Timestamp;
use serde_json::Value;

use crate::{
    SearchedPullRequest, gh_partial_json,
    proto::{Event, EventKind},
};

// PRs per request: each with its last 50 items and their rendered bodies, well within GitHub's ~10s GraphQL timeout.
const CHUNK: usize = 10;
// How long a refresh waits on GitHub before showing what it has; a PR that was not answered is
// asked again next time.
const TIMEOUT: Duration = Duration::from_secs(20);

const QUERY: &str = r#"
query($ids: [ID!]!) {
  nodes(ids: $ids) {
    ... on PullRequest {
      id
      timelineItems(last: 50, itemTypes: [
        ISSUE_COMMENT, PULL_REQUEST_REVIEW, REVIEW_DISMISSED_EVENT, READY_FOR_REVIEW_EVENT,
        CONVERT_TO_DRAFT_EVENT, BASE_REF_CHANGED_EVENT, RENAMED_TITLE_EVENT, CLOSED_EVENT,
        REOPENED_EVENT, MERGED_EVENT
      ]) {
        nodes {
          __typename
          ... on IssueComment { id author { login } createdAt url body bodyHTML }
          ... on PullRequestReview {
            id state author { login } createdAt url body bodyHTML
            comments(first: 50) { nodes { id author { login } createdAt url body bodyHTML path } }
          }
          ... on ReviewDismissedEvent { id actor { login } createdAt url }
          ... on ReadyForReviewEvent { id actor { login } createdAt url }
          ... on ConvertToDraftEvent { id actor { login } createdAt url }
          ... on BaseRefChangedEvent { id actor { login } createdAt currentRefName }
          ... on RenamedTitleEvent { id actor { login } createdAt currentTitle }
          ... on ClosedEvent { id actor { login } createdAt url }
          ... on ReopenedEvent { id actor { login } createdAt }
          ... on MergedEvent { id actor { login } createdAt url }
        }
      }
    }
  }
}
"#;

// Each PR's events by URL, with the update time they are as of.
static EVENTS: LazyLock<Mutex<HashMap<String, (Timestamp, Vec<Event>)>>> =
    LazyLock::new(Default::default);

// Fetches the events of the PRs updated since their events were fetched, and forgets the PRs no
// longer listed.
pub async fn refresh(prs: &[SearchedPullRequest]) {
    let stale: Vec<&SearchedPullRequest> = {
        let mut events = EVENTS.lock().unwrap();
        events.retain(|url, _| prs.iter().any(|pr| pr.url == *url));
        (prs.iter())
            .filter(|pr| events.get(&pr.url).is_none_or(|(at, _)| *at != pr.updated_at))
            .collect()
    };
    let mut requests = tokio::task::JoinSet::new();
    for chunk in stale.chunks(CHUNK) {
        // Each `-f ids[]=` adds one ID to the list variable $ids.
        let mut args = vec!["api".to_string(), "graphql".to_string(), "-f".to_string(), format!("query={QUERY}")];
        for pr in chunk {
            args.extend(["-f".to_string(), format!("ids[]={}", pr.id)]);
        }
        requests.spawn(async move {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            gh_partial_json::<Value>("activity", &args).await.map(|data| data["data"]["nodes"].clone())
        });
    }
    let by_id: HashMap<&str, &SearchedPullRequest> =
        stale.iter().map(|pr| (pr.id.as_str(), *pr)).collect();
    let _ = tokio::time::timeout(TIMEOUT, async {
        while let Some(result) = requests.join_next().await {
            let Ok(Some(Value::Array(nodes))) = result else { continue };
            let mut events = EVENTS.lock().unwrap();
            for node in &nodes {
                let Some(pr) = node["id"].as_str().and_then(|id| by_id.get(id)) else { continue };
                events.insert(pr.url.clone(), (pr.updated_at.clone(), parse(node)));
            }
        }
    })
    .await;
}

// The PR's events as last fetched, oldest first; none before the first fetch answers.
pub fn events(url: &str) -> Vec<Event> {
    EVENTS.lock().unwrap().get(url).map(|(_, events)| events.clone()).unwrap_or_default()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

fn time(value: &Value) -> Option<Timestamp> {
    serde_json::from_value(value.clone()).ok()
}

// The PR's timeline items as events, oldest first. A review's comments on lines are in its
// `comments`, as GitHub's Conversation tab shows them inside the review.
fn parse(pr: &Value) -> Vec<Event> {
    let mut events = Vec::new();
    for item in pr["timelineItems"]["nodes"].as_array().into_iter().flatten() {
        let actor = text(&item["actor"]["login"]);
        let mut event = Event {
            id: text(&item["id"]),
            actor: if actor.is_empty() { text(&item["author"]["login"]) } else { actor },
            at: time(&item["createdAt"]),
            url: text(&item["url"]),
            ..Default::default()
        };
        let kind = match item["__typename"].as_str().unwrap_or_default() {
            "IssueComment" => {
                event.body = text(&item["body"]);
                event.body_html = text(&item["bodyHTML"]);
                EventKind::Commented
            }
            "PullRequestReview" => {
                let kind = match item["state"].as_str().unwrap_or_default() {
                    "APPROVED" => EventKind::Approved,
                    "CHANGES_REQUESTED" => EventKind::ChangesRequested,
                    // The user's own review they have not submitted.
                    "PENDING" => continue,
                    _ => EventKind::Reviewed,
                };
                event.body = text(&item["body"]);
                event.body_html = text(&item["bodyHTML"]);
                event.comments = (item["comments"]["nodes"].as_array().into_iter().flatten())
                    .map(|comment| Event {
                        id: text(&comment["id"]),
                        kind: EventKind::ReviewCommented as i32,
                        actor: text(&comment["author"]["login"]),
                        at: time(&comment["createdAt"]),
                        url: text(&comment["url"]),
                        body: text(&comment["body"]),
                        body_html: text(&comment["bodyHTML"]),
                        subject: text(&comment["path"]),
                        ..Default::default()
                    })
                    .collect();
                kind
            }
            "ReviewDismissedEvent" => EventKind::ReviewDismissed,
            "ReadyForReviewEvent" => EventKind::ReadyForReview,
            "ConvertToDraftEvent" => EventKind::ConvertedToDraft,
            "BaseRefChangedEvent" => {
                event.subject = text(&item["currentRefName"]);
                EventKind::Retargeted
            }
            "RenamedTitleEvent" => {
                event.subject = text(&item["currentTitle"]);
                EventKind::Renamed
            }
            "ClosedEvent" => EventKind::Closed,
            "ReopenedEvent" => EventKind::Reopened,
            "MergedEvent" => EventKind::Merged,
            _ => continue,
        };
        event.kind = kind as i32;
        events.push(event);
    }
    events.sort_by_key(|event| event.at.as_ref().map(|at| (at.seconds, at.nanos)));
    events
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_timeline_reads_as_events_oldest_first_with_review_comments_inside_their_review() {
        let pr = json!({ "timelineItems": { "nodes": [
            { "__typename": "RenamedTitleEvent", "id": "E1", "actor": { "login": "me" },
                "createdAt": "2026-11-01T09:00:00Z", "currentTitle": "Retry TTS" },
            { "__typename": "PullRequestReview", "id": "R1", "state": "CHANGES_REQUESTED",
                "author": { "login": "amy" }, "createdAt": "2026-11-01T12:00:00Z", "url": "u/r1",
                "body": "Two things", "bodyHTML": "<p>Two things</p>",
                "comments": { "nodes": [ { "id": "C1", "author": { "login": "amy" },
                    "createdAt": "2026-11-01T11:59:00Z", "url": "u/c1", "body": "nit", "bodyHTML": "<p>nit</p>",
                    "path": "src/tts.py" } ] } },
            { "__typename": "PullRequestReview", "id": "R0", "state": "PENDING", "author": { "login": "me" },
                "createdAt": "2026-11-01T13:00:00Z", "comments": { "nodes": [] } },
            { "__typename": "IssueComment", "id": "I1", "author": { "login": "bot" },
                "createdAt": "2026-11-01T14:00:00Z", "url": "u/i1", "body": "LGTM", "bodyHTML": "<p>LGTM</p>" },
        ] } });
        let events = parse(&pr);
        let kinds: Vec<(i32, &str)> =
            events.iter().map(|e| (e.kind, e.actor.as_str())).collect();
        assert_eq!(kinds, [
            (EventKind::Renamed as i32, "me"),
            (EventKind::ChangesRequested as i32, "amy"),
            (EventKind::Commented as i32, "bot"),
        ]);
        assert_eq!(events[0].subject, "Retry TTS");
        let comments = &events[1].comments;
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].kind, EventKind::ReviewCommented as i32);
        assert_eq!((comments[0].subject.as_str(), comments[0].body_html.as_str()), ("src/tts.py", "<p>nit</p>"));
        assert!(events.iter().all(|event| event.at.is_some()));
    }
}
