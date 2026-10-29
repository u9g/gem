// gem's MCP servers. `server mcp --state <dir>` speaks MCP over stdio, so agents can read the
// clusters gem shows, and answers from the snapshot the running server keeps in <dir>'s database. The server
// registers it with Claude Code, as the user-scope server `gem`, whenever it starts.
//
// The agents gem starts to propose answers get `gem-agent` instead, over HTTP on the loopback
// interface, as the user-scope server is not there when they run in Claude Cloud: T3 Code relays
// a cloud session's calls to it. Each agent's bearer token proposes only on what it answers.

use std::{
    env,
    io::{BufRead, Write},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use gem_db::{
    Db,
    rusqlite::{OptionalExtension, params},
};
use serde_json::{Value, json};

use crate::investigations;
use crate::proposals::{self, GitHub};
use crate::proto::{ListOpenPullRequestsResponse, PullRequestCluster};
use crate::sandbox;
use crate::tokens::{Subject, Tokens};

const NAME: &str = "gem";

// Called after every refresh.
pub async fn save(response: &ListOpenPullRequestsResponse) {
    save_to(crate::db(), response);
}

fn save_to(db: &Db, response: &ListOpenPullRequestsResponse) {
    let saved = db.with(|db| {
        db.execute(
            "INSERT OR REPLACE INTO snapshot (id, response) VALUES (0, ?1)",
            params![serde_json::to_string(response).unwrap()],
        )
    });
    if let Err(error) = saved {
        tracing::warn!(%error, "cannot save the snapshot");
    }
}

// The snapshot's JSON, once the server saved one.
fn snapshot(db: &Db) -> Option<String> {
    db.with(|db| {
        db.query_row("SELECT response FROM snapshot", [], |row| row.get(0))
            .optional()
    })
    .ok()
    .flatten()
}

// Points Claude Code's `gem` MCP server at this build and this state directory, unless it
// already does. Builds are replaced on every update, so each one registers itself.
pub fn register() {
    let exe = env::current_exe().unwrap();
    let state = env::current_dir().unwrap();
    let wanted = json!({
        "type": "stdio",
        "command": exe,
        "args": ["mcp", "--state", state],
        "env": {},
    });
    let registered = env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .and_then(|home| std::fs::read(PathBuf::from(home).join(".claude.json")).ok())
        .and_then(|json| serde_json::from_slice::<Value>(&json).ok())
        .map(|config| config["mcpServers"][NAME].clone());
    if registered.as_ref() == Some(&wanted) {
        return;
    }
    let claude = |args: &[&str]| Command::new("claude").args(args).output();
    let _ = claude(&["mcp", "remove", "--scope", "user", NAME]);
    match claude(&[
        "mcp",
        "add-json",
        "--scope",
        "user",
        NAME,
        &wanted.to_string(),
    ]) {
        Ok(output) if output.status.success() => {}
        Ok(output) => eprintln!(
            "mcp: cannot register with Claude Code: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => eprintln!("mcp: cannot run claude: {error}"),
    }
}

// Answers MCP requests on stdin until it closes.
pub fn serve(args: &[String]) {
    let db = match args {
        [flag, dir] if flag == "--state" => Db::open(Path::new(dir).join(crate::DB_FILE)),
        _ => {
            eprintln!("usage: server mcp --state <the server's working directory>");
            std::process::exit(2);
        }
    }
    .unwrap_or_else(|error| panic!("{error}"));
    let mut stdout = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // Notifications have no id and get no response.
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let response = response(id, handle(&db, &request));
        let _ = writeln!(stdout, "{response}");
        let _ = stdout.flush();
    }
}

fn handle(db: &Db, request: &Value) -> Result<Value, (i64, String)> {
    let params = &request["params"];
    match request["method"].as_str().unwrap_or_default() {
        "initialize" => Ok(initialize(
            params,
            NAME,
            "gem groups the user's open pull requests into clusters of PRs linked by stacking or \
        mentions. `execute` runs a script that reads the clusters as gem last showed them and \
        returns only what you need.",
        )),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": [execute_tool(USER_API)] })),
        "tools/call" => Ok(tool_result(
            match params["name"].as_str().unwrap_or_default() {
                "execute" => code(&params["arguments"]).and_then(|code| {
                    let db = db.clone();
                    sandbox::run(code, READS, move |op, args| read(&db, op, &args))
                }),
                tool => Err(format!("no tool {tool}")),
            },
        )),
        method => Err((-32601, format!("no method {method}"))),
    }
}

fn response(id: Value, result: Result<Value, (i64, String)>) -> Value {
    match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    }
}

fn initialize(params: &Value, name: &str, instructions: &str) -> Value {
    json!({
        "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
        "capabilities": { "tools": {} },
        "serverInfo": { "name": name, "version": env!("CARGO_PKG_VERSION") },
        "instructions": instructions,
    })
}

