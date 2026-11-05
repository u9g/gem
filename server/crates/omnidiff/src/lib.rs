// What omnidiff (https://github.com/ivankovic/omnidiff) makes of a unified diff: within the lines
// each file's diff deletes and adds, the parts that changed and the parts that only moved, as it
// reads both sides by their syntax trees. Where a line diff marks every line a renamed variable is
// on, omnidiff marks the name.
//
// It runs the `omnidiff` binary on PATH in its JSON mode, omnidiff's stable contract; its library's
// API changes between minor versions. omnidiff diffs two whole files, and a unified diff holds only
// its hunks, so each side of a file is rebuilt from them, with a blank line between hunks: both
// sides then hold the same lines apart from what the diff changes, and each row is a line of the
// diff or one of those blanks.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde::Deserialize;
use tokio::{process::Command, sync::Semaphore};

// How many omnidiffs run at once, and how long one may take before it counts as failed. Most
// files take milliseconds.
static RUNNING: Semaphore = Semaphore::const_new(4);
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    // Text only this side has.
    Deleted,
    Inserted,
    // Text both sides have, edited, like a renamed variable.
    Updated,
    // Text the other side has elsewhere.
    Moved,
}

// A changed part of one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    // The line's number on its side, as the hunk headers count them.
    pub line: u32,
    // Where the part starts and ends in the line's text, in UTF-16 code units, as JavaScript
    // indexes a string.
    pub start: u32,
    pub end: u32,
    pub change: Change,
}

// How omnidiff sums up a file's change, when it is all of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Summary {
    NoChanges,
    NewFile,
    DeletedFile,
    WhitespaceOnly,
    CommentsOnly,
    MovesOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    // As the file's header names it: its new path, or its old one when it's deleted.
    pub path: String,
    pub summary: Option<Summary>,
    // In order, on the old side's lines and the new side's.
    pub before: Vec<Span>,
    pub after: Vec<Span>,
}

// The highlights of each file of `diff` that has hunks. A file omnidiff fails on is left out, and
// when it fails on every one, as when it isn't installed, the first failure is returned.
pub async fn highlight(diff: &str) -> Result<Vec<File>, String> {
    let mut files = Vec::new();
    let mut failed = None;
    for piece in pieces(diff).into_iter().filter(|piece| piece.hunks > 0) {
        match run(&piece).await {
            Ok(file) => files.push(file),
            Err(error) => {
                failed.get_or_insert(error);
            }
        }
    }
    match failed {
        Some(error) if files.is_empty() => Err(error),
        _ => Ok(files),
    }
}

// One file of a diff, both its sides rebuilt from its hunks.
#[derive(Debug, Default)]
struct Piece {
    path: String,
    hunks: usize,
    before: Side,
    after: Side,
}

// Each row of a side: a line of the diff, or a blank between hunks.
#[derive(Debug, Default, PartialEq)]
struct Side {
    rows: Vec<Option<Row>>,
}

#[derive(Debug, PartialEq)]
struct Row {
    // Its number on its side.
    line: u32,
    text: String,
    // For a deleted or added line, its run of changed lines between context lines, and where it
    // is among the run's lines on its side: the nth deleted line and the nth added one of a run
    // are a line and what it became.
    changed: Option<(usize, usize)>,
}

impl Side {
    fn text(&self) -> String {
        let rows = self.rows.iter();
        rows.map(|row| row.as_ref().map_or("", |row| row.text.as_str()))
            .map(|text| format!("{text}\n"))
            .collect()
    }

    fn changed(&self, row: usize) -> Option<(usize, usize)> {
        self.rows.get(row)?.as_ref()?.changed
    }
}

