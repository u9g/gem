use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    future::Future,
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    http::StatusCode,
    routing::{get, post},
};
use pbjson_types::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::RwLock;

use changes::Changes;
use conversations::Conversations;
use names::Names;
use proto::{
    ApproveProposalRequest, ApproveProposalResponse, Checks, CloseInvestigationRequest,
    CloseInvestigationResponse, DismissProposalRequest, DismissProposalResponse,
    InvestigateRequest, InvestigateResponse, ListOpenPullRequestsRequest,
    ListOpenPullRequestsResponse, MessageClusterRequest, MessageClusterResponse,
    MessageInvestigationRequest, MessageInvestigationResponse, PullRequest, PullRequestCluster,
    RebasePullRequestRequest, RebasePullRequestResponse, Repository, RestartClusterRequest,
    RestartClusterResponse,
};

mod changes;
mod conversations;
mod investigations;
mod mcp;
mod names;
mod proposals;
mod rebases;

// The server's own layers, each a crate in crates/ that depends on nothing of the server's.
use gem_github::{gh, gh_json, login};
use gem_proto as proto;
use gem_t3 as t3;

type Cache<T> = Arc<RwLock<Option<T>>>;

// Without an owner, gh lists the authenticated user's repositories.
const REPOS: &[&str] = &["repo", "list", "--limit", "1000", "--json", "name,url"];

