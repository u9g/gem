// What happened on each open pull request, from its GitHub timeline: its comments, reviews and
// review requests, and its draft, open and closed state changing. The Activity screen shows them
// across every PR. Asked for ACTIVITY_CHUNK PRs a request, all at once: a search page carrying
// each PR's timeline would pass GitHub's ~10s GraphQL timeout.

use std::collections::HashMap;

use gem_github::gh_partial_json;
use pbjson_types::Timestamp;
use serde::Deserialize;
use serde_json::json;

use crate::{
    SearchedPullRequest,
    proto::{Event, EventKind},
};

const ACTIVITY_CHUNK: usize = 10;

// The last EVENTS of each PR, oldest first. `who` and `at` are aliased so each kind of item reads
// the same; a Team's `login` is its slug.
const EVENTS: usize = 100;
const QUERY: &str = r#"
query($ids: [ID!]!) {
  nodes(ids: $ids) {
    ... on PullRequest {
      id
      timelineItems(last: 100, itemTypes: [
        ISSUE_COMMENT, PULL_REQUEST_REVIEW, READY_FOR_REVIEW_EVENT, CONVERT_TO_DRAFT_EVENT,
        REVIEW_REQUESTED_EVENT, REVIEW_REQUEST_REMOVED_EVENT, REVIEW_DISMISSED_EVENT,
        REOPENED_EVENT, CLOSED_EVENT
      ]) {
        nodes {
          __typename
          ... on IssueComment { id url who: author { login } body at: createdAt }
          ... on PullRequestReview {
            id url who: author { login } body state at: submittedAt
            comments(first: 1) { totalCount nodes { body } }
          }
          ... on ReadyForReviewEvent { id who: actor { login } at: createdAt }
          ... on ConvertToDraftEvent { id who: actor { login } at: createdAt }
          ... on ReviewRequestedEvent {
            id who: actor { login } at: createdAt
            requestedReviewer { ... on User { login } ... on Bot { login } ... on Team { login: slug } }
          }
          ... on ReviewRequestRemovedEvent {
            id who: actor { login } at: createdAt
            requestedReviewer { ... on User { login } ... on Bot { login } ... on Team { login: slug } }
          }
          ... on ReviewDismissedEvent { id who: actor { login } at: createdAt }
          ... on ReopenedEvent { id who: actor { login } at: createdAt }
          ... on ClosedEvent { id who: actor { login } at: createdAt }
        }
      }
    }
  }
}
"#;

#[derive(Deserialize)]
struct Response {
    data: Data,
}