// The files of `diff` and their hunks, read as `parseDiff` in frontend/surfaces.tsx reads them, so
// the lines are numbered as the frontend numbers them.
fn pieces(diff: &str) -> Vec<Piece> {
    let lines: Vec<&str> = diff.split('\n').collect();
    let mut pieces: Vec<Piece> = Vec::new();
    let (mut old, mut new) = (0, 0);
    // Lines left in the current hunk on each side, from its header; within them, every line is code.
    let (mut old_left, mut new_left) = (0i64, 0i64);
    // The run of changed lines, and how many deleted and added lines it has so far.
    let (mut run, mut deleted, mut added) = (0, 0, 0);
    for (i, &line) in lines.iter().enumerate() {
        let in_hunk = old_left > 0 || new_left > 0;
        let next = lines.get(i + 1).copied().unwrap_or_default();
        let previous = if i > 0 { lines[i - 1] } else { "" };
        // A file's header is a `--- ` line then a `+++ ` line, between hunks.
        if !in_hunk && line.starts_with("--- ") && next.starts_with("+++ ") {
            continue;
        }
        if !in_hunk && line.starts_with("+++ ") && previous.starts_with("--- ") {
            // A deleted file's new side is /dev/null; its path is on the old side.
            let named = if line == "+++ /dev/null" {
                previous
            } else {
                line
            };
            let path = named.get(6..).unwrap_or_default().to_string();
            pieces.push(Piece {
                path,
                ..Piece::default()
            });
            continue;
        }
        let Some(piece) = pieces.last_mut() else {
            continue;
        };
        // A context line or a hunk ends a run.
        let changes = in_hunk && (line.starts_with(['+', '-', '\\']));
        if !changes && deleted + added > 0 {
            (run, deleted, added) = (run + 1, 0, 0);
        }
        if !in_hunk {
            if let Some((from, from_count, to, to_count)) = hunk_header(line) {
                if piece.hunks > 0 {
                    piece.before.rows.push(None);
                    piece.after.rows.push(None);
                }
                piece.hunks += 1;
                (old, new, old_left, new_left) = (from, to, from_count, to_count);
            }
            continue;
        }
        // "\ No newline at end of file": both sides end in one here.
        if line.starts_with('\\') {
            continue;
        }
        // An empty line is an empty context line whose leading space was stripped.
        let text = line.get(1..).unwrap_or_default().to_string();
        if line.starts_with('+') {
            new_left -= 1;
            let changed = Some((run, added));
            piece.after.rows.push(Some(Row {
                line: new,
                text,
                changed,
            }));
            (new, added) = (new + 1, added + 1);
        } else if line.starts_with('-') {
            old_left -= 1;
            let changed = Some((run, deleted));
            piece.before.rows.push(Some(Row {
                line: old,
                text,
                changed,
            }));
            (old, deleted) = (old + 1, deleted + 1);
        } else {
            (old_left, new_left) = (old_left - 1, new_left - 1);
            let context = |line| {
                Some(Row {
                    line,
                    text: text.clone(),
                    changed: None,
                })
            };
            piece.before.rows.push(context(old));
            piece.after.rows.push(context(new));
            (old, new) = (old + 1, new + 1);
        }
    }
    pieces
}

// The old side's start and count and the new side's, from a hunk's `@@ -a,b +c,d @@` header. A
// count left out is 1.
fn hunk_header(line: &str) -> Option<(u32, i64, u32, i64)> {
    let (old, rest) = line.strip_prefix("@@ -")?.split_once(" +")?;
    let new = rest.split(' ').next()?;
    let range = |range: &str| {
        let (start, count) = range.split_once(',').unwrap_or((range, "1"));
        Some((start.parse().ok()?, count.parse().ok()?))
    };
    let ((from, from_count), (to, to_count)) = (range(old)?, range(new)?);
    Some((from, from_count, to, to_count))
}

// What `omnidiff --mode json` prints, as far as it's read here: see its
// src/tui/json_output.schema.json.
#[derive(Deserialize)]
struct Output {
    before: OutputSide,
    after: OutputSide,
    summary: Option<String>,
}

#[derive(Deserialize)]
struct OutputSide {
    hunks: Vec<OutputHunk>,
}

#[derive(Deserialize)]
struct OutputHunk {
    operation: String,
    range: Range,
    // For a move, where the text is on the other side.
    move_target: Option<Range>,
}

// Rows and byte columns, 0-indexed, from the start up to, not including, the end.
#[derive(Deserialize)]
struct Range {
    start_row: usize,
    start_column: usize,
    end_row: usize,
    end_column: usize,
}