// A tool's text, or its error for the agent to read and act on.
fn tool_result(text: Result<String, String>) -> Value {
    match text {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
        Err(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
    }
}

// Both servers have one tool, `execute`, in code mode as Cloudflare's and Datadog's MCP servers
// have: an agent writes a script against the `gem` API that `api` declares in TypeScript, and reads
// only what it returns. One script joins and filters clusters, investigations and proposals, where
// a tool per question would put each whole answer in the agent's context.
fn execute_tool(api: &str) -> Value {
    json!({
        "name": "execute",
        "description": format!(
            "Runs JavaScript with the `gem` object below, and returns what the script returns, as \
    JSON, after what it passed to `console.log`. Return only what you need: pick fields, filter and \
    join in the script. `gem`'s functions are synchronous, and throw an Error saying what is wrong. \
    The script has no network, file system or modules, and stops after 30 seconds.\n\n\
    ```ts\n{TYPES}\n\n{api}\n```"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "An async arrow function, such as `async () => \
    gem.clusters().map(c => c.name)`, or the body of one.",
                },
            },
            "required": ["code"],
            "additionalProperties": false,
        },
    })
}

fn code(arguments: &Value) -> Result<&str, String> {
    match arguments["code"].as_str() {
        Some(code) if !code.trim().is_empty() => Ok(code),
        _ => Err("`code` must be a script to run".into()),
    }
}

// The snapshot's types, as pbjson writes them: camelCase, with fields that are false, 0 or empty
// left out.
const TYPES: &str = r#"// A cluster of the user's open pull requests, linked by stacking or mentions.
interface Cluster {
  name?: string; // Short name for its shared work.
  updatedAt: string; // RFC 3339.
  mergeBy?: MergeBy; // Left out when it cannot merge.
  pullRequests: PullRequest[]; // Oldest first.
  agents?: Agent[]; // The agents gem started on its pull requests, newest first.
}
type MergeBy = "MERGE_BY_APPROVAL" | "MERGE_BY_ADMIN";
interface PullRequest {
  url: string;
  title: string;
  repository: { owner: string; name: string };
  number: number;
  base: string; // The branch it merges into.
  headOid: string;
  createdAt: string;
  approved?: boolean;
  checks?: { total?: number; done?: number; failed?: number };
  checksPassing?: boolean;
  checksFailing?: boolean;
  behind?: boolean; // It must be rebased onto `base` before it can merge.
  conflicts?: boolean;
  mergeBy?: MergeBy;
  rebasing?: boolean;
  merging?: boolean;
  // What agents propose answering the conversations and failed checks waiting on the user.
  proposals?: Proposal[];
  noProposalReason?: string; // Why the agent answering its failed checks proposed nothing.
}
interface Agent {
  id: string;
  title: string;
  state?: "AGENT_STATE_QUEUED" | "AGENT_STATE_WAITING"; // Left out while it works.
  pullRequestUrls: string[];
  reply?: string; // Its latest reply, in markdown.
  proposals?: Proposal[];
  startedAt: string;
}
interface Proposal {
  id: string;
  summary: string;
  pullRequestUrl: string;
  comment?: Comment; // The comment it answers.
  earlierComments?: Comment[]; // The comments before it in its review thread.
  checkFailure?: { id: string; headOid: string; checks: FailedCheck[] }; // The checks it answers.
  patch?: Patch;
  otherPatches?: Patch[];
  openPullRequest?: { repository: string; base: string; branch: string; title: string; body: string; patch: Patch };
  reply?: string;
  resolveThread?: boolean;
  followUp?: string;
  rerun?: boolean;
  closePullRequest?: boolean;
  requestedTool?: string;
  error?: string; // Why approving it last failed.
}
interface Comment { id: string; author: string; body: string; url: string; path?: string; line?: number; diffHunk?: string }
interface FailedCheck { id: string; name: string; conclusion: string; url: string; workflowRunId?: string; excerpt?: string }
interface Patch { diff: string; commitMessage: string; headOid: string; pullRequestUrl?: string }"#;

const USER_API: &str = "declare const gem: {
  // Every cluster gem shows, most recently updated first.
  clusters(): Cluster[];
  // The cluster holding the pull request at `url`, such as https://github.com/livekit/agents/pull/123.
  cluster(url: string): Cluster | null;
};";

// What both servers' scripts can read.
const READS: &[&str] = &["clusters", "cluster"];

