use std::{
    cmp::Reverse,
    collections::HashMap,
    ffi::OsString,
    path::Path,
    sync::{Arc, OnceLock, RwLock},
    thread,
    time::Duration,
};

use gem_http::{Reply, Router, ok};
use gem_json::{Error, FromJson, ToJson, Value, json};
use gem_log::{info, warn};
use gem_proto::Timestamp;

use conversations::Conversations;
use gem_changes::Changes;
use names::Names;
use proto::{
    ApproveProposalRequest, ApproveProposalResponse, Checks, CloseAgentRequest,
    CloseAgentResponse, DismissProposalRequest, DismissProposalResponse, InvestigateRequest,
    InvestigateResponse, ListOpenPullRequestsRequest, ListOpenPullRequestsResponse, MergeBy,
    MergeClusterRequest, MergeClusterResponse, MessageAgentRequest, MessageAgentResponse,
    MessageClusterRequest, MessageClusterResponse, PullRequest, PullRequestCluster,
    RebaseClusterRequest,
    RebaseClusterResponse, RebasePullRequestRequest, RebasePullRequestResponse,
    RenderMarkdownRequest, RenderMarkdownResponse, Repository, RestartClusterRequest,
    RestartClusterResponse, SearchPullRequestsRequest, SearchPullRequestsResponse,
};

mod agents;
mod conversations;
mod files;
mod import;
mod investigations;
mod mcp;
mod merges;
mod names;
mod proposals;
mod rebases;
mod search;
mod tokens;

// The server's own layers, each a crate in crates/ that depends on nothing of the server's.
use gem_github::{gh, gh_json, gh_partial_json, login, render};
use gem_proto as proto;
use gem_t3 as t3;

type Cache<T> = Arc<RwLock<Option<T>>>;

// The server's own database, in its working directory, with its state.
const DB_FILE: &str = "gem.sqlite";
static DB: OnceLock<gem_db::Db> = OnceLock::new();

// Opened when the server starts, before anything reads its state.
fn db() -> &'static gem_db::Db {
    DB.get().expect("the database is not open")
}

// Without an owner, gh lists the authenticated user's repositories.
const REPOS: &[&str] = &["repo", "list", "--limit", "1000", "--json", "name,url"];