// Runs omnidiff on the piece's two sides, written to files named as the piece's file is, as it
// picks the language by the name.
async fn run(piece: &Piece) -> Result<File, String> {
    let _running = RUNNING.acquire().await.map_err(|e| e.to_string())?;
    let dir = scratch();
    let output = async {
        let name = file_name(&piece.path);
        let (before, after) = (dir.join("before").join(name), dir.join("after").join(name));
        for (path, side) in [(&before, &piece.before), (&after, &piece.after)] {
            tokio::fs::create_dir_all(path.parent().unwrap())
                .await
                .map_err(|e| e.to_string())?;
            tokio::fs::write(path, side.text())
                .await
                .map_err(|e| e.to_string())?;
        }
        let ran = Command::new("omnidiff")
            // Its most faithful reading, whatever its TUI last saved: brackets, separators and
            // reindents that carry no meaning are left unpainted.
            .args(["--mode", "json", "--minimal"])
            .args([&before, &after])
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(TIMEOUT, ran)
            .await
            .map_err(|_| format!("omnidiff took over {TIMEOUT:?} on {}", piece.path))?
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => format!(
                    "omnidiff is not on PATH; install it as \
                     https://github.com/ivankovic/omnidiff#installation says"
                ),
                _ => format!("omnidiff: {e}"),
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("omnidiff on {}: {}", piece.path, stderr.trim()));
        }
        serde_json::from_slice::<Output>(&output.stdout)
            .map_err(|e| format!("omnidiff's output on {}: {e}", piece.path))
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let output = output?;
    Ok(File {
        path: piece.path.clone(),
        summary: output.summary.as_deref().and_then(summary),
        before: spans(&piece.before, &piece.after, &output.before.hunks),
        after: spans(&piece.after, &piece.before, &output.after.hunks),
    })
}

// A directory of its own for one run.
fn scratch() -> PathBuf {
    static RUNS: AtomicU64 = AtomicU64::new(0);
    let run = RUNS.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join("gem-omnidiff")
        .join(format!("{}-{run}", std::process::id()))
}

// The last part of `path`, or a stand-in where it names no file.
fn file_name(path: &str) -> &Path {
    match path.rsplit('/').next() {
        Some(name) if !name.is_empty() && name != "." && name != ".." => Path::new(name),
        _ => Path::new("file"),
    }
}

fn summary(summary: &str) -> Option<Summary> {
    Some(match summary {
        "no_changes" => Summary::NoChanges,
        "new_file" => Summary::NewFile,
        "deleted_file" => Summary::DeletedFile,
        "whitespace_only" => Summary::WhitespaceOnly,
        "comment_only" => Summary::CommentsOnly,
        "refactor_moved_only" => Summary::MovesOnly,
        _ => return None,
    })
}

// omnidiff's ranges on a side, cut into one span per line and placed on the diff's lines. Parts of
// the blanks between hunks are dropped, and so is a move onto what the line became, like the rest
// of a line shifting over when a name on it gets shorter: there it reads as unchanged.
fn spans(side: &Side, other: &Side, hunks: &[OutputHunk]) -> Vec<Span> {
    let mut spans = Vec::new();
    for hunk in hunks {
        let change = match hunk.operation.as_str() {
            "delete" => Change::Deleted,
            "insert" => Change::Inserted,
            "update" => Change::Updated,
            "move" => Change::Moved,
            _ => continue,
        };
        let range = &hunk.range;
        for row in range.start_row..=range.end_row {
            let Some(Some(Row {
                line,
                text,
                changed,
            })) = side.rows.get(row)
            else {
                continue;
            };
            let target = hunk.move_target.as_ref();
            let onto =
                target.and_then(|target| other.changed(target.start_row + row - range.start_row));
            if change == Change::Moved && changed.is_some() && onto == *changed {
                continue;
            }
            let start = if row == range.start_row {
                range.start_column
            } else {
                0
            };
            let end = if row == range.end_row {
                range.end_column
            } else {
                text.len()
            };
            let (start, end) = (utf16(text, start), utf16(text, end));
            if start < end {
                spans.push(Span {
                    line: *line,
                    start,
                    end,
                    change,
                });
            }
        }
    }
    spans.sort_by_key(|span| (span.line, span.start, span.end));
    spans
}

