// gem's MCP servers. `server mcp --state <dir>` speaks MCP over stdio, so agents can read the
// clusters gem shows, and answers from the snapshot the running server keeps in <dir>. The server
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
use serde_json::{Value, json};

use crate::proposals::{self, GitHub};
use crate::proto::{ListOpenPullRequestsResponse, PullRequestCluster};
use crate::tokens::{self, Subject, Tokens};

// The clusters as gem last showed them, with their names and proposals, relative to the
// server's working directory.
const SNAPSHOT: &str = "clusters.json";
const NAME: &str = "gem";

// Called after every refresh. A reader never sees a half-written snapshot.
pub async fn save(response: &ListOpenPullRequestsResponse) {
    let json = serde_json::to_vec(response).unwrap();
    let partial = format!("{SNAPSHOT}.partial");
    if tokio::fs::write(&partial, json).await.is_ok() {
        let _ = tokio::fs::rename(&partial, SNAPSHOT).await;
    }
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
    let state = match args {
        [flag, dir] if flag == "--state" => PathBuf::from(dir),
        _ => {
            eprintln!("usage: server mcp --state <the server's working directory>");
            std::process::exit(2);
        }
    };
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
        let response = response(id, handle(&state, &request));
        let _ = writeln!(stdout, "{response}");
        let _ = stdout.flush();
    }
}

fn handle(state: &Path, request: &Value) -> Result<Value, (i64, String)> {
    let params = &request["params"];
    match request["method"].as_str().unwrap_or_default() {
        "initialize" => Ok(initialize(
            params,
            NAME,
            "gem groups the user's open pull requests into clusters of PRs linked by stacking or \
        mentions. These tools read the clusters as gem last showed them.",
        )),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => Ok(tool_result(call(
            state,
            params["name"].as_str().unwrap_or_default(),
            &params["arguments"],
        ))),
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

fn tools() -> Value {
    json!([
        get_cluster(),
        {
            "name": "list_clusters",
            "description": "Every cluster gem shows, most recently updated first: its name and \
    its pull requests' URLs and titles.",
            "inputSchema": { "type": "object", "properties": {} },
        },
    ])
}

fn get_cluster() -> Value {
    json!(
        {
            "name": "get_cluster",
            "description": "The cluster holding a pull request: its name, and each pull \
    request's title, review and check state, and the conversations on it still waiting on the \
    user, with what agents propose answering.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pull_request_url": {
                        "type": "string",
                        "description": "Any pull request in the cluster, e.g. \
    https://github.com/livekit/agents/pull/123",
                    },
                },
                "required": ["pull_request_url"],
            },
        }
    )
}

