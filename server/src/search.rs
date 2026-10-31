use std::{env, sync::LazyLock};

use reqwest::Client;
use serde_json::{Map, Value, json};
use tokio::sync::RwLock;

use crate::SearchedPullRequest;

// TypeSafe's Jev decision model, on the key in TYPESAFE_API_KEY (see `load_typesafe_key`).
const URL: &str = "https://api.typesafe.ai/v1/systemone";
const MODEL: &str = "jev-1.13.0";
// Jev reads at most 64k tokens a request, and 95 PRs' descriptions are more.
const PRS_PER_REQUEST: usize = 25;
// The model's probability that a PR matches, from which it is shown.
const THRESHOLD: f64 = 0.5;

// What search reads of an open PR. Kept here rather than in the clusters, so descriptions are not
// copied into every frontend's replica of the view.
#[derive(Clone)]
struct Described {
    url: String,
    title: String,
    body: String,
}

// Every open PR, as of the last refresh.
static OPEN: LazyLock<RwLock<Vec<Described>>> = LazyLock::new(Default::default);

pub async fn index(prs: &[SearchedPullRequest]) {
    *OPEN.write().await = (prs.iter())
        .map(|pr| Described {
            url: pr.url.clone(),
            title: pr.title.clone(),
            body: pr.body.clone(),
        })
        .collect();
}

// The URLs of the open PRs a model decides match `query`, each PR its own question. Fails when
// any request fails, rather than leave its PRs out.
pub async fn matching(query: &str) -> Result<Vec<String>, String> {
    let prs = OPEN.read().await.clone();
    // reqwest's TLS takes the process's default crypto provider, and ring is the one LiveKit's
    // SDK builds on.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let key = env::var("TYPESAFE_API_KEY").map_err(
        |_| "TYPESAFE_API_KEY is not set: name its 1Password item in GEM_TYPESAFE_OP_ITEM",
    )?;
    let client = Client::new();
    let mut decisions = tokio::task::JoinSet::new();
    for chunk in prs.chunks(PRS_PER_REQUEST) {
        let questions: Map<String, Value> = (chunk.iter().enumerate())
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
        let request = client.post(URL).bearer_auth(&key).json(&json!({
            "model": MODEL,
            "state": { "search": query },
            "questions": questions,
        }));
        let urls: Vec<String> = chunk.iter().map(|pr| pr.url.clone()).collect();
        decisions.spawn(async move {
            let response = request.send().await.map_err(|e| e.to_string())?;
            let status = response.status();
            let body = response.text().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("TypeSafe answered {status}: {body}"));
            }
            let response: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
            Ok::<_, String>(
                (urls.into_iter().enumerate())
                    .filter(|(i, _)| {
                        response["answers"][i.to_string()]["noul"].as_f64() >= Some(THRESHOLD)
                    })
                    .map(|(_, url)| url)
                    .collect::<Vec<_>>(),
            )
        });
    }
    let mut matched = Vec::new();
    while let Some(decision) = decisions.join_next().await {
        matched.extend(decision.map_err(|e| e.to_string())??);
    }
    tracing::info!(query, matched = matched.len(), "searched pull requests");
    Ok(matched)
}