// Answers `gem.<op>(...args)` from the snapshot.
fn read(db: &Db, op: &str, args: &[Value]) -> Result<Value, String> {
    let snapshot = snapshot(db)
        .ok_or("gem has not listed any clusters yet; is its server running?".to_string())?;
    let mut snapshot: Value = serde_json::from_str(&snapshot).map_err(|e| e.to_string())?;
    // Rendered HTML repeats the markdown next to it.
    strip_view(&mut snapshot);
    let clusters = snapshot["clusters"].as_array().cloned().unwrap_or_default();
    match op {
        "clusters" => Ok(Value::Array(clusters)),
        "cluster" => {
            let url = (args.first().and_then(Value::as_str))
                .ok_or("gem.cluster takes a pull request's URL")?;
            let url = url.trim().trim_end_matches('/');
            Ok(clusters
                .into_iter()
                .find(|cluster| {
                    let prs = cluster["pullRequests"].as_array();
                    prs.is_some_and(|prs| prs.iter().any(|pr| pr["url"].as_str() == Some(url)))
                })
                .unwrap_or_default())
        }
        op => Err(format!("no function gem.{op}")),
    }
}

// The URLs of the pull requests in the snapshot's cluster holding `pr`, or none when it holds none.
fn cluster_of(db: &Db, pr: &str) -> Vec<String> {
    let pr = pr.trim().trim_end_matches('/');
    let snapshot: ListOpenPullRequestsResponse = snapshot(db)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    (snapshot.clusters.into_iter())
        .map(|cluster| {
            cluster
                .pull_requests
                .into_iter()
                .map(|p| p.url)
                .collect::<Vec<_>>()
        })
        .find(|urls| urls.iter().any(|url| url == pr))
        .unwrap_or_default()
}

// A cluster as `gem.cluster` returns it, for prompts of agents that may not reach this server.
pub fn cluster_json(cluster: &PullRequestCluster) -> String {
    let mut cluster = serde_json::to_value(cluster).unwrap();
    strip_view(&mut cluster);
    serde_json::to_string_pretty(&cluster).unwrap()
}

// Drops what only gem's own view shows: rendered HTML, and PRs' file diffs.
fn strip_view(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|key, _| !key.ends_with("Html") && key != "files");
            object.values_mut().for_each(strip_view);
        }
        Value::Array(values) => values.iter_mut().for_each(strip_view),
        _ => {}
    }
}

// The MCP server of the agents gem starts to propose, named apart from the user's `gem`, which
// local sessions also load.
pub const AGENT_NAME: &str = "gem-agent";
// How agents' prompts introduce it.
pub const EXECUTE: &str = "The `execute` tool of the `gem-agent` MCP server runs JavaScript with \
a `gem` object, which its description declares in TypeScript. One script can read, filter and \
propose, and returns only what you pick.";
// A thread keeps the URL it was started with across server restarts, so the port stays the same.
// GEM_MCP_PORT overrides it.
const DEFAULT_PORT: u16 = 47474;

