// What omnidiff makes of each diff the view shows, for frontends to mark the parts of lines that
// changed rather than whole lines. Each diff is diffed once, in the background, and kept for as
// long as it's shown: a snapshot never waits on omnidiff, and shows a diff's highlights from the
// first snapshot after they're ready.

use std::{
    collections::HashMap,
    sync::LazyLock,
    time::{Duration, Instant},
};

use gem_omnidiff::{Change, Span, Summary};
use sha1::{Digest, Sha1};
use tokio::sync::Mutex;

use crate::proto::{
    DiffHighlights, DiffSummary, FileHighlights, ListOpenPullRequestsResponse, Proposal,
    SideHighlights,
};

// How long a diff omnidiff failed on waits before it's diffed again, as once omnidiff is installed.
const RETRY: Duration = Duration::from_secs(10 * 60);

enum Diffed {
    Diffing,
    Done(DiffHighlights),
    Failed(Instant),
}

// Each diff shown, by its key.
static DIFFED: LazyLock<Mutex<HashMap<String, Diffed>>> = LazyLock::new(Default::default);

// Gives the response the highlights of each diff it shows that omnidiff has diffed, and starts
// diffing the rest.
pub async fn fill(response: &mut ListOpenPullRequestsResponse) {
    let shown: HashMap<String, String> = shown(response)
        .into_iter()
        .map(|diff| (key(&diff), diff))
        .collect();
    let mut diffed = DIFFED.lock().await;
    diffed.retain(|key, _| shown.contains_key(key));
    for (key, diff) in shown {
        match diffed.get(&key) {
            Some(Diffed::Done(highlights)) => {
                response.diffs.insert(key, highlights.clone());
                continue;
            }
            Some(Diffed::Diffing) => continue,
            Some(Diffed::Failed(at)) if at.elapsed() < RETRY => continue,
            _ => {}
        }
        diffed.insert(key.clone(), Diffed::Diffing);
        tokio::spawn(async move {
            let done = match gem_omnidiff::highlight(&diff).await {
                Ok(files) => Diffed::Done(DiffHighlights {
                    files: files.into_iter().map(file).collect(),
                }),
                Err(error) => {
                    warn(&error);
                    Diffed::Failed(Instant::now())
                }
            };
            // Unless it stopped showing meanwhile.
            if let Some(entry) = DIFFED.lock().await.get_mut(&key) {
                *entry = done;
            }
        });
    }
}