// Paginated by `gh api graphql --paginate`, which fills in $endCursor. $q is the user's open PRs,
// narrowed by the search qualifiers in GEM_SEARCH, e.g. `org:livekit`.
const PRS_QUERY: &str = r#"
query($endCursor: String, $q: String!) {
  search(query: $q, type: ISSUE, first: 100, after: $endCursor) {
    pageInfo { hasNextPage endCursor }
    nodes {
      ... on PullRequest {
        id number state title url headRefName headRefOid baseRefName createdAt updatedAt reviewDecision
        commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 0) {
          checkRunCount checkRunCountsByState { state count }
          statusContextCount statusContextCountsByState { state count }
        } } } } }
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
// same-repository heads can have dependents. Search keeps matching `is:open` for a while after a
// merge or close, but each node's state is current.
const PRS_JQ: &str = r#".data.search.nodes[] | select(.state == "OPEN") | {
  id, repository: {owner: .repository.owner.login, name: .repository.name}, number, title, url,
  head: (if .headRepository.nameWithOwner == .repository.nameWithOwner then .headRefName else null end),
  head_oid: .headRefOid,
  base: .baseRefName,
  created_at: .createdAt,
  updated_at: .updatedAt,
  approved: (.reviewDecision == "APPROVED"),
  checks_passing: (.commits.nodes[0].commit.statusCheckRollup.state | . == null or . == "SUCCESS"),
  checks_failing: (.commits.nodes[0].commit.statusCheckRollup.state | . == "FAILURE" or . == "ERROR"),
  checks: (.commits.nodes[0].commit.statusCheckRollup.contexts
    | if . == null then {total: 0, done: 0, failed: 0} else
      [.checkRunCountsByState[], .statusContextCountsByState[]] as $by
      | (.checkRunCount + .statusContextCount) as $total
      | {
        total: $total,
        done: ($total - ([$by[] | select(.state | IN("QUEUED", "IN_PROGRESS", "PENDING", "WAITING", "REQUESTED", "EXPECTED")) | .count] | add // 0)),
        failed: ([$by[] | select(.state | IN("FAILURE", "TIMED_OUT", "STARTUP_FAILURE", "ERROR")) | .count] | add // 0)
      } end),
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
    checks: Checks,
    // URLs of PRs whose description or comments link to this one.
    mentioned_by: Vec<String>,
}

// A refresh costs one GitHub API request per 100 results, far under GitHub's hourly limits.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
// Building the snapshot reads only memory and the state directory.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(2);

// `serve` runs the server, `propose` records a proposal for it, `mcp` serves its clusters to
// agents over MCP, and no command runs the launcher, which runs `serve` and keeps it up to date.
fn main() {
    // GEM_SEARCH, GEM_T3_PROVIDER and GEM_OP_ITEM, or LIVEKIT_URL, LIVEKIT_API_KEY,
    // LIVEKIT_API_SECRET and GEM_ROOM themselves, can live in .env in the working directory, next
    // to the server's other state. The real environment wins.
    let _ = dotenvy::dotenv();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime = || tokio::runtime::Runtime::new().unwrap();
    match args.first().map(String::as_str) {
        Some("serve") => {
            load_op_item().unwrap_or_else(|error| panic!("{error}"));
            runtime().block_on(run_server())
        }
        Some("mcp") => mcp::serve(&args[1..]),
        Some("propose") => match runtime().block_on(proposals::propose(&args[1..])) {
            Ok(id) => println!("{id}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        },
        _ => {
            // Nothing restarts the launcher, so it asks again rather than exit.
            while let Err(error) = load_op_item() {
                eprintln!("launcher: {error}; asking again in {OP_RETRY_DELAY:?}");
                std::thread::sleep(OP_RETRY_DELAY);
            }
            gem_launcher::run()
        }
    }
}

// How long the launcher waits to read the 1Password item again after it could not, e.g. when the
// read was denied.
const OP_RETRY_DELAY: Duration = Duration::from_secs(60);

// The variables a 1Password item can hold, each in a field named after it. A login's username and
// password hold the API key and secret instead, so iOS AutoFill can fill them into the frontend.
const OP_FIELDS: &[&str] = &[
    "LIVEKIT_URL",
    "LIVEKIT_API_KEY",
    "LIVEKIT_API_SECRET",
    "GEM_ROOM",
];

// Sets OP_FIELDS from the 1Password item GEM_OP_ITEM names, as op://<vault>/<item>, so the
// LiveKit login stays out of .env. A variable already set, by .env or the real environment, wins.
// The launcher reads the item once; every server it starts inherits the variables, so servers ask
// 1Password, and the phone, nothing.
fn load_op_item() -> Result<(), String> {
    #[derive(Deserialize)]
    struct Item {
        fields: Vec<Field>,
    }
    #[derive(Deserialize)]
    struct Field {
        label: String,
        // USERNAME or PASSWORD for a login's own username and password.
        purpose: Option<String>,
        value: Option<String>,
    }

    let Ok(reference) = std::env::var("GEM_OP_ITEM") else {
        return Ok(());
    };
    if OP_FIELDS
        .iter()
        .all(|name| std::env::var_os(name).is_some())
    {
        return Ok(());
    }
    let (vault, item) = reference
        .strip_prefix("op://")
        .and_then(|path| path.trim_end_matches('/').split_once('/'))
        .ok_or_else(|| format!("GEM_OP_ITEM {reference} is not op://<vault>/<item>"))?;
    let output = std::process::Command::new("op")
        .args([
            "item", "get", item, "--vault", vault, "--format", "json", "--reveal",
        ])
        .output()
        .map_err(|error| format!("cannot run op: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot read {reference}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let item: Item = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("op printed no item for {reference}: {error}"))?;
    for Field {
        label,
        purpose,
        value,
    } in item.fields
    {
        let name = match purpose.as_deref() {
            Some("USERNAME") => "LIVEKIT_API_KEY",
            Some("PASSWORD") => "LIVEKIT_API_SECRET",
            _ => label.as_str(),
        };
        let Some(value) = value.filter(|value| !value.is_empty()) else {
            continue;
        };
        if OP_FIELDS.contains(&name) && std::env::var_os(name).is_none() {
            // SAFETY: main calls this before starting any other thread.
            unsafe { std::env::set_var(name, value) };
        }
    }
    Ok(())
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

    // Before anything uses it, so a server without gh signed in panics now, not on a request.
    login();

    // Nothing watches the repositories for changes.
    let repos: Cache<Value> = cached(|| gh_json(REPOS), Changes::default());
    let names = Names::load().await;
    let conversations = Conversations::load().await;
    let changes = Changes::default();
    changes.poll();
    let prs: Cache<ListOpenPullRequestsResponse> = {
        let (names, conversations, watched) =
            (names.clone(), conversations.clone(), changes.clone());
        cached(
            move || open_pull_requests(names.clone(), conversations.clone(), watched.clone()),
            changes,
        )
    };
    // The server's own database, next to its other state.
    let db = gem_db::Db::open("gem.sqlite").unwrap_or_else(|error| panic!("{error}"));
    rebases::init(db, prs.clone()).unwrap_or_else(|error| panic!("{error}"));

    // Agents read the clusters through `server mcp`, and frontends are sent them, as soon as they
    // change. Proposals and investigations change on disk, written by other processes, so the
    // snapshot is compared rather than announced.
    tokio::task::spawn_blocking(mcp::register);
    let (snapshots, snapshot) = tokio::sync::watch::channel(String::new());
    {
        let (prs, names, conversations) = (prs.clone(), names.clone(), conversations.clone());
        tokio::spawn(async move {
            loop {
                if let Some(response) = filled(&prs, &names, &conversations).await {
                    let json = serde_json::to_string(&response).unwrap();
                    if *snapshots.borrow() != json {
                        mcp::save(&response).await;
                        snapshots.send_replace(json);
                    }
                }
                tokio::time::sleep(SNAPSHOT_INTERVAL).await;
            }
        });
    }

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
        .route("/clusters/message", {
            let (prs, names, conversations) = (prs.clone(), names.clone(), conversations.clone());
            post(|Json(request): Json<MessageClusterRequest>| async move {
                let response = filled(&prs, &names, &conversations).await;
                let first = request.pull_request_urls.first();
                let cluster = (response.iter().flat_map(|r| &r.clusters))
                    .find(|c| c.pull_requests.iter().any(|pr| Some(&pr.url) == first))
                    .ok_or((StatusCode::NOT_FOUND, "the cluster is gone".to_string()))?;
                investigations::ask(cluster, &request.message)
                    .await
                    .map(|investigation_id| Json(MessageClusterResponse { investigation_id }))
                    .map_err(|error| (StatusCode::BAD_REQUEST, error))
            })
        })
        .route("/prs", {
            let prs = prs.clone();
            post(move |Json(_): Json<ListOpenPullRequestsRequest>| {
                serve_prs(prs, names, conversations)
            })
        })
        .route(
            "/prs/rebase",
            post(|Json(request): Json<RebasePullRequestRequest>| async move {
                rebases::start(&request.url)
                    .await
                    .map(|()| Json(RebasePullRequestResponse {}))
                    .map_err(|error| (StatusCode::BAD_REQUEST, error))
            }),
        )
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
    gem_room::serve(move |request| gem_http::respond(app.clone(), request), snapshot).await;
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

// Requests never call GitHub; a failed refresh keeps serving the last good result. Refreshes as
// soon as `changes` sees GitHub change, and every REFRESH_INTERVAL for what it does not watch.
fn cached<T, F, Fut>(refresh: F, changes: Changes) -> Cache<T>
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
            tokio::select! {
                () = tokio::time::sleep(REFRESH_INTERVAL) => {}
                () = changes.changed() => {}
            }
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
    (filled(&cache, &names, &conversations).await)
        .map(Json)
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

// The cached clusters with their names, proposals and investigations, as gem shows them.
async fn filled(
    cache: &Cache<ListOpenPullRequestsResponse>,
    names: &Names,
    conversations: &Conversations,
) -> Option<ListOpenPullRequestsResponse> {
    let mut response = cache.read().await.clone()?;
    names.fill(&mut response.clusters).await;
    proposals::fill(&mut response.clusters, &conversations.waiting().await).await;
    investigations::fill(&mut response).await;
    rebases::fill(&mut response.clusters).await;
    conversations.fill(&mut response.clusters).await;
    Some(response)
}

async fn open_pull_requests(
    names: Names,
    conversations: Conversations,
    changes: Changes,
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
    changes.watch(&prs).await;
    let clusters = cluster(&prs, &behind(&prs).await);
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
fn cluster(prs: &[SearchedPullRequest], behind: &HashSet<&str>) -> Vec<PullRequestCluster> {
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
                        behind: behind.contains(pr.url.as_str()),
                        rebase: None,
                        checks: Some(pr.checks),
                        head_oid: pr.head_oid.clone(),
                        agents: Vec::new(),
                    })
                    .collect(),
                name: String::new(),
            }
        })
        .collect();
    clusters.sort_by_key(|c| Reverse(c.updated_at.as_ref().map(|t| (t.seconds, t.nanos))));
    clusters
}

// The URLs of the PRs whose head commit lacks commits on their base branch, in one request. A
// field's arguments cannot come from another field, so search cannot compare them itself.
async fn behind(prs: &[SearchedPullRequest]) -> HashSet<&str> {
    let fields: String = (prs.iter().enumerate())
        .map(|(i, pr)| {
            format!(
                "pr{i}: repository(owner: {}, name: {}) {{ ref(qualifiedName: {}) {{ compare(headRef: {}) {{ behindBy }} }} }}\n",
                json!(pr.repository.owner),
                json!(pr.repository.name),
                json!(pr.base),
                json!(pr.head_oid),
            )
        })
        .collect();
    if fields.is_empty() {
        return HashSet::new();
    }
    let Some(data) =
        gh_json::<Value>(&["api", "graphql", "-f", &format!("query={{\n{fields}}}")]).await
    else {
        return HashSet::new();
    };
    (prs.iter().enumerate())
        .filter(|(i, _)| {
            data["data"][format!("pr{i}")]["ref"]["compare"]["behindBy"].as_u64() > Some(0)
        })
        .map(|(_, pr)| pr.url.as_str())
        .collect()
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}