// Paginated by `gh api graphql --paginate`, which fills in $endCursor. $q is the user's open PRs,
// narrowed by the search qualifiers in GEM_SEARCH, e.g. `org:livekit`. Search ORs the qualifiers
// naming where to look, so `org:livekit repo:u9g/gem` adds one repository to an organization. A page
// must finish within GitHub's ~10s GraphQL timeout, past which it answers 502; 100 PRs per page do not.
const PRS_QUERY: &str = r#"
query($endCursor: String, $q: String!) {
  search(query: $q, type: ISSUE, first: 25, after: $endCursor) {
    pageInfo { hasNextPage endCursor }
    nodes {
      ... on PullRequest {
        id number state title body url headRefName headRefOid baseRefName createdAt updatedAt reviewDecision
        commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 0) {
          checkRunCount checkRunCountsByState { state count }
          statusContextCount statusContextCountsByState { state count }
        } } } } }
        repository { nameWithOwner name owner { login } viewerPermission }
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
//
// Whether checks pass or fail comes from the counts, not statusCheckRollup.state: search rolls a
// CANCELLED check up as FAILURE, so a run cancelled by a newer one on the same commit (a workflow's
// concurrency group) failed the PR although every check of the newer run passed. A cancelled check
// counts as neither, as it does for investigations. ACTION_REQUIRED, a run waiting on a maintainer's
// approval to start, is done but has not passed.
const PRS_JQ: &str = r#".data.search.nodes[] | select(.state == "OPEN")
| (.commits.nodes[0].commit.statusCheckRollup.contexts
  | if . == null then {total: 0, done: 0, failed: 0, blocked: 0} else
    [.checkRunCountsByState[], .statusContextCountsByState[]] as $by
    | (.checkRunCount + .statusContextCount) as $total
    | {
      total: $total,
      done: ($total - ([$by[] | select(.state | IN("QUEUED", "IN_PROGRESS", "PENDING", "WAITING", "REQUESTED", "EXPECTED")) | .count] | add // 0)),
      failed: ([$by[] | select(.state | IN("FAILURE", "TIMED_OUT", "STARTUP_FAILURE", "ERROR")) | .count] | add // 0),
      blocked: ([$by[] | select(.state == "ACTION_REQUIRED") | .count] | add // 0)
    } end) as $checks
| {
  id, repository: {owner: .repository.owner.login, name: .repository.name}, number, title, body, url,
  head: (if .headRepository.nameWithOwner == .repository.nameWithOwner then .headRefName else null end),
  head_oid: .headRefOid,
  base: .baseRefName,
  created_at: .createdAt,
  updated_at: .updatedAt,
  approved: (.reviewDecision == "APPROVED"),
  admin: (.repository.viewerPermission == "ADMIN"),
  checks_passing: ($checks.done == $checks.total and $checks.failed == 0 and $checks.blocked == 0),
  checks_failing: ($checks.failed > 0),
  checks: ($checks | del(.blocked)),
  mentioned_by: [.timelineItems.nodes[].source.url | select(. != null)]
}"#;

struct SearchedPullRequest {
    // GraphQL node ID.
    id: String,
    repository: Repository,
    number: u32,
    title: String,
    // Description, in markdown.
    body: String,
    url: String,
    head: Option<String>,
    // Commit at the tip of the head branch.
    head_oid: String,
    base: String,
    created_at: Timestamp,
    updated_at: Timestamp,
    approved: bool,
    // Set by `merge_states`.
    behind: bool,
    conflicts: bool,
    admin: bool,
    checks_passing: bool,
    checks_failing: bool,
    checks: Checks,
    // URLs of PRs whose description or comments link to this one.
    mentioned_by: Vec<String>,
}

impl FromJson for SearchedPullRequest {
    fn from_json(value: &Value) -> Result<SearchedPullRequest, Error> {
        Ok(SearchedPullRequest {
            id: value.field("id")?,
            repository: value.field("repository")?,
            number: value.field("number")?,
            title: value.field("title")?,
            body: value.field("body")?,
            url: value.field("url")?,
            head: value.field("head")?,
            head_oid: value.field("head_oid")?,
            base: value.field("base")?,
            created_at: value.field("created_at")?,
            updated_at: value.field("updated_at")?,
            approved: value.field("approved")?,
            behind: value.field_or_default("behind")?,
            conflicts: value.field_or_default("conflicts")?,
            admin: value.field("admin")?,
            checks_passing: value.field("checks_passing")?,
            checks_failing: value.field("checks_failing")?,
            checks: value.field("checks")?,
            mentioned_by: value.field("mentioned_by")?,
        })
    }
}

fn merge_by(pr: &SearchedPullRequest) -> MergeBy {
    match (pr.checks_passing, pr.approved, pr.admin) {
        (false, _, _) => MergeBy::Unspecified,
        (true, true, _) => MergeBy::Approval,
        (true, false, true) => MergeBy::Admin,
        (true, false, false) => MergeBy::Unspecified,
    }
}

pub(crate) fn cluster_merge_by(cluster: &PullRequestCluster) -> MergeBy {
    let prs = || cluster.pull_requests.iter().map(|pr| pr.merge_by());
    if prs().any(|by| by == MergeBy::Unspecified) {
        MergeBy::Unspecified
    } else if prs().any(|by| by == MergeBy::Admin) {
        MergeBy::Admin
    } else {
        MergeBy::Approval
    }
}

// A refresh costs one GitHub API request per 100 results, far under GitHub's hourly limits.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
// Building the snapshot reads only memory and the database.
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(2);

// `serve` runs the server, `mcp` serves its clusters to agents over MCP, and no command runs the
// launcher, which runs `serve` and keeps it up to date.
fn main() {
    // GEM_SEARCH, GEM_T3_HOME, GEM_T3_PROVIDER, GEM_MCP_PORT, GEM_OP_ITEM and GEM_TYPESAFE_OP_ITEM,
    // or LIVEKIT_URL, LIVEKIT_API_KEY, LIVEKIT_API_SECRET and GEM_ROOM themselves, can live in .env
    // in the working directory, next to the server's other state. The real environment wins.
    let dotenv = load_dotenv();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => {
            (load_op_item().and_then(|()| load_typesafe_key())).unwrap_or_else(|error| panic!("{error}"));
            run_server()
        }
        Some("mcp") => mcp::serve(&args[1..]),
        _ => {
            // Nothing restarts the launcher, so it asks again rather than exit.
            while let Err(error) = load_op_item().and_then(|()| load_typesafe_key()) {
                eprintln!("launcher: {error}; asking again in {OP_RETRY_DELAY:?}");
                thread::sleep(OP_RETRY_DELAY);
            }
            gem_launcher::run(dotenv)
        }
    }
}

// Loads .env into gem_env and returns the variables it set, other than the LiveKit login and room,
// for the launcher to leave out of each server's environment: the server reads them from .env
// again, and sees an edit made since. The login and room stay inherited, as they do from
// 1Password, so a server never has to ask for them.
fn load_dotenv() -> Vec<OsString> {
    let vars = match gem_env::read_dotenv(Path::new(".env")) {
        None => return Vec::new(),
        Some(Err(error)) => {
            eprintln!("cannot read .env: {error}");
            return Vec::new();
        }
        Some(Ok(vars)) => vars,
    };
    let mut set = Vec::new();
    for (name, value) in vars {
        // A variable already set, by the real environment or earlier in .env, wins, as dotenvy has
        // it.
        if gem_env::is_set(&name) {
            continue;
        }
        gem_env::set(&name, &value);
        if !OP_FIELDS.contains(&name.as_str()) {
            set.push(OsString::from(name));
        }
    }
    set
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
    struct Item {
        fields: Vec<Field>,
    }
    struct Field {
        label: String,
        // USERNAME or PASSWORD for a login's own username and password.
        purpose: Option<String>,
        value: Option<String>,
    }
    impl FromJson for Item {
        fn from_json(value: &Value) -> Result<Item, Error> {
            Ok(Item { fields: value.field("fields")? })
        }
    }
    impl FromJson for Field {
        fn from_json(value: &Value) -> Result<Field, Error> {
            Ok(Field {
                label: value.field("label")?,
                purpose: value.field("purpose")?,
                value: value.field("value")?,
            })
        }
    }

    let Some(reference) = gem_env::var("GEM_OP_ITEM") else {
        return Ok(());
    };
    if OP_FIELDS.iter().all(|name| gem_env::is_set(name)) {
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
    let item: Item = gem_json::from_slice(&output.stdout)
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
        if OP_FIELDS.contains(&name) && !gem_env::is_set(name) {
            gem_env::set(name, &value);
        }
    }
    Ok(())
}

// Sets TYPESAFE_API_KEY, which search decides with, from the credential of the 1Password API
// credential GEM_TYPESAFE_OP_ITEM names, as op://<vault>/<item>. Read once, like GEM_OP_ITEM, so
// servers inherit it and ask 1Password nothing. TYPESAFE_API_KEY already set wins.
fn load_typesafe_key() -> Result<(), String> {
    let Some(reference) = gem_env::var("GEM_TYPESAFE_OP_ITEM") else {
        return Ok(());
    };
    if gem_env::is_set("TYPESAFE_API_KEY") {
        return Ok(());
    }
    let output = std::process::Command::new("op")
        .args(["read", &format!("{}/credential", reference.trim_end_matches('/'))])
        .output()
        .map_err(|error| format!("cannot run op: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot read {reference}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
    gem_env::set("TYPESAFE_API_KEY", &key);
    Ok(())
}

fn run_server() -> ! {
    // Servers from before GEM_ROOM was required joined the room gem.
    if !gem_env::is_set("GEM_ROOM") {
        let env = std::fs::read_to_string(".env").unwrap_or_default();
        let separator = if env.is_empty() || env.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(".env", format!("{env}{separator}GEM_ROOM=gem\n"))
            .expect("cannot write .env");
        load_dotenv();
    }

    // Before anything uses it, so a server without gh signed in panics now, not on a request.
    login();

    let db = gem_db::Db::open(DB_FILE).unwrap_or_else(|error| panic!("{error}"));
    import::import(&db).unwrap_or_else(|error| panic!("{error}"));
    let _ = DB.set(db.clone());

    // Nothing watches the repositories for changes.
    let repos: Cache<Value> = cached(|| gh_json("repositories", REPOS), Changes::default());
    let names = Names::load().unwrap_or_else(|error| panic!("{error}"));
    let conversations = Conversations::load().unwrap_or_else(|error| panic!("{error}"));
    let changes = Changes::default();
    changes.poll();
    let prs: Cache<ListOpenPullRequestsResponse> = {
        let (conversations, watched) = (conversations.clone(), changes.clone());
        cached(
            move || open_pull_requests(conversations.clone(), watched.clone()),
            changes,
        )
    };
    rebases::init(db.clone(), prs.clone()).unwrap_or_else(|error| panic!("{error}"));
    merges::init(db).unwrap_or_else(|error| panic!("{error}"));

    // Agents read the clusters through `server mcp`, and frontends are sent them, as soon as they
    // change. Proposals and investigations change in the database, so the snapshot is compared rather than
    // announced. The agents gem starts propose through `gem-agent`.
    thread::spawn(|| {
        mcp::register();
    });
    thread::spawn(|| {
        mcp::listen();
    });
    let snapshots = gem_room::Snapshots::new();
    {
        let (snapshots, prs, names, conversations) =
            (snapshots.clone(), prs.clone(), names.clone(), conversations.clone());
        thread::spawn(move || {
            loop {
                if let Some(response) = filled(&prs, &names, &conversations) {
                    let json = gem_json::to_string(&response);
                    if snapshots.current() != json {
                        mcp::save(&response);
                        snapshots.send(json);
                    }
                }
                thread::sleep(SNAPSHOT_INTERVAL);
            }
        });
    }

    let app = Router::new()
        .get("/healthz", healthz)
        .get("/repos", move || serve(&repos))
        .post("/clusters/restart", {
            let conversations = conversations.clone();
            move |request: RestartClusterRequest| {
                conversations.restart(&request.pull_request_urls);
                ok(&RestartClusterResponse {})
            }
        })
        .post("/clusters/message", {
            let (prs, names, conversations) = (prs.clone(), names.clone(), conversations.clone());
            move |request: MessageClusterRequest| {
                let response = filled(&prs, &names, &conversations);
                let cluster = acted_on(response.as_ref(), &request.pull_request_urls)?;
                investigations::ask(cluster, &request.message)
                    .map(|agent_id| MessageClusterResponse { agent_id }.to_json())
                    .map_err(|error| (400, error))
            }
        })
        .post("/clusters/merge", {
            let (prs, names, conversations) = (prs.clone(), names.clone(), conversations.clone());
            move |request: MergeClusterRequest| {
                let response = filled(&prs, &names, &conversations);
                let cluster = acted_on(response.as_ref(), &request.pull_request_urls)?;
                match (cluster.merge_by(), request.admin) {
                    (MergeBy::Unspecified, _) => {
                        return Err((409, "the cluster cannot merge".to_string()));
                    }
                    (MergeBy::Admin, false) => {
                        return Err((
                            409,
                            "the cluster has unapproved pull requests; merge it as an admin"
                                .to_string(),
                        ));
                    }
                    _ => {}
                }
                merges::start(cluster)
                    .map(|()| MergeClusterResponse {}.to_json())
                    .map_err(|error| (400, error))
            }
        })
        .post("/clusters/rebase", {
            let (prs, names, conversations) = (prs.clone(), names.clone(), conversations.clone());
            move |request: RebaseClusterRequest| {
                let response = filled(&prs, &names, &conversations);
                let cluster = acted_on(response.as_ref(), &request.pull_request_urls)?;
                rebases::start_cluster(cluster)
                    .map(|()| RebaseClusterResponse {}.to_json())
                    .map_err(|error| (400, error))
            }
        })
        .post("/prs", {
            let (prs, names, conversations) = (prs.clone(), names.clone(), conversations.clone());
            move |_: ListOpenPullRequestsRequest| serve_prs(&prs, &names, &conversations)
        })
        .post("/prs/search", |request: SearchPullRequestsRequest| {
            search::matching(&request.query)
                .map(|pull_request_urls| SearchPullRequestsResponse { pull_request_urls }.to_json())
                .map_err(|error| (502, error))
        })
        .post("/prs/rebase", |request: RebasePullRequestRequest| {
            rebases::start(&request.url)
                .map(|()| RebasePullRequestResponse {}.to_json())
                .map_err(|error| (400, error))
        })
        .post("/proposals/approve", {
            let prs = prs.clone();
            move |request: ApproveProposalRequest| {
                let closed = proposals::approve(
                    &request.id,
                    &request.reply,
                    &request.follow_up,
                    &request.diff,
                );
                match &closed {
                    Ok(closed) => info!(
                        "proposal approved",
                        proposal = request.id,
                        closed_pull_request = format!("{closed:?}")
                    ),
                    Err(error) => {
                        warn!("proposal approve failed", proposal = request.id, error = error)
                    }
                }
                // Search can list a PR as open for a while after it closes, so drop it here
                // rather than wait for the next refresh.
                if let (Ok(Some(url)), Some(response)) = (closed, &mut *prs.write().unwrap()) {
                    for cluster in &mut response.clusters {
                        cluster.pull_requests.retain(|pr| pr.url != url);
                    }
                    response.clusters.retain(|c| !c.pull_requests.is_empty());
                }
                ok(&ApproveProposalResponse {})
            }
        })
        .post("/investigations/start", |request: InvestigateRequest| {
            investigations::start(&request.proposal_id)
                .map(|()| InvestigateResponse {}.to_json())
                .map_err(|error| (400, error))
        })
        .post("/agents/message", {
            let conversations = conversations.clone();
            move |request: MessageAgentRequest| {
                agents::message(&conversations, &request.id, &request.message)
                    .map(|()| MessageAgentResponse {}.to_json())
                    .map_err(|error| (400, error))
            }
        })
        .post("/agents/close", {
            let conversations = conversations.clone();
            move |request: CloseAgentRequest| {
                agents::close(&conversations, &request.id)
                    .map(|()| CloseAgentResponse {}.to_json())
                    .map_err(|error| (404, error))
            }
        })
        .post("/markdown/render", |request: RenderMarkdownRequest| {
            render(&request.pull_request_url, &request.markdown)
                .map(|html| RenderMarkdownResponse { html }.to_json())
                .map_err(|error| (502, error))
        })
        .post("/proposals/dismiss", |request: DismissProposalRequest| {
            proposals::dismiss(&request.id);
            info!("proposal dismissed", proposal = request.id);
            ok(&DismissProposalResponse {})
        });

    investigations::watch();
    agents::watch(conversations);
    // Each request is answered on its own thread.
    let app = Arc::new(app);
    gem_room::serve(move |request| gem_http::respond(&app, &request), snapshots)
}

fn healthz() -> Reply {
    Ok(json!({ "status": "ok" }))
}

// Requests never call GitHub; a failed refresh keeps serving the last good result. Refreshes as
// soon as `changes` sees GitHub change, and every REFRESH_INTERVAL for what it does not watch.
fn cached<T, F>(refresh: F, changes: Changes) -> Cache<T>
where
    T: Send + Sync + 'static,
    F: Fn() -> Option<T> + Send + 'static,
{
    let cache = Cache::default();
    let writer = cache.clone();
    thread::spawn(move || {
        loop {
            if let Some(value) = refresh() {
                *writer.write().unwrap() = Some(value);
            }
            changes.changed(REFRESH_INTERVAL);
        }
    });
    cache
}

fn serve<T: ToJson>(cache: &Cache<T>) -> Reply {
    (cache.read().unwrap().as_ref())
        .map(ToJson::to_json)
        .ok_or((503, String::new()))
}

// Names and proposals arrive after the refresh that asked for them, so they are filled in per
// request.
fn serve_prs(
    cache: &Cache<ListOpenPullRequestsResponse>,
    names: &Names,
    conversations: &Conversations,
) -> Reply {
    (filled(cache, names, conversations))
        .map(|response| response.to_json())
        .ok_or((503, String::new()))
}

// The cluster the user acted on, holding exactly the pull requests they saw in it: clusters join and
// split as links and agents come and go, and an action never reaches a pull request the user did not
// see in the cluster.
fn acted_on<'a>(
    response: Option<&'a ListOpenPullRequestsResponse>,
    urls: &[String],
) -> Result<&'a PullRequestCluster, (u16, String)> {
    let cluster = (response.iter().flat_map(|r| &r.clusters))
        .find(|c| c.pull_requests.iter().any(|pr| Some(&pr.url) == urls.first()))
        .ok_or((404, "the cluster is gone".to_string()))?;
    let mut now: Vec<&str> = (cluster.pull_requests.iter()).map(|pr| pr.url.as_str()).collect();
    let mut seen: Vec<&str> = urls.iter().map(String::as_str).collect();
    now.sort();
    seen.sort();
    if now != seen {
        return Err((
            409,
            "the cluster's pull requests changed; look at it again".to_string(),
        ));
    }
    Ok(cluster)
}

// The cached clusters with their names and proposals, and the agents, as gem shows them.
fn filled(
    cache: &Cache<ListOpenPullRequestsResponse>,
    names: &Names,
    conversations: &Conversations,
) -> Option<ListOpenPullRequestsResponse> {
    let mut response = cache.read().unwrap().clone()?;
    agents::fill(&mut response, conversations);
    // Names are asked for the clusters as shown, after agents join them, so none is asked for a
    // cluster that never shows.
    names.request_missing(&response.clusters);
    names.fill(&mut response.clusters);
    proposals::fill(&mut response.clusters, &conversations.waiting());
    conversations.fill(&mut response.clusters);
    files::fill(&mut response.clusters);
    rebases::fill(&mut response.clusters);
    merges::fill(&mut response.clusters);
    Some(response)
}

fn open_pull_requests(
    conversations: Conversations,
    changes: Changes,
) -> Option<ListOpenPullRequestsResponse> {
    let search = gem_env::var("GEM_SEARCH").unwrap_or_default();
    let stdout = gh("open pull requests", &[
        "api",
        "graphql",
        "--paginate",
        "-f",
        &format!("query={PRS_QUERY}"),
        "-f",
        &format!("q=is:pr is:open author:{} {search}", login()),
        "--jq",
        PRS_JQ,
    ])?;
    let mut prs = gem_json::from_stream::<SearchedPullRequest>(&stdout)
        .inspect_err(|error| warn!("cannot read open pull requests", error = error))
        .ok()?;
    merge_states(&mut prs);
    changes.watch(watched(&prs));
    search::index(&prs);
    let clusters = cluster(&prs);
    conversations.refresh(&prs);
    Some(ListOpenPullRequestsResponse {
        clusters,
        login: login().to_string(),
        search,
        agents: Vec::new(),
    })
}

// mergeStateStatus is slow enough for GitHub to compute that the search's pages time out with it,
// so it is asked for MERGE_STATES_CHUNK PRs a request, all at once. A PR whose node errors, e.g. in
// a repository behind SAML enforcement the token is not authorized for, is neither behind nor
// conflicting, without taking the rest of its request's PRs with it.
const MERGE_STATES_CHUNK: usize = 25;

// Sets `behind` when GitHub will not merge a PR until it is rebased (its base branch requires it up
// to date, or it conflicts) and `conflicts` when it conflicts.
fn merge_states(prs: &mut [SearchedPullRequest]) {
    let results: Vec<Option<Value>> = thread::scope(|scope| {
        let requests: Vec<_> = (prs.chunks(MERGE_STATES_CHUNK))
            .map(|chunk| {
                let ids: Vec<&String> = chunk.iter().map(|pr| &pr.id).collect();
                let ids = json!(ids);
                let query = format!(
                    "query={{ nodes(ids: {ids}) {{ ... on PullRequest {{ id mergeStateStatus mergeable }} }} }}"
                );
                scope.spawn(move || {
                    gh_partial_json::<Value>("merge states", &["api", "graphql", "-f", &query])
                })
            })
            .collect();
        (requests.into_iter())
            .map(|request| request.join().ok().flatten())
            .collect()
    });
    let mut states = HashMap::new();
    for mut data in results.into_iter().flatten() {
        let nodes = (data.get_mut("data"))
            .and_then(|data| data.get_mut("nodes"))
            .and_then(Value::as_array_mut);
        for node in nodes.into_iter().flatten() {
            let id = node["id"].as_str().unwrap_or_default().to_string();
            states.insert(id, node.take());
        }
    }
    for pr in prs {
        let Some(state) = states.get(&pr.id) else {
            continue;
        };
        pr.conflicts = state["mergeable"] == "CONFLICTING";
        pr.behind = pr.conflicts || state["mergeStateStatus"] == "BEHIND";
    }
}

// What `changes` asks GitHub about each PR. Its own response changes with pushes, its title,
// comments, reviews and closing; its head commit's check runs and statuses with its checks; and its
// base branch with whether it is behind or conflicts.
fn watched(prs: &[SearchedPullRequest]) -> Vec<String> {
    (prs.iter())
        .flat_map(|pr| {
            let repo = format!("repos/{}/{}", pr.repository.owner, pr.repository.name);
            [
                format!("{repo}/pulls/{}", pr.number),
                format!("{repo}/commits/{}/check-runs?per_page=100", pr.head_oid),
                format!("{repo}/commits/{}/status", pr.head_oid),
                format!("{repo}/git/ref/heads/{}", pr.base),
            ]
        })
        .collect()
}

// Two PRs are linked when one is stacked on the other (its base branch is the other's head
// branch in the same repository) or one mentions the other. A cluster is every PR linked through
// any chain of links, not only direct ones.
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

    let links = prs.iter().enumerate().flat_map(|(i, pr)| {
        let stacked_on = by_head.get(&(&pr.repository, pr.base.as_str()));
        let mentioned_by = pr
            .mentioned_by
            .iter()
            .filter_map(|url| by_url.get(url.as_str()));
        stacked_on
            .into_iter()
            .chain(mentioned_by)
            .map(move |&j| (i, j))
    });
    let clusters = gem_cluster::components(prs.len(), links);

    let mut clusters: Vec<PullRequestCluster> = clusters
        .into_iter()
        .map(|group| group.into_iter().map(|i| &prs[i]).collect::<Vec<_>>())
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
                        merge_by: merge_by(pr) as i32,
                        checks_passing: pr.checks_passing,
                        checks_failing: pr.checks_failing,
                        proposals: Vec::new(),
                        behind: pr.behind,
                        conflicts: pr.conflicts,
                        checks: Some(pr.checks.clone()),
                        head_oid: pr.head_oid.clone(),
                        base: pr.base.clone(),
                        files: Vec::new(),
                        rebasing: false,
                        rebased: false,
                        merging: false,
                        no_proposal_reason: String::new(),
                        created_at: Some(pr.created_at.clone()),
                    })
                    .collect::<Vec<_>>(),
                name: String::new(),
                merge_by: MergeBy::Unspecified as i32,
                agents: Vec::new(),
            }
        })
        .map(|mut cluster| {
            cluster.merge_by = cluster_merge_by(&cluster) as i32;
            cluster
        })
        .collect();
    clusters.sort_by_key(|c| Reverse(c.updated_at.as_ref().map(|t| (t.seconds, t.nanos))));
    clusters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_action_reaches_only_the_cluster_the_user_saw() {
        let pr = |url: &str| PullRequest {
            url: url.to_string(),
            ..Default::default()
        };
        let response = ListOpenPullRequestsResponse {
            clusters: vec![PullRequestCluster {
                pull_requests: vec![pr("a"), pr("b")],
                ..Default::default()
            }],
            ..Default::default()
        };
        let urls = |urls: &[&str]| urls.iter().map(|u| u.to_string()).collect::<Vec<_>>();
        assert!(acted_on(Some(&response), &urls(&["b", "a"])).is_ok());
        let conflict = |urls: &[String]| acted_on(Some(&response), urls).err().map(|e| e.0);
        assert_eq!(conflict(&urls(&["a"])), Some(409));
        assert_eq!(conflict(&urls(&["a", "b", "c"])), Some(409));
        assert_eq!(conflict(&urls(&["c"])), Some(404));
    }
}