fn call(state: &Path, tool: &str, arguments: &Value) -> Result<String, String> {
    let snapshot = std::fs::read(state.join(SNAPSHOT))
        .map_err(|_| "gem has not listed any clusters yet; is its server running?".to_string())?;
    let mut snapshot: Value = serde_json::from_slice(&snapshot).map_err(|e| e.to_string())?;
    // Rendered HTML repeats the markdown next to it.
    strip_html(&mut snapshot);
    let clusters = snapshot["clusters"].as_array().cloned().unwrap_or_default();
    match tool {
        "get_cluster" => {
            let url = arguments["pull_request_url"].as_str().unwrap_or_default();
            let url = url.trim().trim_end_matches('/');
            let cluster = clusters
                .into_iter()
                .find(|cluster| {
                    let prs = cluster["pullRequests"].as_array();
                    prs.is_some_and(|prs| prs.iter().any(|pr| pr["url"].as_str() == Some(url)))
                })
                .ok_or(format!("no open pull request {url} in gem's clusters"))?;
            Ok(serde_json::to_string_pretty(&cluster).unwrap())
        }
        "list_clusters" => {
            let clusters: Vec<Value> = (clusters.iter())
                .map(|cluster| {
                    let prs = cluster["pullRequests"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    json!({
                        "name": cluster["name"],
                        "pullRequests": prs
                            .iter()
                            .map(|pr| json!({ "url": pr["url"], "title": pr["title"] }))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(serde_json::to_string_pretty(&clusters).unwrap())
        }
        tool => Err(format!("no tool {tool}")),
    }
}

// A cluster as `get_cluster` returns it, for prompts of agents that may not reach this server.
pub fn cluster_json(cluster: &PullRequestCluster) -> String {
    let mut cluster = serde_json::to_value(cluster).unwrap();
    strip_html(&mut cluster);
    serde_json::to_string_pretty(&cluster).unwrap()
}

fn strip_html(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|key, _| !key.ends_with("Html"));
            object.values_mut().for_each(strip_html);
        }
        Value::Array(values) => values.iter_mut().for_each(strip_html),
        _ => {}
    }
}

// The MCP server of the agents gem starts to propose, named apart from the user's `gem`, which
// local sessions also load.
pub const AGENT_NAME: &str = "gem-agent";
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

// The --mcp-config of a new agent answering `subject`, as the path of a file in the state
// directory: T3 Code's CLI is a batch file on Windows, which cannot take JSON as an argument.
pub async fn agent_config(subject: &Subject) -> Result<String, String> {
    let path = Tokens::here().issue(subject, &url()).await?;
    Ok(path.to_string_lossy().into_owned())
}

// Serves `gem-agent` on the loopback interface until the server stops. A server restarting for an
// update can start before the one it replaces has let go of the port, so it keeps trying.
pub async fn listen() {
    let app = router(env::current_dir().unwrap(), proposals::Live);
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
    // The server's working directory, with the snapshot, proposals and tokens.
    state: PathBuf,
    tokens: Tokens,
    github: G,
}

// Streamable HTTP without streams: each POST to /mcp is one JSON-RPC message, answered with JSON.
fn router(state: PathBuf, github: impl GitHub) -> Router {
    let endpoint = Arc::new(Endpoint {
        tokens: Tokens::new(state.join(tokens::DIR)),
        state,
        github,
    });
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
    endpoint: &Endpoint<G>,
    subject: &Subject,
    request: &Value,
) -> Result<Value, (i64, String)> {
    let params = &request["params"];
    match request["method"].as_str().unwrap_or_default() {
        "initialize" => Ok(initialize(
            params,
            AGENT_NAME,
            "gem shows the user what agents propose doing about their pull requests, to approve \
        or dismiss. `propose` records a proposal about what this thread answers, and \
        `get_cluster` reads a cluster as gem shows it now.",
        )),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": [get_cluster(), propose_tool(subject)] })),
        "tools/call" => {
            let arguments = &params["arguments"];
            Ok(tool_result(
                match params["name"].as_str().unwrap_or_default() {
                    "get_cluster" => call(&endpoint.state, "get_cluster", arguments),
                    "propose" => propose(endpoint, subject, arguments).await,
                    tool => Err(format!("no tool {tool}")),
                },
            ))
        }
        method => Err((-32601, format!("no method {method}"))),
    }
}

async fn propose<G: GitHub>(
    endpoint: &Endpoint<G>,
    subject: &Subject,
    arguments: &Value,
) -> Result<String, String> {
    let request: proposals::Request =
        serde_json::from_value(arguments.clone()).map_err(|e| e.to_string())?;
    let dir = endpoint.state.join(proposals::DIR);
    let proposed = proposals::propose(&endpoint.github, &dir, subject, request).await;
    match &proposed {
        Ok(id) => tracing::info!(proposal = %id, subject = subject.id(), "proposal recorded"),
        Err(error) => tracing::info!(subject = subject.id(), %error, "proposal rejected"),
    }
    proposed.map(|id| format!("Recorded proposal {id}."))
}

// `propose`, with the steps its subject can take.
fn propose_tool(subject: &Subject) -> Value {
    let mut properties = json!({
        "pr": {
            "type": "string",
            "description": format!(
                "The pull request the proposal is about: {}",
                subject.prs().join(" or ")
            ),
        },
        "summary": {
            "type": "string",
            "description": "One line saying what approving does; for `request_tool`, \
    \"<tool name>: <one line>\".",
        },
        "patch": {
            "type": "string",
            "description": "A change to the pull request's code, as `git diff` prints it \
    against its head. Never commit or push it yourself.",
        },
        "message": { "type": "string", "description": "The commit message for `patch`." },
        "reply": {
            "type": "string",
            "description": "Markdown posted as the user, in the first person.",
        },
        "follow_up": {
            "type": "string",
            "description": "A prompt for a T3 Code agent that approving starts.",
        },
        "close_pull_request": {
            "type": "boolean",
            "description": "Close the pull request without merging it.",
        },
        "request_tool": {
            "type": "string",
            "description": "Instead of steps: markdown describing a tool you are missing, what \
    it would do and why this needs it.",
        },
    });
    let mut required = vec!["pr", "summary"];
    match subject {
        Subject::Comment {
            thread: Some(_), ..
        } => {
            properties["resolve_thread"] = json!({
                "type": "boolean",
                "description": "Resolve the review thread.",
            });
        }
        Subject::CheckFailure { .. } => {
            properties["rerun"] = json!({
                "type": "boolean",
                "description": "Rerun the failed GitHub Actions jobs.",
            });
            properties["excerpts"] = json!({
                "type": "array",
                "description": "For each check the proposal answers, the log lines that show \
            why it failed: the error itself, at most 10, no setup or timestamps.",
                "items": {
                    "type": "object",
                    "properties": {
                        "check": { "type": "string", "description": "The check's node ID." },
                        "lines": { "type": "string" },
                    },
                    "required": ["check", "lines"],
                    "additionalProperties": false,
                },
            });
            required.push("excerpts");
        }
        _ => {}
    }
    json!({
        "name": "propose",
        "description": "Records a proposal for the user to approve or dismiss in gem. It is \
    checked first: an error says what is wrong, to fix and propose again. Approving runs its steps \
    in this order: `follow_up`, `patch`, `reply`, `resolve_thread`, `rerun`, \
    `close_pull_request`.",
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;
    use crate::proposals::FetchedComment;
    use crate::proto::{CheckFailure, Comment, FailedCheck, Proposal};
    use crate::tokens::tests::{temp_dir, token};

    const PR: &str = "https://github.com/o/r/pull/1";
    const OTHER: &str = "https://github.com/o/r/pull/2";

    fn comment(id: &str) -> Comment {
        Comment {
            id: id.into(),
            author: "reviewer".into(),
            body: format!("{id}'s body"),
            url: format!("{PR}#{id}"),
            body_html: String::new(),
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

        async fn render(&self, _: &str, markdown: &str) -> Result<String, String> {
            Ok(format!("<p>{markdown}</p>"))
        }
    }

    struct Server {
        state: PathBuf,
        app: Router,
    }

    impl Server {
        fn new(name: &str) -> Self {
            let state = temp_dir(name);
            Self {
                app: router(state.clone(), Fake),
                state,
            }
        }

        // A token for an agent answering `subject`.
        async fn token(&self, subject: &Subject) -> String {
            let tokens = Tokens::new(self.state.join(tokens::DIR));
            token(
                &tokens
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

        // The result of calling `tool`.
        async fn call(&self, token: &str, tool: &str, arguments: Value) -> Value {
            let (status, response) = self
                .post(
                    Some(token),
                    json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "tools/call",
                        "params": { "name": tool, "arguments": arguments },
                    }),
                )
                .await;
            assert_eq!(status, StatusCode::OK);
            response["result"].clone()
        }

        fn proposals(&self) -> Vec<Proposal> {
            let Ok(entries) = std::fs::read_dir(self.state.join(proposals::DIR)) else {
                return Vec::new();
            };
            (entries.flatten())
                .map(|entry| serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap())
                .collect()
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

        Tokens::new(server.state.join(tokens::DIR))
            .revoke("PRRC_1")
            .await;
        assert_eq!(
            server.post(Some(&token), initialize()).await.0,
            StatusCode::UNAUTHORIZED
        );
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
        let propose = async |subject: Subject| {
            let token = server.token(&subject).await;
            let (_, response) = server.post(Some(&token), list.clone()).await;
            let tools = response["result"]["tools"].as_array().unwrap().clone();
            let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
            assert_eq!(names, ["get_cluster", "propose"]);
            tools[1]["inputSchema"].clone()
        };

        let schema = propose(in_thread()).await;
        assert!(schema["properties"]["resolve_thread"].is_object());
        assert!(schema["properties"]["rerun"].is_null());

        let schema = propose(Subject::CheckFailure {
            pr: PR.into(),
            id: "abc,CR_1".into(),
        })
        .await;
        assert!(schema["properties"]["rerun"].is_object());
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("excerpts"))
        );
        assert!(schema["properties"]["resolve_thread"].is_null());
    }

    #[tokio::test]
    async fn a_valid_proposal_is_saved_with_its_subject() {
        let server = Server::new("valid");
        let token = server.token(&in_thread()).await;
        let result = server
            .call(
                &token,
                "propose",
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
        assert_eq!(text(&result), format!("Recorded proposal {}.", proposal.id));
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
    async fn an_investigation_proposes_on_any_of_its_clusters_pull_requests() {
        let server = Server::new("investigation");
        let subject = Subject::Investigation {
            id: "inv".into(),
            prs: vec![PR.into(), OTHER.into()],
        };
        let token = server.token(&subject).await;
        let result = server
            .call(
                &token,
                "propose",
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
    async fn a_check_failure_proposal_keeps_the_checks_it_excerpts() {
        let server = Server::new("checks");
        let subject = Subject::CheckFailure {
            pr: PR.into(),
            id: "abc,CR_1".into(),
        };
        let token = server.token(&subject).await;
        let result = server
            .call(
                &token,
                "propose",
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
            let result = server.call(&token, "propose", arguments.clone()).await;
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
            .call(
                &token,
                "propose",
                json!({ "pr": OTHER, "summary": "Reply", "reply": "Elsewhere." }),
            )
            .await;
        assert_eq!(result["isError"], true);
        assert!(text(&result).contains(PR), "{result}");
        assert!(server.proposals().is_empty());

        // Only a check failure's agent can rerun, so a comment's cannot reach one.
        let result = server
            .call(
                &token,
                "propose",
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
    async fn get_cluster_reads_the_snapshot() {
        let server = Server::new("cluster");
        let snapshot = json!({ "clusters": [{
            "name": "Renames",
            "pullRequests": [{ "url": PR, "title": "Rename", "titleHtml": "<p>Rename</p>" }],
        }] });
        std::fs::write(server.state.join(SNAPSHOT), snapshot.to_string()).unwrap();
        let token = server.token(&in_thread()).await;
        let result = server
            .call(&token, "get_cluster", json!({ "pull_request_url": PR }))
            .await;
        let cluster: Value = serde_json::from_str(text(&result)).unwrap();
        assert_eq!(cluster["name"], "Renames");
        assert_eq!(cluster["pullRequests"][0]["titleHtml"], Value::Null);
    }
}
