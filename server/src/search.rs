use std::{
    io::Write,
    process::{Command, Stdio},
    sync::{
        LazyLock, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use gem_json::{Map, Value, json};
use gem_log::info;

use crate::SearchedPullRequest;

// TypeSafe's Jev decision model, on the key in TYPESAFE_API_KEY (see `load_typesafe_key`).
const URL: &str = "https://api.typesafe.ai/v1/systemone";
const MODEL: &str = "jev-1.13.0";
// Jev reads at most 64k tokens a request, and 95 PRs' descriptions are more.
const PRS_PER_REQUEST: usize = 25;
// The model's probability that a PR matches, from which it is shown.
const THRESHOLD: f64 = 0.5;

// What search reads of an open PR. Kept here rather than in the clusters, so descriptions are not
// sent to every frontend with each snapshot.
#[derive(Clone)]
struct Described {
    url: String,
    title: String,
    body: String,
}

// Every open PR, as of the last refresh.
static OPEN: LazyLock<RwLock<Vec<Described>>> = LazyLock::new(Default::default);

pub fn index(prs: &[SearchedPullRequest]) {
    *OPEN.write().unwrap() = (prs.iter())
        .map(|pr| Described {
            url: pr.url.clone(),
            title: pr.title.clone(),
            body: pr.body.clone(),
        })
        .collect();
}

// The URLs of the open PRs a model decides match `query`, each PR its own question. Fails when
// any request fails, rather than leave its PRs out.
pub fn matching(query: &str) -> Result<Vec<String>, String> {
    let prs = OPEN.read().unwrap().clone();
    let key = gem_env::var("TYPESAFE_API_KEY")
        .ok_or("TYPESAFE_API_KEY is not set: name its 1Password item in GEM_TYPESAFE_OP_ITEM")?;
    let decisions: Vec<Result<Vec<String>, String>> = thread::scope(|scope| {
        let requests: Vec<_> = (prs.chunks(PRS_PER_REQUEST))
            .map(|chunk| {
                let questions: Map = (chunk.iter().enumerate())
                    .map(|(i, pr)| {
                        let instructions = format!(
                            "Is this pull request about, or does it mention, what the user searches \
                             for?\n\nTitle: {}\n\nDescription:\n{}",
                            pr.title, pr.body
                        );
                        (
                            i.to_string(),
                            json!({ "type": "noul", "instructions": instructions }),
                        )
                    })
                    .collect();
                let request = json!({
                    "model": MODEL,
                    "state": { "search": query },
                    "questions": questions,
                });
                let urls: Vec<String> = chunk.iter().map(|pr| pr.url.clone()).collect();
                let key = &key;
                scope.spawn(move || {
                    let (status, body) = post(&request, key)?;
                    if !(200..300).contains(&status) {
                        return Err(format!("TypeSafe answered {status}: {body}"));
                    }
                    let response = gem_json::parse(&body).map_err(|e| e.to_string())?;
                    Ok::<_, String>(
                        (urls.into_iter().enumerate())
                            .filter(|(i, _)| {
                                response["answers"][i.to_string().as_str()]["noul"].as_f64()
                                    >= Some(THRESHOLD)
                            })
                            .map(|(_, url)| url)
                            .collect::<Vec<_>>(),
                    )
                })
            })
            .collect();
        (requests.into_iter())
            .map(|request| {
                (request.join()).unwrap_or_else(|_| Err("a search request panicked".to_string()))
            })
            .collect()
    });
    let mut matched = Vec::new();
    for decision in decisions {
        matched.extend(decision?);
    }
    info!(
        "searched pull requests",
        query = query,
        matched = matched.len()
    );
    Ok(matched)
}

// Names each request's body file.
static REQUESTS: AtomicU64 = AtomicU64::new(0);

// POSTs `body` to URL with curl, which every platform the server runs on has. The status and body
// of the response.
fn post(body: &Value, key: &str) -> Result<(u16, String), String> {
    // The body, up to 25 PRs' descriptions, is too long for a command line, and stdin carries the
    // headers, so it goes in a file.
    let file = std::env::temp_dir().join(format!(
        "gem-search-{}-{}.json",
        std::process::id(),
        REQUESTS.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&file, body.to_string()).map_err(|e| e.to_string())?;
    let output = curl(&file, key);
    let _ = std::fs::remove_file(&file);
    let output = output?;
    // `--write-out` puts the status on a line of its own after the body.
    let (body, status) = output.rsplit_once('\n').unwrap_or(("", &output));
    let status = (status.trim().parse())
        .map_err(|_| format!("curl printed no status for {URL}: {status}"))?;
    Ok((status, body.to_string()))
}

fn curl(file: &std::path::Path, key: &str) -> Result<String, String> {
    // The headers go on stdin, so the key is not in the process list.
    let mut child = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--request",
            "POST",
            "--header",
            "@-",
        ])
        .args(["--data-binary", &format!("@{}", file.display())])
        .args(["--write-out", r"\n%{http_code}", URL])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    let headers = format!("Authorization: Bearer {key}\nContent-Type: application/json\n");
    let mut stdin = child.stdin.take().ok_or("curl has no stdin")?;
    let written = stdin.write_all(headers.as_bytes());
    drop(stdin);
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    written.map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "cannot reach TypeSafe: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