// A diff's key in the view: the SHA-1 of its text in lowercase hex, as frontends compute it.
pub fn key(diff: &str) -> String {
    Sha1::digest(diff.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// Hunks without a file header, as GitHub gives a file's and a review comment's, under one, so
// omnidiff knows the file's language by its name. The frontend shows them the same way.
fn with_header(path: &str, hunks: &str) -> String {
    format!("--- a/{path}\n+++ b/{path}\n{hunks}")
}

// Every diff the clusters and agents show.
fn shown(response: &ListOpenPullRequestsResponse) -> Vec<String> {
    let mut diffs = Vec::new();
    for pr in response
        .clusters
        .iter()
        .flat_map(|cluster| &cluster.pull_requests)
    {
        let files = pr.files.iter().filter(|file| !file.patch.is_empty());
        diffs.extend(files.map(|file| with_header(&file.path, &file.patch)));
        diffs.extend(pr.proposals.iter().flat_map(proposal_diffs));
    }
    let agents = response.clusters.iter().flat_map(|cluster| &cluster.agents);
    let proposals = agents
        .chain(&response.agents)
        .flat_map(|agent| &agent.proposals);
    diffs.extend(proposals.flat_map(proposal_diffs));
    diffs
}

// The proposal's patches, and the code its review comment is on.
fn proposal_diffs(proposal: &Proposal) -> impl Iterator<Item = String> {
    let opens = proposal
        .open_pull_request
        .iter()
        .flat_map(|open| &open.patch);
    let patches = proposal
        .patch
        .iter()
        .chain(&proposal.other_patches)
        .chain(opens);
    let comments = proposal
        .comment
        .iter()
        .filter(|comment| !comment.diff_hunk.is_empty());
    (patches
        .filter(|patch| !patch.diff.is_empty())
        .map(|patch| patch.diff.clone()))
    .chain(comments.map(|comment| with_header(&comment.path, &comment.diff_hunk)))
}

fn file(file: gem_omnidiff::File) -> FileHighlights {
    let summary = match file.summary {
        None => DiffSummary::Mixed,
        Some(Summary::NoChanges) => DiffSummary::NoChanges,
        Some(Summary::NewFile) => DiffSummary::NewFile,
        Some(Summary::DeletedFile) => DiffSummary::DeletedFile,
        Some(Summary::WhitespaceOnly) => DiffSummary::WhitespaceOnly,
        Some(Summary::CommentsOnly) => DiffSummary::CommentsOnly,
        Some(Summary::MovesOnly) => DiffSummary::MovesOnly,
    };
    FileHighlights {
        path: file.path,
        summary: summary.into(),
        before: Some(side(&file.before)),
        after: Some(side(&file.after)),
    }
}

fn side(spans: &[Span]) -> SideHighlights {
    let mut side = SideHighlights::default();
    for span in spans {
        let list = if span.change == Change::Moved {
            &mut side.moved
        } else {
            &mut side.changed
        };
        list.extend([span.line, span.start, span.end]);
    }
    side
}

// Logs a failure unless it's the one logged last, so a missing omnidiff is told once, not for
// every diff.
fn warn(error: &str) {
    static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let mut last = LAST.lock().unwrap_or_else(|error| error.into_inner());
    if *last != error {
        tracing::warn!(%error, "cannot diff with omnidiff; diffs show without its highlights");
        *last = error.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{
        Agent, Comment, FileDiff, NewPullRequest, Patch, PullRequest, PullRequestCluster,
    };

    fn patch(diff: &str) -> Option<Patch> {
        Some(Patch {
            diff: diff.to_string(),
            ..Default::default()
        })
    }

    #[test]
    fn a_key_is_the_sha1_of_the_diff_in_hex() {
        assert_eq!(key(""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(key("abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn every_diff_shown_is_diffed_once_and_hunks_get_their_files_header() {
        let comment = Comment {
            path: "src/a.rs".to_string(),
            diff_hunk: "@@ -1 +1 @@\n-a\n+b".to_string(),
            ..Default::default()
        };
        let proposal = Proposal {
            comment: Some(comment),
            patch: patch("one"),
            other_patches: vec![Patch::default(), patch("two").unwrap()],
            open_pull_request: Some(NewPullRequest {
                patch: patch("three"),
                ..Default::default()
            }),
            ..Default::default()
        };
        let files = vec![
            FileDiff {
                path: "src/a.rs".to_string(),
                patch: "@@ -1 +1 @@\n-x\n+y".to_string(),
            },
            // Too large for GitHub to diff.
            FileDiff {
                path: "big.json".to_string(),
                patch: String::new(),
            },
        ];
        let agent = |diff| Agent {
            proposals: vec![Proposal {
                patch: patch(diff),
                ..Default::default()
            }],
            ..Default::default()
        };
        let pr = PullRequest {
            files,
            proposals: vec![proposal],
            ..Default::default()
        };
        let cluster = PullRequestCluster {
            pull_requests: vec![pr],
            agents: vec![agent("four")],
            ..Default::default()
        };
        let response = ListOpenPullRequestsResponse {
            clusters: vec![cluster],
            agents: vec![agent("five")],
            ..Default::default()
        };
        assert_eq!(
            shown(&response),
            [
                "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-x\n+y",
                "one",
                "two",
                "three",
                "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-a\n+b",
                "four",
                "five",
            ]
        );
    }

    #[test]
    fn spans_are_kept_as_triples_moved_apart_from_changed() {
        let span = |line, start, end, change| Span {
            line,
            start,
            end,
            change,
        };
        let side = side(&[
            span(3, 0, 4, Change::Updated),
            span(3, 8, 12, Change::Moved),
            span(9, 2, 5, Change::Inserted),
        ]);
        assert_eq!(side.changed, [3, 0, 4, 9, 2, 5]);
        assert_eq!(side.moved, [3, 8, 12]);
    }
}