fn port() -> u16 {
    env::var("GEM_MCP_PORT")
        .ok()
        .and_then(|port| port.trim().parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

fn url() -> String {
    format!("http://127.0.0.1:{}/mcp", port())
}

// The --mcp-config of a new agent answering `subject`, as the path of a file in the server's
// working directory: T3 Code's CLI is a batch file on Windows, which cannot take JSON as an
// argument.
pub async fn agent_config(subject: &Subject) -> Result<String, String> {
    let path = Tokens::here().issue(subject, &url()).await?;
    Ok(path.to_string_lossy().into_owned())
}

// Serves `gem-agent` on the loopback interface until the server stops. A server restarting for an
// update can start before the one it replaces has let go of the port, so it keeps trying.
pub async fn listen() {
    let app = router(crate::db().clone(), Tokens::here(), proposals::Live);
    let port = port();
    let listener = loop {
        match tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
            Ok(listener) => break listener,
            Err(error) => {
                tracing::warn!(port, %error, "cannot listen for agents; trying again");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    };
    tracing::info!(port, "listening for agents");
    if let Err(error) = axum::serve(listener, app).await {
        tracing::error!(%error, "stopped listening for agents");
    }
}

struct Endpoint<G> {
    // With the snapshot, proposals and investigations.
    db: Db,
    tokens: Tokens,
    github: G,
}

// Streamable HTTP without streams: each POST to /mcp is one JSON-RPC message, answered with JSON.
fn router(db: Db, tokens: Tokens, github: impl GitHub) -> Router {
    let endpoint = Arc::new(Endpoint { db, tokens, github });
    Router::new()
        .route("/mcp", post(answer))
        .with_state(endpoint)
}

async fn answer<G: GitHub>(
    State(endpoint): State<Arc<Endpoint<G>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let token = (headers.get(header::AUTHORIZATION))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    let Some(subject) = endpoint.tokens.subject(token.trim()).await else {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "no agent of gem's has this token",
        )
            .into_response();
    };
    let Ok(request) = serde_json::from_slice::<Value>(&body) else {
        let error = response(Value::Null, Err((-32700, "the body is not JSON".into())));
        return (StatusCode::BAD_REQUEST, Json(error)).into_response();
    };
    // Notifications have no id and get no response.
    let Some(id) = request.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };
    Json(response(
        id,
        handle_agent(&endpoint, &subject, &request).await,
    ))
    .into_response()
}

async fn handle_agent<G: GitHub>(
    endpoint: &Arc<Endpoint<G>>,
    subject: &Subject,
    request: &Value,
) -> Result<Value, (i64, String)> {
    let params = &request["params"];
    match request["method"].as_str().unwrap_or_default() {
        "initialize" => Ok(initialize(
            params,
            AGENT_NAME,
            "gem shows the user what agents propose doing about their pull requests, to approve \
        or dismiss. `execute` runs a script with `gem.propose`, which records a proposal about \
        what this thread answers, `gem.cluster`, which reads a cluster as gem shows it now, and \
        `gem.investigations`, which has what earlier agents found investigating failures.",
        )),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": [execute_tool(&agent_api(subject))] })),
        "tools/call" => Ok(tool_result(
            match params["name"].as_str().unwrap_or_default() {
                "execute" => match code(&params["arguments"]) {
                    Ok(code) => execute(endpoint, subject, code.to_string()).await,
                    Err(error) => Err(error),
                },
                tool => Err(format!("no tool {tool}")),
            },
        )),
        method => Err((-32601, format!("no method {method}"))),
    }
}

// Runs an agent's script on a blocking thread, as QuickJS cannot wait: `gem.propose` waits there
// for GitHub to check the proposal.
async fn execute<G: GitHub>(
    endpoint: &Arc<Endpoint<G>>,
    subject: &Subject,
    code: String,
) -> Result<String, String> {
    let endpoint = Arc::clone(endpoint);
    let subject = subject.clone();
    let runtime = tokio::runtime::Handle::current();
    let ops = [READS, &["investigations", "propose"]].concat();
    tokio::task::spawn_blocking(move || {
        sandbox::run(&code, &ops, move |op, args| match op {
            "investigations" => Ok(Value::Array(investigations::archived(&endpoint.db))),
            "propose" => {
                let proposal = args.into_iter().next().unwrap_or_default();
                runtime.block_on(propose(&endpoint, &subject, proposal))
            }
            op => read(&endpoint.db, op, &args),
        })
    })
    .await
    .map_err(|error| format!("the script stopped: {error}"))?
}

async fn propose<G: GitHub>(
    endpoint: &Endpoint<G>,
    subject: &Subject,
    proposal: Value,
) -> Result<Value, String> {
    let request: proposals::Request =
        serde_json::from_value(proposal).map_err(|e| e.to_string())?;
    let cluster = cluster_of(&endpoint.db, &request.pr);
    let proposed =
        proposals::propose(&endpoint.github, &endpoint.db, subject, &cluster, request).await;
    match &proposed {
        Ok(id) => tracing::info!(proposal = %id, subject = subject.id(), "proposal recorded"),
        Err(error) => tracing::info!(subject = subject.id(), %error, "proposal rejected"),
    }
    proposed.map(Value::String)
}

// The `gem` of an agent answering `subject`, whose proposals take the steps it can.
fn agent_api(subject: &Subject) -> String {
    let mut steps = String::new();
    let mut excerpts = "";
    match subject {
        Subject::Comment {
            thread: Some(_), ..
        } => steps.push_str("  resolve_thread?: boolean; // Resolve the review thread.\n"),
        Subject::CheckFailure { .. } => {
            steps.push_str("  rerun?: boolean; // Rerun the failed GitHub Actions jobs.\n");
            excerpts = "
  // For each check it answers, the log lines that show why it failed: the error itself, at most
  // 10, no setup or timestamps.
  excerpts: { check: string /* The check's node ID. */; lines: string }[];";
        }
        _ => {}
    }
    format!(
        "declare const gem: {{
  // Every cluster gem shows, most recently updated first.
  clusters(): Cluster[];
  // The cluster holding the pull request at `url`, as gem shows it now.
  cluster(url: string): Cluster | null;
  // The investigations earlier agents finished into failed checks and other problems on the
  // user's pull requests, newest first. `reply` says what each found. Find ones into the same
  // failure by the failing test's name or words of the error in `title`, `failedChecks` and `reply`.
  investigations(): {{ title: string; pullRequestUrls: string[]; startedAt: string; failedChecks: string; reply: string }}[];
  // Records a proposal for the user to approve or dismiss in gem, and returns its ID. It is
  // checked first: an Error says what is wrong, to fix and propose again. Approving runs its steps
  // in this order: follow_up, patch, other_patches, open_pull_request, reply, resolve_thread,
  // rerun, close_pull_request.
  propose(proposal: NewProposal): string;
}};
interface NewProposal {{
  pr: string; // The pull request it is about: {prs}.
  summary: string; // One line saying what approving does; for request_tool, \"<tool name>: <one line>\".{excerpts}
  // A change to pr's code, as `git diff` prints it against its head, and its commit message.
  // Never commit or push it yourself.
  patch?: string;
  message?: string;
  // Changes to the other pull requests of pr's cluster, such as a fix in the pull request that
  // causes the problem, or the same fix in another repository's matching pull request. Each is
  // `git diff` against that pull request's head. Never commit or push them yourself.
  other_patches?: {{ pr: string; patch: string; message: string }}[];
  // A new pull request, such as a fix on the base branch for a test that fails there too.
  // Approving commits patch on branch off base, pushes it to repository, or to the user's fork of
  // it when they cannot push there, and opens it. Never commit, push or open it yourself.
  open_pull_request?: {{
    repository: string; // owner/name it merges into, such as the repository pr merges into, not a fork.
    base: string; // The branch it merges into.
    branch: string; // A new branch, which neither repository nor the user's fork has.
    title: string;
    body: string; // Markdown description, posted as the user. Must link pr.
    patch: string; // As `git diff` prints it against base's head.
    message: string;
  }};
  reply?: string; // Markdown posted as the user, in the first person.
  follow_up?: string; // A prompt for a T3 Code agent that approving starts.
  close_pull_request?: boolean; // Close the pull request without merging it.
{steps}  // Instead of steps: markdown describing a tool you are missing, what it would do and why
  // this needs it.
  request_tool?: string;
}}",
        prs = subject.prs().join(" or "),
    )
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;
    use crate::proposals::FetchedComment;
    use crate::proto::{CheckFailure, Comment, FailedCheck, Proposal};
    use crate::tokens::tests::{db, temp_dir, token};

    const PR: &str = "https://github.com/o/r/pull/1";
    const OTHER: &str = "https://github.com/o/r/pull/2";

    fn comment(id: &str) -> Comment {
        Comment {
            id: id.into(),
            author: "reviewer".into(),
            body: format!("{id}'s body"),
            url: format!("{PR}#{id}"),
            body_html: String::new(),
            ..Default::default()
        }
    }

    // GitHub as it would answer about PR, where every patch applies.
    struct Fake;

    impl GitHub for Fake {
        async fn comment(&self, id: &str) -> Result<FetchedComment, String> {
            Ok(FetchedComment {
                comment: comment(id),
                pull_request: PR.into(),
            })
        }

        async fn check_failure(&self, _: &str, id: &str) -> Result<CheckFailure, String> {
            Ok(CheckFailure {
                id: id.into(),
                head_oid: "abc".into(),
                checks: vec![FailedCheck {
                    id: "CR_1".into(),
                    name: "test".into(),
                    conclusion: "FAILURE".into(),
                    url: String::new(),
                    workflow_run_id: 7,
                    excerpt: String::new(),
                }],
            })
        }

        async fn review_thread(&self, _: &str) -> Result<(String, Vec<Comment>), String> {
            Ok((PR.into(), vec![comment("PRRC_0"), comment("PRRC_1")]))
        }

        async fn check_patch(&self, _: &str, _: &str) -> Result<String, String> {
            Ok("abc".into())
        }

        async fn check_new_pull_request(
            &self,
            repository: &str,
            base: &str,
            branch: &str,
            _: &str,
        ) -> Result<String, String> {
            match (repository, base, branch) {
                ("o/r", "master", "test/fix") => Ok("def".into()),
                _ => Err(format!("{repository} {base} {branch}")),
            }
        }

        async fn render(&self, _: &str, markdown: &str) -> Result<String, String> {
            Ok(format!("<p>{markdown}</p>"))
        }
    }

    struct Server {
        db: Db,
        tokens: Tokens,
        app: Router,
    }

    impl Server {
        fn new(name: &str) -> Self {
            let db = db();
            let tokens = Tokens::new(db.clone(), temp_dir(name));
            Self {
                app: router(db.clone(), tokens.clone(), Fake),
                db,
                tokens,
            }
        }

        fn db(&self) -> &Db {
            &self.db
        }

        // A token for an agent answering `subject`.
        async fn token(&self, subject: &Subject) -> String {
            token(
                &self
                    .tokens
                    .issue(subject, "http://127.0.0.1:1/mcp")
                    .await
                    .unwrap(),
            )
        }

        async fn post(&self, token: Option<&str>, body: Value) -> (StatusCode, Value) {
            let mut request = Request::post("/mcp").header("content-type", "application/json");
            if let Some(token) = token {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            let request = request.body(Body::from(body.to_string())).unwrap();
            let response = self.app.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
        }

        // The result of running `code` with `execute`.
        async fn execute(&self, token: &str, code: &str) -> Value {
            let (status, response) = self
                .post(
                    Some(token),
                    json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "tools/call",
                        "params": { "name": "execute", "arguments": { "code": code } },
                    }),
                )
                .await;
            assert_eq!(status, StatusCode::OK);
            response["result"].clone()
        }

        // The result of a script that passes `proposal` to `gem.propose`.
        async fn propose(&self, token: &str, proposal: Value) -> Value {
            self.execute(token, &format!("async () => gem.propose({proposal})"))
                .await
        }

        fn proposals(&self) -> Vec<Proposal> {
            proposals::load(self.db())
        }
    }

    fn in_thread() -> Subject {
        Subject::Comment {
            pr: PR.into(),
            comment: "PRRC_1".into(),
            thread: Some("PRRT_1".into()),
        }
    }

    fn initialize() -> Value {
        json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {} })
    }

    fn text(result: &Value) -> &str {
        result["content"][0]["text"].as_str().unwrap()
    }

    #[tokio::test]
    async fn requests_without_a_valid_token_are_unauthorized() {
        let server = Server::new("unauthorized");
        let token = server.token(&in_thread()).await;
        assert_eq!(
            server.post(None, initialize()).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            server.post(Some("0123abcd"), initialize()).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (status, response) = server.post(Some(&token), initialize()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["result"]["serverInfo"]["name"], AGENT_NAME);

        server.tokens.revoke("PRRC_1").await;
        assert_eq!(
            server.post(Some(&token), initialize()).await.0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn prompts_name_the_agents_server() {
        assert!(EXECUTE.contains(&format!("`{AGENT_NAME}`")));
    }

    #[tokio::test]
    async fn notifications_are_accepted_without_a_response() {
        let server = Server::new("notification");
        let token = server.token(&in_thread()).await;
        let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(
            server.post(Some(&token), notification).await.0,
            StatusCode::ACCEPTED
        );
    }

    #[tokio::test]
    async fn propose_offers_the_steps_its_subject_can_take() {
        let server = Server::new("tools");
        let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        let api = async |subject: Subject| {
            let token = server.token(&subject).await;
            let (_, response) = server.post(Some(&token), list.clone()).await;
            let tools = response["result"]["tools"].as_array().unwrap().clone();
            let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
            assert_eq!(names, ["execute"]);
            tools[0]["description"].as_str().unwrap().to_string()
        };

        let api = api(in_thread()).await;
        assert!(api.contains("propose(proposal: NewProposal)"), "{api}");
        assert!(api.contains("resolve_thread?: boolean"));
        assert!(!api.contains("rerun?: boolean; // Rerun") && !api.contains("excerpts: {"));

        let token = server
            .token(&Subject::CheckFailure {
                pr: PR.into(),
                id: "abc,CR_1".into(),
            })
            .await;
        let (_, response) = server.post(Some(&token), list.clone()).await;
        let api = response["result"]["tools"][0]["description"]
            .as_str()
            .unwrap();
        assert!(api.contains("rerun?: boolean; // Rerun") && api.contains("excerpts: {"));
        assert!(!api.contains("resolve_thread?: boolean"));
    }

    #[tokio::test]
    async fn a_valid_proposal_is_saved_with_its_subject() {
        let server = Server::new("valid");
        let token = server.token(&in_thread()).await;
        let result = server
            .propose(
                &token,
                json!({
                    "pr": PR,
                    "summary": "Rename it",
                    "patch": "diff --git a/x b/x\n",
                    "message": "Rename it",
                    "reply": "Renamed.",
                    "resolve_thread": true,
                }),
            )
            .await;
        assert_eq!(result["isError"], Value::Null, "{result}");

        let [proposal] = &server.proposals()[..] else {
            panic!("{:?}", server.proposals());
        };
        assert_eq!(text(&result), proposal.id);
        assert_eq!(proposal.pull_request_url, PR);
        assert_eq!(proposal.comment.as_ref().unwrap().id, "PRRC_1");
        assert_eq!(proposal.thread_id, "PRRT_1");
        let earlier: Vec<&str> = (proposal.earlier_comments.iter())
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(earlier, ["PRRC_0"]);
        assert_eq!(proposal.patch.as_ref().unwrap().head_oid, "abc");
        assert_eq!(proposal.reply_html, "<p>Renamed.</p>");
        assert!(proposal.resolve_thread);
        assert_eq!(proposal.investigation_id, "");
    }

    #[tokio::test]
    async fn a_top_level_reply_opens_by_quoting_the_comment() {
        let server = Server::new("quote");
        let subject = Subject::Comment {
            pr: PR.into(),
            comment: "IC_1".into(),
            thread: None,
        };
        let token = server.token(&subject).await;
        let result = server
            .propose(
                &token,
                json!({ "pr": PR, "summary": "Reply", "reply": "Done." }),
            )
            .await;
        assert_eq!(result["isError"], Value::Null, "{result}");
        let [proposal] = &server.proposals()[..] else {
            panic!("{:?}", server.proposals());
        };
        assert_eq!(proposal.reply, "> IC_1's body\n\nDone.");
        assert_eq!(proposal.reply_html, "<p>> IC_1's body\n\nDone.</p>");
    }

    #[tokio::test]
    async fn an_investigation_proposes_on_any_of_its_clusters_pull_requests() {
        let server = Server::new("investigation");
        let subject = Subject::Investigation {
            id: "inv".into(),
            prs: vec![PR.into(), OTHER.into()],
        };
        let token = server.token(&subject).await;
        let result = server
            .propose(
                &token,
                json!({ "pr": OTHER, "summary": "Fix it", "follow_up": "Fix it." }),
            )
            .await;
        assert_eq!(result["isError"], Value::Null, "{result}");
        let [proposal] = &server.proposals()[..] else {
            panic!("{:?}", server.proposals());
        };
        assert_eq!(proposal.investigation_id, "inv");
        assert_eq!(proposal.pull_request_url, OTHER);
        assert_eq!(proposal.comment, None);
    }

    #[tokio::test]
    async fn a_proposal_opens_a_pull_request_that_links_its_own() {
        let server = Server::new("open-pull-request");
        let subject = Subject::Investigation {
            id: "inv".into(),
            prs: vec![PR.into()],
        };
        let token = server.token(&subject).await;
        let open = |body: &str| {
            json!({
                "pr": PR,
                "summary": "Fix the test on master",
                "open_pull_request": {
                    "repository": "https://github.com/o/r",
                    "base": "master",
                    "branch": "test/fix",
                    "title": "Fix the test",
                    "body": body,
                    "patch": "diff --git a/x b/x\n",
                    "message": "Fix the test",
                },
            })
        };

        for body in ["Fixes the test.".to_string(), format!("Found on {PR}0.")] {
            let result = server.propose(&token, open(&body)).await;
            assert_eq!(result["isError"], true, "{result}");
            assert!(server.proposals().is_empty());
        }

        let body = format!("Fixes the test that fails on {PR}.");
        let result = server.propose(&token, open(&body)).await;
        assert_eq!(result["isError"], Value::Null, "{result}");
        let [proposal] = &server.proposals()[..] else {
            panic!("{:?}", server.proposals());
        };
        let new = proposal.open_pull_request.as_ref().unwrap();
        assert_eq!(new.repository, "o/r");
        assert_eq!(new.base, "master");
        assert_eq!(new.branch, "test/fix");
        assert_eq!(new.body_html, format!("<p>{body}</p>"));
        let patch = new.patch.as_ref().unwrap();
        assert_eq!(patch.head_oid, "def");
        assert_eq!(patch.commit_message, "Fix the test");
    }

    #[tokio::test]
    async fn a_check_failure_proposal_keeps_the_checks_it_excerpts() {
        let server = Server::new("checks");
        let subject = Subject::CheckFailure {
            pr: PR.into(),
            id: "abc,CR_1".into(),
        };
        let token = server.token(&subject).await;
        let result = server
            .propose(
                &token,
                json!({
                    "pr": PR,
                    "summary": "Rerun it",
                    "rerun": true,
                    "excerpts": [{ "check": "CR_1", "lines": "error: timed out" }],
                }),
            )
            .await;
        assert_eq!(result["isError"], Value::Null, "{result}");
        let [proposal] = &server.proposals()[..] else {
            panic!("{:?}", server.proposals());
        };
        let failure = proposal.check_failure.as_ref().unwrap();
        assert_eq!(failure.id, "abc,CR_1");
        assert_eq!(failure.checks[0].excerpt, "error: timed out");
    }

    #[tokio::test]
    async fn an_invalid_proposal_is_a_tool_error_and_saves_nothing() {
        let server = Server::new("invalid");
        let token = server.token(&in_thread()).await;
        let invalid = [
            // No steps.
            json!({ "pr": PR, "summary": "Do nothing" }),
            json!({ "pr": PR, "summary": "Fix it", "patch": "diff --git a/x b/x\n" }),
            json!({ "pr": PR, "summary": "Rerun it", "rerun": true }),
            json!({ "pr": PR, "summary": "Reply", "replyy": "Typo." }),
            json!({ "summary": "Reply", "reply": "No PR." }),
        ];
        for arguments in invalid {
            let result = server.propose(&token, arguments.clone()).await;
            assert_eq!(result["isError"], true, "{arguments} gave {result}");
            assert!(!text(&result).is_empty());
        }
        assert!(server.proposals().is_empty());
    }

    #[tokio::test]
    async fn a_token_proposes_only_on_its_own_subject() {
        let server = Server::new("scope");
        let token = server.token(&in_thread()).await;
        let result = server
            .propose(
                &token,
                json!({ "pr": OTHER, "summary": "Reply", "reply": "Elsewhere." }),
            )
            .await;
        assert_eq!(result["isError"], true);
        assert!(text(&result).contains(PR), "{result}");
        assert!(server.proposals().is_empty());

        // Only a check failure's agent can rerun, so a comment's cannot reach one.
        let result = server
            .propose(
                &token,
                json!({
                    "pr": PR,
                    "summary": "Rerun",
                    "rerun": true,
                    "excerpts": [{ "check": "CR_1", "lines": "error" }],
                }),
            )
            .await;
        assert_eq!(result["isError"], true);
        assert!(server.proposals().is_empty());
    }

    #[tokio::test]
    async fn a_proposal_patches_other_pull_requests_of_its_cluster() {
        let server = Server::new("other-patches");
        let cluster = |urls: &[&str]| PullRequestCluster {
            pull_requests: (urls.iter())
                .map(|url| crate::proto::PullRequest {
                    url: url.to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        const ELSEWHERE: &str = "https://github.com/o/r/pull/3";
        let snapshot = ListOpenPullRequestsResponse {
            clusters: vec![cluster(&[PR, OTHER]), cluster(&[ELSEWHERE])],
            ..Default::default()
        };
        save_to(server.db(), &snapshot);
        let token = server.token(&in_thread()).await;
        let other_patch = |pr: &str| {
            json!({
                "pr": PR,
                "summary": "Port it",
                "reply": "Ported.",
                "other_patches": [{ "pr": pr, "patch": "diff --git a/x b/x\n", "message": "Port it" }],
            })
        };

        for pr in [PR, ELSEWHERE] {
            let result = server.propose(&token, other_patch(pr)).await;
            assert_eq!(result["isError"], true, "{pr} gave {result}");
        }
        let mut twice = other_patch(OTHER);
        let patch = twice["other_patches"][0].clone();
        twice["other_patches"].as_array_mut().unwrap().push(patch);
        let result = server.propose(&token, twice).await;
        assert_eq!(result["isError"], true, "{result}");
        assert!(server.proposals().is_empty());

        let result = server.propose(&token, other_patch(OTHER)).await;
        assert_eq!(result["isError"], Value::Null, "{result}");
        let [proposal] = &server.proposals()[..] else {
            panic!("{:?}", server.proposals());
        };
        assert_eq!(proposal.pull_request_url, PR);
        assert_eq!(proposal.patch, None);
        let [patch] = &proposal.other_patches[..] else {
            panic!("{:?}", proposal.other_patches);
        };
        assert_eq!(patch.pull_request_url, OTHER);
        assert_eq!(patch.head_oid, "abc");
    }

    #[tokio::test]
    async fn scripts_read_the_snapshot() {
        let server = Server::new("cluster");
        let snapshot = json!({ "clusters": [{
            "name": "Renames",
            "pullRequests": [{ "url": PR, "title": "Rename", "titleHtml": "<p>Rename</p>" }],
        }] });
        server
            .db()
            .with(|db| {
                db.execute(
                    "INSERT INTO snapshot (id, response) VALUES (0, ?1)",
                    [snapshot.to_string()],
                )
            })
            .unwrap();
        let token = server.token(&in_thread()).await;
        let result = server
            .execute(&token, &format!("async () => gem.cluster('{PR}/')"))
            .await;
        let cluster: Value = serde_json::from_str(text(&result)).unwrap();
        assert_eq!(cluster["name"], "Renames");
        assert_eq!(cluster["pullRequests"][0]["titleHtml"], Value::Null);

        let script = format!(
            "const none = gem.cluster('{OTHER}');
             const titles = gem.clusters().flatMap(c => c.pullRequests.map(pr => pr.title));
             return {{ none, titles }};"
        );
        let result = server.execute(&token, &script).await;
        assert_eq!(
            text(&result),
            serde_json::to_string_pretty(&json!({ "none": null, "titles": ["Rename"] })).unwrap()
        );

        // The user's own server reads the same snapshot.
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "execute", "arguments": { "code": "gem.clusters()[0].name" } },
        });
        assert_eq!(text(&handle(server.db(), &request).unwrap()), "Renames");
    }

    #[tokio::test]
    async fn scripts_read_the_closed_investigations() {
        let server = Server::new("search");
        let investigation = json!({
            "id": "18f",
            "title": "Investigate test on r#1",
            "pullRequestUrl": PR,
            "reply": "test_join times out when the room closes first.",
        });
        server
            .db()
            .with(|db| {
                db.execute(
                    "INSERT INTO investigations (id, investigation, closed) VALUES ('18f', ?1, 1)",
                    [investigation.to_string()],
                )
            })
            .unwrap();
        let token = server.token(&in_thread()).await;
        let script = "async () => gem.investigations()
            .filter(i => /test_join/i.test(i.title + i.failedChecks + i.reply))
            .map(i => i.pullRequestUrls)";
        let result = server.execute(&token, script).await;
        let found: Value = serde_json::from_str(text(&result)).unwrap();
        assert_eq!(found, json!([[PR]]));

        let result = server.execute(&token, "async () => gem.cluster()").await;
        assert_eq!(result["isError"], true);
        let result = server.execute(&token, "").await;
        assert_eq!(result["isError"], true);
    }
}