#[derive(Deserialize)]
struct Data {
    // A node that errored, e.g. in a repository behind SAML enforcement, is null.
    nodes: Vec<Option<Node>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Node {
    id: String,
    timeline_items: Items,
}

#[derive(Deserialize)]
struct Items {
    nodes: Vec<Item>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    #[serde(rename = "__typename")]
    typename: String,
    id: String,
    #[serde(default)]
    url: String,
    // A deleted account is null.
    who: Option<Login>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    state: String,
    at: Option<Timestamp>,
    comments: Option<Comments>,
    requested_reviewer: Option<Login>,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Comments {
    total_count: u32,
    nodes: Vec<Body>,
}

#[derive(Deserialize)]
struct Body {
    body: String,
}

// Sets each PR's `events`. A PR whose request fails keeps none.
pub(crate) async fn fill(prs: &mut [SearchedPullRequest]) {
    let mut requests = tokio::task::JoinSet::new();
    for chunk in prs.chunks(ACTIVITY_CHUNK) {
        let ids = json!(chunk.iter().map(|pr| &pr.id).collect::<Vec<_>>()).to_string();
        requests.spawn(async move {
            gh_partial_json::<Response>("activity", &[
                "api", "graphql", "-f", &format!("query={QUERY}"), "-F", &format!("ids={ids}"),
            ])
            .await
        });
    }
    let mut events: HashMap<String, Vec<Event>> = HashMap::new();
    while let Some(result) = requests.join_next().await {
        let Ok(Some(response)) = result else {
            continue;
        };
        for node in response.data.nodes.into_iter().flatten() {
            let items = node.timeline_items.nodes.into_iter().filter_map(event);
            events.insert(node.id, items.take(EVENTS).collect());
        }
    }
    for pr in prs {
        if let Some(events) = events.remove(&pr.id) {
            pr.events = events;
        }
    }
}

// A review the viewer has not submitted is PENDING, and is nothing yet.
fn event(item: Item) -> Option<Event> {
    let kind = match (item.typename.as_str(), item.state.as_str()) {
        ("IssueComment", _) => EventKind::Comment,
        ("PullRequestReview", "APPROVED") => EventKind::Approved,
        ("PullRequestReview", "CHANGES_REQUESTED") => EventKind::ChangesRequested,
        ("PullRequestReview", "PENDING") => return None,
        ("PullRequestReview", _) => EventKind::Reviewed,
        ("ReadyForReviewEvent", _) => EventKind::ReadyForReview,
        ("ConvertToDraftEvent", _) => EventKind::ConvertedToDraft,
        ("ReviewRequestedEvent", _) => EventKind::ReviewRequested,
        ("ReviewRequestRemovedEvent", _) => EventKind::ReviewRequestRemoved,
        ("ReviewDismissedEvent", _) => EventKind::ReviewDismissed,
        ("ReopenedEvent", _) => EventKind::Reopened,
        ("ClosedEvent", _) => EventKind::Closed,
        _ => return None,
    };
    let comments = item.comments.as_ref().map_or(0, |c| c.total_count);
    let body = if let Some(reviewer) = item.requested_reviewer {
        reviewer.login
    } else if item.body.is_empty() {
        (item.comments.into_iter().flat_map(|c| c.nodes).next()).map_or(String::new(), |c| c.body)
    } else {
        item.body
    };
    Some(Event {
        id: item.id,
        kind: kind as i32,
        actor: item.who.map_or_else(|| "ghost".to_string(), |who| who.login),
        at: item.at,
        body,
        url: item.url,
        comments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(json: serde_json::Value) -> Option<Event> {
        event(serde_json::from_value(json).unwrap())
    }

    #[test]
    fn each_kind_of_item_is_an_event() {
        let comment = item(json!({
            "__typename": "IssueComment", "id": "c1", "url": "https://github.com/o/r/pull/1#issuecomment-1",
            "who": {"login": "theo"}, "body": "Looks right", "at": "2026-11-09T18:34:29Z"
        }))
        .unwrap();
        assert_eq!(comment.kind, EventKind::Comment as i32);
        assert_eq!(comment.actor, "theo");
        assert_eq!(comment.body, "Looks right");
        assert_eq!(comment.at.unwrap().seconds, 1_794_249_269);

        // A review that only left inline comments carries the first.
        let review = item(json!({
            "__typename": "PullRequestReview", "id": "r1", "url": "u", "who": {"login": "theo"},
            "body": "", "state": "COMMENTED", "at": "2026-11-09T18:34:29Z",
            "comments": {"totalCount": 2, "nodes": [{"body": "nit: rename"}]}
        }))
        .unwrap();
        assert_eq!(review.kind, EventKind::Reviewed as i32);
        assert_eq!((review.body.as_str(), review.comments), ("nit: rename", 2));

        let approved = item(json!({
            "__typename": "PullRequestReview", "id": "r2", "url": "u", "who": null, "body": "LGTM",
            "state": "APPROVED", "at": "2026-11-09T18:34:29Z", "comments": {"totalCount": 0, "nodes": []}
        }))
        .unwrap();
        assert_eq!((approved.kind, approved.actor.as_str()), (EventKind::Approved as i32, "ghost"));

        let requested = item(json!({
            "__typename": "ReviewRequestedEvent", "id": "e1", "who": {"login": "u9g"},
            "at": "2026-11-09T18:34:29Z", "requestedReviewer": {"login": "theo"}
        }))
        .unwrap();
        assert_eq!((requested.kind, requested.body.as_str()), (EventKind::ReviewRequested as i32, "theo"));

        let pending = item(json!({
            "__typename": "PullRequestReview", "id": "r3", "url": "u", "who": {"login": "u9g"},
            "body": "", "state": "PENDING", "at": null, "comments": {"totalCount": 0, "nodes": []}
        }));
        assert!(pending.is_none());
    }
}
