use std::{
    cmp::Reverse,
    collections::HashMap,
    future::Future,
    sync::{Arc, OnceLock},
    time::Duration,
};

use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use pbjson_types::Timestamp;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{process::Command, sync::RwLock};

use conversations::Conversations;
use names::Names;
use proto::{
    ApproveProposalRequest, ApproveProposalResponse, CloseInvestigationRequest,
    CloseInvestigationResponse, DismissProposalRequest, DismissProposalResponse,
    InvestigateRequest, InvestigateResponse, ListOpenPullRequestsRequest,
    ListOpenPullRequestsResponse, MessageInvestigationRequest, MessageInvestigationResponse,
    PullRequest, PullRequestCluster, Repository, RestartClusterRequest, RestartClusterResponse,
};

mod conversations;
mod investigations;
mod launcher;
mod names;
mod proposals;
mod room;
mod t3;

mod proto {
    include!(concat!(env!("OUT_DIR"), "/gem.v1.rs"));
    include!(concat!(env!("OUT_DIR"), "/gem.v1.serde.rs"));
}

type Cache<T> = Arc<RwLock<Option<T>>>;

// Without an owner, gh lists the authenticated user's repositories.
const REPOS: &[&str] = &["repo", "list", "--limit", "1000", "--json", "name,url"];

static LOGIN: OnceLock<String> = OnceLock::new();

// The GitHub user gh is authenticated as: whose PRs are listed, and who proposals are run as.
pub fn login() -> &'static str {
    LOGIN.get().unwrap()
}

// Paginated by `gh api graphql --paginate`, which fills in $endCursor. $q is the user's open PRs,
// narrowed by the search qualifiers in GEM_SEARCH, e.g. `org:livekit`.
const PRS_QUERY: &str = r#"
query($endCursor: String, $q: String!) {
  search(query: $q, type: ISSUE, first: 100, after: $endCursor) {
    pageInfo { hasNextPage endCursor }
    nodes {
      ... on PullRequest {
        id number title url headRefName headRefOid baseRefName createdAt updatedAt reviewDecision
        commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
        repository { nameWithOwner name owner { login } }
        headRepository { nameWithOwner }
        timelineItems(itemTypes: [CROSS_REFERENCED_EVENT], first: 100) {
          nodes { ... on CrossReferencedEvent { source { ... on PullRequest { url } } } }
        }
      }
    }
  }
}
"#;

// A fork's head branch can share a name with a base-repo branch (e.g. `main`), so only
// same-repository heads can have dependents.
const PRS_JQ: &str = r#".data.search.nodes[] | {
  id, repository: {owner: .repository.owner.login, name: .repository.name}, number, title, url,
  head: (if .headRepository.nameWithOwner == .repository.nameWithOwner then .headRefName else null end),
  head_oid: .headRefOid,
  base: .baseRefName,
  created_at: .createdAt,
  updated_at: .updatedAt,
  approved: (.reviewDecision == "APPROVED"),
  checks_passing: (.commits.nodes[0].commit.statusCheckRollup.state | . == null or . == "SUCCESS"),
  checks_failing: (.commits.nodes[0].commit.statusCheckRollup.state | . == "FAILURE" or . == "ERROR"),
  mentioned_by: [.timelineItems.nodes[].source.url | select(. != null)]
}"#;

#[derive(Deserialize)]
struct SearchedPullRequest {
    // GraphQL node ID.
    id: String,
    repository: Repository,
    number: u32,
    title: String,
    url: String,
    head: Option<String>,
    // Commit at the tip of the head branch.
    head_oid: String,
    base: String,
    created_at: Timestamp,
    updated_at: Timestamp,
    approved: bool,
    checks_passing: bool,
    checks_failing: bool,
    // URLs of PRs whose description or comments link to this one.
    mentioned_by: Vec<String>,
}

// A refresh costs one GitHub API request per 100 results, far under GitHub's hourly limits.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