// How many UTF-16 code units the first `bytes` bytes of `text` are, cut back to a whole character.
fn utf16(text: &str, bytes: usize) -> u32 {
    let mut at = bytes.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    text[..at].encode_utf16().count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(side: &Side) -> Vec<Option<(u32, &str)>> {
        let rows = side.rows.iter();
        rows.map(|row| row.as_ref().map(|row| (row.line, row.text.as_str())))
            .collect()
    }

    fn range(
        (start_row, start_column): (usize, usize),
        (end_row, end_column): (usize, usize),
    ) -> Range {
        Range {
            start_row,
            start_column,
            end_row,
            end_column,
        }
    }

    fn hunk(operation: &str, start: (usize, usize), end: (usize, usize)) -> OutputHunk {
        let (operation, range) = (operation.to_string(), range(start, end));
        OutputHunk {
            operation,
            range,
            move_target: None,
        }
    }

    const TWO_HUNKS: &str = "diff --git a/src/mean.rs b/src/mean.rs
index 1111111..2222222 100644
--- a/src/mean.rs
+++ b/src/mean.rs
@@ -1,3 +1,3 @@
-fn mean(numbers: &[f64]) -> f64 {
+fn mean(values: &[f64]) -> f64 {
     let total = 0.0;

@@ -10,2 +10,3 @@ fn mean(numbers: &[f64]) -> f64 {
     total
+    // done
 }
\\ No newline at end of file
";

    #[test]
    fn each_side_is_its_hunks_lines_numbered_as_the_headers_count_them() {
        let [piece] = pieces(TWO_HUNKS).try_into().unwrap();
        assert_eq!(piece.path, "src/mean.rs");
        assert_eq!(piece.hunks, 2);
        assert_eq!(
            rows(&piece.before),
            [
                Some((1, "fn mean(numbers: &[f64]) -> f64 {")),
                Some((2, "    let total = 0.0;")),
                Some((3, "")),
                None,
                Some((10, "    total")),
                Some((11, "}")),
            ]
        );
        assert_eq!(
            rows(&piece.after),
            [
                Some((1, "fn mean(values: &[f64]) -> f64 {")),
                Some((2, "    let total = 0.0;")),
                Some((3, "")),
                None,
                Some((10, "    total")),
                Some((11, "    // done")),
                Some((12, "}")),
            ]
        );
        assert_eq!(piece.after.text().lines().count(), 7);
    }

    #[test]
    fn a_new_files_old_side_is_empty_and_a_deleted_one_is_named_by_its_old_path() {
        let diff = "--- /dev/null\n+++ b/new.py\n@@ -0,0 +1,2 @@\n+import os\n+print(os.name)\n\
                    --- a/gone.py\n+++ /dev/null\n@@ -1 +0,0 @@\n-pass\n";
        let [new, gone] = pieces(diff).try_into().unwrap();
        assert_eq!(
            (new.path.as_str(), gone.path.as_str()),
            ("new.py", "gone.py")
        );
        assert_eq!(rows(&new.before), []);
        assert_eq!(
            rows(&new.after),
            [Some((1, "import os")), Some((2, "print(os.name)"))]
        );
        assert_eq!(rows(&gone.before), [Some((1, "pass"))]);
        assert_eq!(rows(&gone.after), []);
    }

    #[test]
    fn a_file_with_no_hunks_has_nothing_to_diff() {
        let diff =
            "diff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n";
        assert!(pieces(diff).iter().all(|piece| piece.hunks == 0));
        let diff = "--- a/logo.png\n+++ b/logo.png\nBinary files differ\n";
        assert_eq!(pieces(diff)[0].hunks, 0);
    }

    #[test]
    fn a_header_inside_a_hunk_is_code() {
        let diff = "--- a/notes.md\n+++ b/notes.md\n@@ -1 +1 @@\n--- a/x\n+++ b/x\n";
        let [piece] = pieces(diff).try_into().unwrap();
        assert_eq!(rows(&piece.before), [Some((1, "-- a/x"))]);
        assert_eq!(rows(&piece.after), [Some((1, "++ b/x"))]);
    }

    #[test]
    fn ranges_become_a_span_per_line_on_the_diffs_lines() {
        let [piece] = pieces(TWO_HUNKS).try_into().unwrap();
        let hunks = [
            hunk("update", (0, 8), (0, 15)),
            // Over the blank between the hunks, onto the next hunk's first line.
            hunk("move", (2, 0), (4, 6)),
            hunk("identical", (5, 0), (5, 1)),
        ];
        assert_eq!(
            spans(&piece.before, &piece.after, &hunks),
            [
                Span {
                    line: 1,
                    start: 8,
                    end: 15,
                    change: Change::Updated
                },
                Span {
                    line: 10,
                    start: 0,
                    end: 6,
                    change: Change::Moved
                },
            ]
        );
    }

    #[test]
    fn a_move_onto_what_its_line_became_is_no_change() {
        let diff = "--- a/a.py\n+++ b/a.py\n@@ -1,4 +1,4 @@\n\
                    -n = numbers.len()\n+n = xs.len()\n pass\n-x = one()\n-y = two()\n+y = two()\n+z = one()\n";
        let [piece] = pieces(diff).try_into().unwrap();
        let moved = |from, to, onto: ((usize, usize), (usize, usize))| OutputHunk {
            move_target: Some(range(onto.0, onto.1)),
            ..hunk("move", from, to)
        };
        let hunks = [
            hunk("update", (0, 4), (0, 11)),
            // `.len()`, shifted over on the line it became.
            moved((0, 11), (0, 17), ((0, 6), (0, 12))),
            // `y = two()`, now before the line `x = one()` became.
            moved((3, 0), (3, 9), ((2, 0), (2, 9))),
        ];
        assert_eq!(
            spans(&piece.before, &piece.after, &hunks),
            [
                Span {
                    line: 1,
                    start: 4,
                    end: 11,
                    change: Change::Updated
                },
                Span {
                    line: 4,
                    start: 0,
                    end: 9,
                    change: Change::Moved
                },
            ]
        );
    }

    #[test]
    fn columns_are_utf16_code_units_cut_back_to_a_whole_character() {
        let text = "let s = \"héllo 👋\";";
        assert_eq!(utf16(text, 9), 9);
        // é is 2 bytes and 1 unit; 👋 is 4 bytes and 2 units.
        assert_eq!(utf16(text, 12), 11);
        assert_eq!(utf16(text, 11), 10);
        assert_eq!(utf16(text, 21), 18);
        assert_eq!(utf16(text, 18), 15);
        assert_eq!(utf16(text, 100), text.encode_utf16().count() as u32);
    }

    #[test]
    fn files_are_named_as_in_the_diff() {
        assert_eq!(file_name("src/tui/app.rs"), Path::new("app.rs"));
        assert_eq!(file_name("Makefile"), Path::new("Makefile"));
        assert_eq!(file_name("dir/.."), Path::new("file"));
        assert_eq!(file_name(""), Path::new("file"));
    }

    // Needs omnidiff on PATH: `cargo test -p gem-omnidiff -- --ignored`.
    #[tokio::test]
    #[ignore = "needs omnidiff on PATH"]
    async fn omnidiff_marks_a_rename_rather_than_the_lines_it_is_on() {
        let before = [
            "fn mean(numbers: &[f64]) -> f64 {",
            "    numbers.iter().sum::<f64>() / numbers.len() as f64",
        ];
        let after = [
            "fn mean(values: &[f64]) -> f64 {",
            "    values.iter().sum::<f64>() / values.len() as f64",
        ];
        let diff = format!(
            "--- a/mean.rs\n+++ b/mean.rs\n@@ -1,3 +1,3 @@\n-{}\n-{}\n+{}\n+{}\n }}\n",
            before[0], before[1], after[0], after[1],
        );
        let [file] = highlight(&diff).await.unwrap().try_into().unwrap();
        // The text of each span on a side, by line.
        let marked = |spans: &[Span], lines: [&str; 2]| -> Vec<(u32, String)> {
            let text = |s: &Span| {
                let line: Vec<u16> = lines[s.line as usize - 1].encode_utf16().collect();
                String::from_utf16(&line[s.start as usize..s.end as usize]).unwrap()
            };
            spans.iter().map(|s| (s.line, text(s))).collect()
        };
        // Only what differs between the names: both end in "s".
        let [one, two] = [1, 2].map(|line| (line, "number".to_string()));
        assert_eq!(marked(&file.before, before), [one, two.clone(), two]);
        let [one, two] = [1, 2].map(|line| (line, "value".to_string()));
        assert_eq!(marked(&file.after, after), [one, two.clone(), two]);
        assert!(file.after.iter().all(|span| span.change == Change::Updated));
        assert_eq!(file.summary, None);
    }

    #[tokio::test]
    async fn a_diff_with_nothing_to_diff_runs_nothing() {
        let diff = "diff --git a/logo.png b/logo.png\nBinary files differ\n";
        assert_eq!(highlight(diff).await, Ok(Vec::new()));
    }
}
