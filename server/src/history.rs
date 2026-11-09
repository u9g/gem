use std::sync::Arc;

use tokio::{sync::Semaphore, task::JoinSet};

use crate::proto::{PullRequestEvent, PullRequestHistory};

// gh processes asking GitHub at once.
const CONCURRENT: usize = 8;

// About one rate-limit point per PR. A review's state is PENDING only to its author, while it is
// a draft, so drafts are left out.
const QUERY: &str = r#"
query($url: URI!) {
  resource(url: $url) {
    ... on PullRequest {
      timelineItems(last: 100, itemTypes: [PULL_REQUEST_COMMIT, HEAD_REF_FORCE_PUSHED_EVENT, PULL_REQUEST_REVIEW, ISSUE_COMMENT, REVIEW_REQUESTED_EVENT, REVIEW_DISMISSED_EVENT, READY_FOR_REVIEW_EVENT, CONVERT_TO_DRAFT_EVENT, CLOSED_EVENT, REOPENED_EVENT, MERGED_EVENT, BASE_REF_CHANGED_EVENT, AUTO_MERGE_ENABLED_EVENT, AUTO_MERGE_DISABLED_EVENT]) {
        nodes {
          __typename
          ... on PullRequestCommit { url commit { abbreviatedOid messageHeadline committedDate author { name user { login } } statusCheckRollup { state } } }
          ... on HeadRefForcePushedEvent { actor { login } createdAt afterCommit { abbreviatedOid statusCheckRollup { state } } }
          ... on PullRequestReview { author { login } state submittedAt createdAt bodyHTML url comments(first: 30) { nodes { path bodyHTML url } } }
          ... on IssueComment { author { login } createdAt bodyHTML url }
          ... on ReviewRequestedEvent { actor { login } createdAt requestedReviewer { ... on User { login } ... on Bot { login } ... on Team { name } } }
          ... on ReviewDismissedEvent { actor { login } createdAt review { author { login } } }
          ... on ReadyForReviewEvent { actor { login } createdAt }
          ... on ConvertToDraftEvent { actor { login } createdAt }
          ... on ClosedEvent { actor { login } createdAt }
          ... on ReopenedEvent { actor { login } createdAt }
          ... on MergedEvent { actor { login } createdAt }
          ... on BaseRefChangedEvent { actor { login } createdAt previousRefName currentRefName }
          ... on AutoMergeEnabledEvent { actor { login } createdAt }
          ... on AutoMergeDisabledEvent { actor { login } createdAt }
        }
      }
    }
  }
}
"#;