// `serve` runs the server, `propose` records a proposal for it, and no command runs the launcher,
// which runs `serve` and keeps it up to date.
fn main() {
    // LIVEKIT_URL, LIVEKIT_API_KEY, LIVEKIT_API_SECRET, GEM_ROOM and GEM_SEARCH can live in .env in the
    // working directory, next to the server's other state. The real environment wins.
    let _ = dotenvy::dotenv();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime = || tokio::runtime::Runtime::new().unwrap();
    match args.first().map(String::as_str) {
        Some("serve") => runtime().block_on(run_server()),
        Some("propose") => match runtime().block_on(proposals::propose(&args[1..])) {
            Ok(id) => println!("{id}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        },
        _ => launcher::run(),
    }
}

async fn run_server() {
    // Servers from before GEM_ROOM was required joined the room gem.
    if std::env::var_os("GEM_ROOM").is_none() {
        let env = std::fs::read_to_string(".env").unwrap_or_default();
        let separator = if env.is_empty() || env.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(".env", format!("{env}{separator}GEM_ROOM=gem\n"))
            .expect("cannot write .env");
        let _ = dotenvy::dotenv();
    }

    let login = gh(&["api", "user", "--jq", ".login"])
        .await
        .expect("gh is not authenticated");
    LOGIN
        .set(String::from_utf8(login).unwrap().trim().to_string())
        .unwrap();

    let repos: Cache<Value> = cached(|| gh_json(REPOS));
    let names = Names::load().await;
    let conversations = Conversations::load().await;
    let prs: Cache<ListOpenPullRequestsResponse> = {
        let names = names.clone();
        let conversations = conversations.clone();
        cached(move || open_pull_requests(names.clone(), conversations.clone()))
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/repos", get(move || serve(repos)))
        .route("/clusters/restart", {
            let conversations = conversations.clone();
            post(|Json(request): Json<RestartClusterRequest>| async move {
                conversations.restart(&request.pull_request_urls).await;
                Json(RestartClusterResponse {})
            })
        })
        .route("/prs", {
            let prs = prs.clone();
            post(move |Json(_): Json<ListOpenPullRequestsRequest>| {
                serve_prs(prs, names, conversations)
            })
        })
        .route("/proposals/approve", {
            let prs = prs.clone();
            post(|Json(request): Json<ApproveProposalRequest>| async move {
                let closed =
                    proposals::approve(&request.id, &request.reply, &request.follow_up).await;
                // Search can list a PR as open for a while after it closes, so drop it here
                // rather than wait for the next refresh.
                if let (Ok(Some(url)), Some(response)) = (closed, &mut *prs.write().await) {
                    for cluster in &mut response.clusters {
                        cluster.pull_requests.retain(|pr| pr.url != url);
                    }
                    response.clusters.retain(|c| !c.pull_requests.is_empty());
                }
                Json(ApproveProposalResponse {})
            })
        })
        .route(
            "/investigations/start",
            post(|Json(request): Json<InvestigateRequest>| async move {
                investigations::start(&request.proposal_id)
                    .await
                    .map(|()| Json(InvestigateResponse {}))
                    .map_err(|error| (StatusCode::BAD_REQUEST, error))
            }),
        )
        .route(
            "/investigations/message",
            post(
                |Json(request): Json<MessageInvestigationRequest>| async move {
                    investigations::message(&request.id, &request.message)
                        .await
                        .map(|()| Json(MessageInvestigationResponse {}))
                        .map_err(|error| (StatusCode::BAD_REQUEST, error))
                },
            ),
        )
        .route(
            "/investigations/close",
            post(
                |Json(request): Json<CloseInvestigationRequest>| async move {
                    investigations::close(&request.id).await;
                    Json(CloseInvestigationResponse {})
                },
            ),
        )
        .route(
            "/proposals/dismiss",
            post(|Json(request): Json<DismissProposalRequest>| async move {
                proposals::dismiss(&request.id).await;
                Json(DismissProposalResponse {})
            }),
        );

    investigations::watch();
    room::serve(app).await;
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

// Requests never call GitHub; a failed refresh keeps serving the last good result.
fn cached<T, F, Fut>(refresh: F) -> Cache<T>
where
    T: Send + Sync + 'static,
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = Option<T>> + Send,
{
    let cache = Cache::default();
    let writer = cache.clone();
    tokio::spawn(async move {
        loop {
            if let Some(value) = refresh().await {
                *writer.write().await = Some(value);
            }
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
    cache
}

async fn serve<T: Serialize + Clone>(cache: Cache<T>) -> Result<Json<T>, StatusCode> {
    cache
        .read()
        .await
        .clone()
        .map(Json)
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

// Names and proposals arrive after the refresh that asked for them, so they are filled in per
// request.
async fn serve_prs(
    cache: Cache<ListOpenPullRequestsResponse>,
    names: Names,
    conversations: Conversations,
) -> Result<Json<ListOpenPullRequestsResponse>, StatusCode> {
    let Json(mut response) = serve(cache).await?;
    names.fill(&mut response.clusters).await;
    proposals::fill(&mut response.clusters, &conversations.waiting().await).await;
    investigations::fill(&mut response).await;
    Ok(Json(response))
}

async fn open_pull_requests(
    names: Names,
    conversations: Conversations,
) -> Option<ListOpenPullRequestsResponse> {
    let search = std::env::var("GEM_SEARCH").unwrap_or_default();
    let stdout = gh(&[
        "api",
        "graphql",
        "--paginate",
        "-f",
        &format!("query={PRS_QUERY}"),
        "-f",
        &format!("q=is:pr is:open author:{} {search}", login()),
        "--jq",
        PRS_JQ,
    ])
    .await?;
    let prs = serde_json::Deserializer::from_slice(&stdout)
        .into_iter()
        .collect::<Result<Vec<SearchedPullRequest>, _>>()
        .ok()?;
    let clusters = cluster(&prs);
    names.request_missing(&clusters).await;
    conversations.refresh(&prs).await;
    Some(ListOpenPullRequestsResponse {
        clusters,
        login: login().to_string(),
        search,
        investigations: Vec::new(),
    })
}

// Two PRs are linked when one is stacked on the other (its base branch is the other's head
// branch in the same repository) or one mentions the other. A cluster is a connected component
// of links, so PRs join through any chain of links, not only direct ones.
fn cluster(prs: &[SearchedPullRequest]) -> Vec<PullRequestCluster> {
    let by_url: HashMap<&str, usize> = prs
        .iter()
        .enumerate()
        .map(|(i, pr)| (pr.url.as_str(), i))
        .collect();
    let by_head: HashMap<(&Repository, &str), usize> = prs
        .iter()
        .enumerate()
        .filter_map(|(i, pr)| Some(((&pr.repository, pr.head.as_deref()?), i)))
        .collect();

    let mut parent: Vec<usize> = (0..prs.len()).collect();
    for (i, pr) in prs.iter().enumerate() {
        let stacked_on = by_head.get(&(&pr.repository, pr.base.as_str()));
        let mentioned_by = pr
            .mentioned_by
            .iter()
            .filter_map(|url| by_url.get(url.as_str()));
        for &j in stacked_on.into_iter().chain(mentioned_by) {
            let (a, b) = (root(&mut parent, i), root(&mut parent, j));
            parent[a] = b;
        }
    }

    let mut clusters: Vec<Vec<&SearchedPullRequest>> = Vec::new();
    let mut cluster_of_root: HashMap<usize, usize> = HashMap::new();
    for (i, pr) in prs.iter().enumerate() {
        let r = root(&mut parent, i);
        let c = *cluster_of_root.entry(r).or_insert_with(|| {
            clusters.push(Vec::new());
            clusters.len() - 1
        });
        clusters[c].push(pr);
    }

    let mut clusters: Vec<PullRequestCluster> = clusters
        .into_iter()
        .map(|mut members| {
            members.sort_by_key(|pr| (pr.created_at.seconds, pr.created_at.nanos));
            PullRequestCluster {
                updated_at: members
                    .iter()
                    .map(|pr| &pr.updated_at)
                    .max_by_key(|t| (t.seconds, t.nanos))
                    .cloned(),
                pull_requests: members
                    .into_iter()
                    .map(|pr| PullRequest {
                        repository: Some(pr.repository.clone()),
                        number: pr.number,
                        title: pr.title.clone(),
                        url: pr.url.clone(),
                        approved: pr.approved,
                        checks_passing: pr.checks_passing,
                        checks_failing: pr.checks_failing,
                        proposals: Vec::new(),
                    })
                    .collect(),
                name: String::new(),
            }
        })
        .collect();
    clusters.sort_by_key(|c| Reverse(c.updated_at.as_ref().map(|t| (t.seconds, t.nanos))));
    clusters
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

// Requires an authenticated `gh` on PATH. gh's default limit is 30, below the repo and PR counts.
async fn gh(args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("gh").args(args).output().await.ok()?;
    output.status.success().then_some(output.stdout)
}

async fn gh_json<T: DeserializeOwned>(args: &[&str]) -> Option<T> {
    serde_json::from_slice(&gh(args).await?).ok()
}