// Each timeline item as a PullRequestEvent in proto3 JSON.
const JQ: &str = r#"[.data.resource.timelineItems.nodes[] | select(.state != "PENDING") | {
  kind: .__typename,
  actor: ((.actor // .author // .commit.author.user).login // .commit.author.name // "ghost"),
  at: (.submittedAt // .createdAt // .commit.committedDate),
  state: (.state // (.commit // .afterCommit).statusCheckRollup.state // ""),
  bodyHtml: (.bodyHTML // ""),
  url: (.url // ""),
  text: (if .currentRefName then "\(.previousRefName) → \(.currentRefName)"
    else .commit.messageHeadline // (.requestedReviewer | .login // .name) // .review.author.login // "" end),
  oid: ((.commit // .afterCommit).abbreviatedOid // ""),
  comments: [.comments.nodes[]? | {path, bodyHtml: .bodyHTML, url}]
}]"#;

// Each PR's history, in the order of `urls`. A PR GitHub gives none for carries why, and the
// others still have theirs.
pub async fn of(urls: Vec<String>) -> Vec<PullRequestHistory> {
    let permits = Arc::new(Semaphore::new(CONCURRENT));
    let mut fetches = JoinSet::new();
    for (i, url) in urls.into_iter().enumerate() {
        let permits = permits.clone();
        fetches.spawn(async move {
            let _permit = permits.acquire().await;
            let history = match fetch(&url).await {
                Ok(events) => PullRequestHistory { url, events, error: String::new() },
                Err(error) => PullRequestHistory { url, events: Vec::new(), error },
            };
            (i, history)
        });
    }
    let mut histories = fetches.join_all().await;
    histories.sort_by_key(|(i, _)| *i);
    histories.into_iter().map(|(_, history)| history).collect()
}

async fn fetch(url: &str) -> Result<Vec<PullRequestEvent>, String> {
    let query = format!("query={QUERY}");
    let url = format!("url={url}");
    let stdout = gem_github::command(
        "gh",
        &["api", "graphql", "-f", &query, "-f", &url, "--jq", JQ],
        None,
    )
    .await?;
    serde_json::from_slice(&stdout).map_err(|error| format!("cannot read GitHub's history: {error}"))
}

#[cfg(test)]
mod tests {
    use std::{io::Write, process::Stdio};

    use super::*;

    // One timeline item of each type the query asks for, as GitHub answers.
    const RESPONSE: &str = r#"{"data":{"resource":{"timelineItems":{"nodes":[
      {"__typename":"PullRequestCommit","url":"u/c","commit":{"abbreviatedOid":"1ff531d","messageHeadline":"feat: x","committedDate":"2026-11-01T00:00:00Z","author":{"name":"Jason","user":{"login":"u9g"}},"statusCheckRollup":{"state":"FAILURE"}}},
      {"__typename":"PullRequestCommit","url":"u/c2","commit":{"abbreviatedOid":"2aa","messageHeadline":"wip","committedDate":"2026-11-01T00:01:00Z","author":{"name":"Someone","user":null},"statusCheckRollup":null}},
      {"__typename":"HeadRefForcePushedEvent","actor":{"login":"u9g"},"createdAt":"2026-11-02T00:00:00Z","afterCommit":{"abbreviatedOid":"3bb","statusCheckRollup":{"state":"SUCCESS"}}},
      {"__typename":"PullRequestReview","author":{"login":"rev"},"state":"APPROVED","submittedAt":"2026-11-03T00:00:00Z","createdAt":"2026-11-02T23:00:00Z","bodyHTML":"<p>lgtm</p>","url":"u/r","comments":{"nodes":[{"path":"a.rs","bodyHTML":"<p>nit</p>","url":"u/rc"}]}},
      {"__typename":"PullRequestReview","author":{"login":"u9g"},"state":"PENDING","submittedAt":null,"createdAt":"2026-11-03T00:00:00Z","bodyHTML":"","url":"u/p","comments":{"nodes":[]}},
      {"__typename":"IssueComment","author":null,"createdAt":"2026-11-04T00:00:00Z","bodyHTML":"<p>hi</p>","url":"u/i"},
      {"__typename":"ReviewRequestedEvent","actor":{"login":"u9g"},"createdAt":"2026-11-05T00:00:00Z","requestedReviewer":{"name":"core"}},
      {"__typename":"ReviewDismissedEvent","actor":{"login":"u9g"},"createdAt":"2026-11-06T00:00:00Z","review":{"author":{"login":"rev"}}},
      {"__typename":"BaseRefChangedEvent","actor":{"login":"u9g"},"createdAt":"2026-11-07T00:00:00Z","previousRefName":"u9g/a","currentRefName":"main"},
      {"__typename":"MergedEvent","actor":{"login":"u9g"},"createdAt":"2026-11-08T00:00:00Z"}
    ]}}}}"#;

    #[test]
    fn reads_each_kind_of_timeline_item() {
        let mut jq = std::process::Command::new("jq")
            .args(["-c", JQ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("cannot run jq");
        jq.stdin.take().unwrap().write_all(RESPONSE.as_bytes()).unwrap();
        let output = jq.wait_with_output().unwrap();
        assert!(output.status.success());
        let events: Vec<PullRequestEvent> = serde_json::from_slice(&output.stdout).unwrap();
        let shown: Vec<(&str, &str, i64, &str, &str, &str, usize)> = (events.iter())
            .map(|e| {
                let at = e.at.as_ref().unwrap().seconds;
                let (kind, actor, state) = (e.kind.as_str(), e.actor.as_str(), e.state.as_str());
                (kind, actor, at, state, e.text.as_str(), e.oid.as_str(), e.comments.len())
            })
            .collect();
        let day = |d: i64| 1_793_491_200 + (d - 1) * 86_400;
        assert_eq!(shown, [
            ("PullRequestCommit", "u9g", day(1), "FAILURE", "feat: x", "1ff531d", 0),
            ("PullRequestCommit", "Someone", day(1) + 60, "", "wip", "2aa", 0),
            ("HeadRefForcePushedEvent", "u9g", day(2), "SUCCESS", "", "3bb", 0),
            ("PullRequestReview", "rev", day(3), "APPROVED", "", "", 1),
            ("IssueComment", "ghost", day(4), "", "", "", 0),
            ("ReviewRequestedEvent", "u9g", day(5), "", "core", "", 0),
            ("ReviewDismissedEvent", "u9g", day(6), "", "rev", "", 0),
            ("BaseRefChangedEvent", "u9g", day(7), "", "u9g/a → main", "", 0),
            ("MergedEvent", "u9g", day(8), "", "", "", 0),
        ]);
        assert_eq!(events[3].body_html, "<p>lgtm</p>");
        assert_eq!(events[3].comments[0].path, "a.rs");
    }
}
