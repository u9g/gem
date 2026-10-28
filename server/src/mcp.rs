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
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use gem_db::{Db, OptionalExtension, params};
use gem_http::server::{Request, Response};
use gem_json::{FromJson, Value, json};
use gem_log::{error, info, warn};

use crate::investigations;
use crate::proposals::{self, GitHub};
use crate::tokens::{Subject, Tokens};
use gem_proto::{ListOpenPullRequestsResponse, PullRequestCluster};

const NAME: &str = "gem";

// Called after every refresh.
pub fn save(response: &ListOpenPullRequestsResponse) {
    save_to(crate::db(), response);
}

fn save_to(db: &Db, response: &ListOpenPullRequestsResponse) {
    let saved = db.with(|db| {
        db.execute(
            "INSERT OR REPLACE INTO snapshot (id, response) VALUES (0, ?1)",
            params![gem_json::to_string(response)],
        )
    });
    if let Err(error) = saved {
        warn!("cannot save the snapshot", error = error);
    }
}

// The snapshot's JSON, once the server saved one.
fn snapshot(db: &Db) -> Option<String> {
    db.with(|db| {
        db.query_row("SELECT response FROM snapshot", params![], |row| {
            row.get::<String>(0)
        })
        .optional()
    })
    .ok()
    .flatten()
}

// Points Claude Code's `gem` MCP server at this build and this state directory, unless it
// already does. Builds are replaced on every update, so each one registers itself.
pub fn register() {
    let exe = env::current_exe().unwrap().to_string_lossy().into_owned();
    let state = env::current_dir().unwrap().to_string_lossy().into_owned();
    let wanted = json!({
        "type": "stdio",
        "command": exe,
        "args": ["mcp", "--state", state],
        "env": {},
    });
    let registered = env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .and_then(|home| std::fs::read(PathBuf::from(home).join(".claude.json")).ok())
        .and_then(|json| gem_json::parse_bytes(&json).ok())
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
        let Ok(request) = gem_json::parse(&line) else {
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
        mentions. These tools read the clusters as gem last showed them.",
        )),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": (tools()) })),
        "tools/call" => Ok(tool_result(call(
            db,
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
        "protocolVersion": (params["protocolVersion"].as_str().unwrap_or("2025-06-18")),
        "capabilities": { "tools": {} },
        "serverInfo": { "name": name, "version": (env!("CARGO_PKG_VERSION")) },
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
        (get_cluster()),
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

fn call(db: &Db, tool: &str, arguments: &Value) -> Result<String, String> {
    let snapshot = snapshot(db)
        .ok_or("gem has not listed any clusters yet; is its server running?".to_string())?;
    let mut snapshot = gem_json::parse(&snapshot).map_err(|e| e.to_string())?;
    // Rendered HTML repeats the markdown next to it.
    strip_view(&mut snapshot);
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
            Ok(cluster.to_string_pretty())
        }
        "list_clusters" => {
            let clusters: Vec<Value> = (clusters.iter())
                .map(|cluster| {
                    let prs = cluster["pullRequests"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    let prs: Vec<Value> = (prs.iter())
                        .map(|pr| json!({ "url": (pr["url"]), "title": (pr["title"]) }))
                        .collect();
                    json!({ "name": (cluster["name"]), "pullRequests": prs })
                })
                .collect();
            Ok(gem_json::to_string_pretty(&clusters))
        }
        tool => Err(format!("no tool {tool}")),
    }
}

// The URLs of the pull requests in the snapshot's cluster holding `pr`, or none when it holds none.
fn cluster_of(db: &Db, pr: &str) -> Vec<String> {
    let pr = pr.trim().trim_end_matches('/');
    let snapshot: ListOpenPullRequestsResponse = snapshot(db)
        .and_then(|json| gem_json::from_str(&json).ok())
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

// A cluster as `get_cluster` returns it, for prompts of agents that may not reach this server.
pub fn cluster_json(cluster: &PullRequestCluster) -> String {
    let mut cluster = gem_json::ToJson::to_json(cluster);
    strip_view(&mut cluster);
    cluster.to_string_pretty()
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
// A thread keeps the URL it was started with across server restarts, so the port stays the same.
// GEM_MCP_PORT overrides it.
const DEFAULT_PORT: u16 = 47474;

fn port() -> u16 {
    gem_env::var("GEM_MCP_PORT")
        .and_then(|port| port.trim().parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

fn url() -> String {
    format!("http://127.0.0.1:{}/mcp", port())
}

// The --mcp-config of a new agent answering `subject`, as the path of a file in the server's
// working directory: T3 Code's CLI is a batch file on Windows, which cannot take JSON as an
// argument.
pub fn agent_config(subject: &Subject) -> Result<String, String> {
    let path = Tokens::here().issue(subject, &url())?;
    Ok(path.to_string_lossy().into_owned())
}

// Serves `gem-agent` on the loopback interface until the server stops. A server restarting for an
// update can start before the one it replaces has let go of the port, so it keeps trying.
pub fn listen() {
    let endpoint = Arc::new(Endpoint {
        db: crate::db().clone(),
        tokens: Tokens::here(),
        github: proposals::Live,
    });
    let port = port();
    let listener = loop {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            Ok(listener) => break listener,
            Err(error) => {
                warn!(
                    "cannot listen for agents; trying again",
                    port = port,
                    error = error
                );
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    };
    info!("listening for agents", port = port);
    if let Err(error) = gem_http::server::serve(listener, move |request| answer(&endpoint, request))
    {
        error!("stopped listening for agents", error = error);
    }
}

struct Endpoint<G> {
    // With the snapshot, proposals and investigations.
    db: Db,
    tokens: Tokens,
    github: G,
}

// Streamable HTTP without streams: each POST to /mcp is one JSON-RPC message, answered with JSON.
fn answer<G: GitHub>(endpoint: &Endpoint<G>, request: Request) -> Response {
    let path = request.path.split('?').next().unwrap_or_default();
    if path != "/mcp" {
        return Response::new(404);
    }
    if request.method != "POST" {
        return Response::new(405).header("allow", "POST");
    }
    let token = (request.header("authorization"))
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    let Some(subject) = endpoint.tokens.subject(token.trim()) else {
        return Response::new(401)
            .header("www-authenticate", "Bearer")
            .header("content-type", "text/plain; charset=utf-8")
            .body(b"no agent of gem's has this token".to_vec());
    };
    let Ok(body) = gem_json::parse_bytes(&request.body) else {
        let error = response(Value::Null, Err((-32700, "the body is not JSON".into())));
        return Response::json(400, &error);
    };
    // Notifications have no id and get no response.
    let Some(id) = body.get("id").cloned() else {
        return Response::new(202);
    };
    Response::json(200, &response(id, handle_agent(endpoint, &subject, &body)))
}

fn handle_agent<G: GitHub>(
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
        or dismiss. `propose` records a proposal about what this thread answers, \
        `get_cluster` reads a cluster as gem shows it now, and `search_investigations` finds what \
        earlier agents found investigating failures.",
        )),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": [(get_cluster()), (search_investigations()), (propose_tool(subject))],
        })),
        "tools/call" => {
            let arguments = &params["arguments"];
            Ok(tool_result(
                match params["name"].as_str().unwrap_or_default() {
                    "get_cluster" => call(&endpoint.db, "get_cluster", arguments),
                    "search_investigations" => match arguments["query"].as_str() {
                        Some(query) if !query.trim().is_empty() => {
                            Ok(investigations::search(&endpoint.db, query))
                        }
                        _ => Err("`query` must be words to search for".into()),
                    },
                    "propose" => propose(endpoint, subject, arguments),
                    tool => Err(format!("no tool {tool}")),
                },
            ))
        }
        method => Err((-32601, format!("no method {method}"))),
    }
}

fn propose<G: GitHub>(
    endpoint: &Endpoint<G>,
    subject: &Subject,
    arguments: &Value,
) -> Result<String, String> {
    let request = proposals::Request::from_json(arguments).map_err(|e| e.to_string())?;
    let cluster = cluster_of(&endpoint.db, &request.pr);
    let proposed = proposals::propose(&endpoint.github, &endpoint.db, subject, &cluster, request);
    match &proposed {
        Ok(id) => info!("proposal recorded", proposal = id, subject = subject.id()),
        Err(error) => info!("proposal rejected", subject = subject.id(), error = error),
    }
    proposed.map(|id| format!("Recorded proposal {id}."))
}

fn search_investigations() -> Value {
    json!({
        "name": "search_investigations",
        "description": "The investigations earlier agents finished into failed checks and other \
    problems on the user's pull requests, the newest 10 first: each one's title, pull requests, \
    when it started, the failed checks it started from with their log excerpts, and its last \
    reply, which says what it found. One matches when its title, failed checks or reply has every \
    word of `query`, ignoring case. `olderMatches` counts the older ones that match too: add words \
    to narrow the search.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "A few distinctive words, such as the failing test's name or \
    words of the error.",
                },
            },
            "required": ["query"],
        },
    })
}

// `propose`, with the steps its subject can take.
fn propose_tool(subject: &Subject) -> Value {
    let mut properties = json!({
        "pr": {
            "type": "string",
            "description": (format!(
                "The pull request the proposal is about: {}",
                subject.prs().join(" or ")
            )),
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
        "other_patches": {
            "type": "array",
            "description": "Changes to the other pull requests in `pr`'s cluster, as `get_cluster` \
    lists them, such as a fix in the pull request that causes the problem, or the same fix in \
    another repository's matching pull request. Each is `git diff` against that pull request's \
    head. Never commit or push them yourself.",
            "items": {
                "type": "object",
                "properties": {
                    "pr": { "type": "string", "description": "The pull request it changes." },
                    "patch": { "type": "string" },
                    "message": { "type": "string", "description": "The commit message." },
                },
                "required": ["pr", "patch", "message"],
                "additionalProperties": false,
            },
        },
        "open_pull_request": {
            "type": "object",
            "description": "A new pull request, such as a fix on the base branch for a test that \
    fails there too. Approving commits `patch` on `branch` off `base`, pushes it to `repository`, \
    or to the user's fork of it when they cannot push there, and opens the pull request. Never \
    commit, push or open it yourself.",
            "properties": {
                "repository": {
                    "type": "string",
                    "description": "`owner/name` it merges into, such as the repository `pr` merges \
    into, not a fork.",
                },
                "base": { "type": "string", "description": "The branch it merges into." },
                "branch": {
                    "type": "string",
                    "description": "A new branch to commit it on, which neither `repository` nor \
    the user's fork has.",
                },
                "title": { "type": "string" },
                "body": {
                    "type": "string",
                    "description": "Markdown description, posted as the user. Must link `pr`.",
                },
                "patch": {
                    "type": "string",
                    "description": "The change, as `git diff` prints it against `base`'s head.",
                },
                "message": { "type": "string", "description": "The commit message." },
            },
            "required": ["repository", "base", "branch", "title", "body", "patch", "message"],
            "additionalProperties": false,
        },
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
    in this order: `follow_up`, `patch`, `other_patches`, `open_pull_request`, `reply`, \
    `resolve_thread`, `rerun`, `close_pull_request`.",
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

    use super::*;
    use crate::proposals::FetchedComment;
    use crate::tokens::tests::{db, temp_dir, token};
    use gem_proto::{CheckFailure, Comment, FailedCheck, Proposal};

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
        fn comment(&self, id: &str) -> Result<FetchedComment, String> {
            Ok(FetchedComment {
                comment: comment(id),
                pull_request: PR.into(),
            })
        }

        fn check_failure(&self, _: &str, id: &str) -> Result<CheckFailure, String> {
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

        fn review_thread(&self, _: &str) -> Result<(String, Vec<Comment>), String> {
            Ok((PR.into(), vec![comment("PRRC_0"), comment("PRRC_1")]))
        }

        fn check_patch(&self, _: &str, _: &str) -> Result<String, String> {
            Ok("abc".into())
        }

        fn check_new_pull_request(
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

        fn render(&self, _: &str, markdown: &str) -> Result<String, String> {
            Ok(format!("<p>{markdown}</p>"))
        }
    }

    struct Server {
        db: Db,
        tokens: Tokens,
        endpoint: Endpoint<Fake>,
    }

    impl Server {
        fn new(name: &str) -> Self {
            let db = db();
            let tokens = Tokens::new(db.clone(), temp_dir(name));
            Self {
                endpoint: Endpoint {
                    db: db.clone(),
                    tokens: tokens.clone(),
                    github: Fake,
                },
                db,
                tokens,
            }
        }

        fn db(&self) -> &Db {
            &self.db
        }

        // A token for an agent answering `subject`.
        fn token(&self, subject: &Subject) -> String {
            token(
                &self
                    .tokens
                    .issue(subject, "http://127.0.0.1:1/mcp")
                    .unwrap(),
            )
        }

        fn post(&self, token: Option<&str>, body: Value) -> (u16, Value) {
            let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
            if let Some(token) = token {
                headers.push(("authorization".into(), format!("Bearer {token}")));
            }
            let request = Request {
                method: "POST".into(),
                path: "/mcp".into(),
                headers,
                body: body.to_string().into_bytes(),
            };
            let response = answer(&self.endpoint, request);
            let body = gem_json::parse_bytes(&response.body).unwrap_or(Value::Null);
            (response.status, body)
        }

        // The result of calling `tool`.
        fn call(&self, token: &str, tool: &str, arguments: Value) -> Value {
            let (status, response) = self.post(
                Some(token),
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": { "name": tool, "arguments": arguments },
                }),
            );
            assert_eq!(status, 200);
            response["result"].clone()
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

    #[test]
    fn requests_without_a_valid_token_are_unauthorized() {
        let server = Server::new("unauthorized");
        let token = server.token(&in_thread());
        assert_eq!(server.post(None, initialize()).0, 401);
        assert_eq!(server.post(Some("0123abcd"), initialize()).0, 401);
        let (status, response) = server.post(Some(&token), initialize());
        assert_eq!(status, 200);
        assert_eq!(response["result"]["serverInfo"]["name"], AGENT_NAME);

        server.tokens.revoke("PRRC_1");
        assert_eq!(server.post(Some(&token), initialize()).0, 401);
    }

    #[test]
    fn notifications_are_accepted_without_a_response() {
        let server = Server::new("notification");
        let token = server.token(&in_thread());
        let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(server.post(Some(&token), notification).0, 202);
    }

    #[test]
    fn propose_offers_the_steps_its_subject_can_take() {
        let server = Server::new("tools");
        let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        let propose = |subject: Subject| {
            let token = server.token(&subject);
            let (_, response) = server.post(Some(&token), list.clone());
            let tools = response["result"]["tools"].as_array().unwrap().clone();
            let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
            assert_eq!(names, ["get_cluster", "search_investigations", "propose"]);
            tools[2]["inputSchema"].clone()
        };

        let schema = propose(in_thread());
        assert!(schema["properties"]["resolve_thread"].is_object());
        assert!(schema["properties"]["rerun"].is_null());

        let schema = propose(Subject::CheckFailure {
            pr: PR.into(),
            id: "abc,CR_1".into(),
        });
        assert!(schema["properties"]["rerun"].is_object());
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("excerpts"))
        );
        assert!(schema["properties"]["resolve_thread"].is_null());
    }

    #[test]
    fn a_valid_proposal_is_saved_with_its_subject() {
        let server = Server::new("valid");
        let token = server.token(&in_thread());
        let result = server.call(
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
        );
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

    #[test]
    fn a_top_level_reply_opens_by_quoting_the_comment() {
        let server = Server::new("quote");
        let subject = Subject::Comment {
            pr: PR.into(),
            comment: "IC_1".into(),
            thread: None,
        };
        let token = server.token(&subject);
        let result = server.call(
            &token,
            "propose",
            json!({ "pr": PR, "summary": "Reply", "reply": "Done." }),
        );
        assert_eq!(result["isError"], Value::Null, "{result}");
        let [proposal] = &server.proposals()[..] else {
            panic!("{:?}", server.proposals());
        };
        assert_eq!(proposal.reply, "> IC_1's body\n\nDone.");
        assert_eq!(proposal.reply_html, "<p>> IC_1's body\n\nDone.</p>");
    }

    #[test]
    fn an_investigation_proposes_on_any_of_its_clusters_pull_requests() {
        let server = Server::new("investigation");
        let subject = Subject::Investigation {
            id: "inv".into(),
            prs: vec![PR.into(), OTHER.into()],
        };
        let token = server.token(&subject);
        let result = server.call(
            &token,
            "propose",
            json!({ "pr": OTHER, "summary": "Fix it", "follow_up": "Fix it." }),
        );
        assert_eq!(result["isError"], Value::Null, "{result}");
        let [proposal] = &server.proposals()[..] else {
            panic!("{:?}", server.proposals());
        };
        assert_eq!(proposal.investigation_id, "inv");
        assert_eq!(proposal.pull_request_url, OTHER);
        assert_eq!(proposal.comment, None);
    }

    #[test]
    fn a_proposal_opens_a_pull_request_that_links_its_own() {
        let server = Server::new("open-pull-request");
        let subject = Subject::Investigation {
            id: "inv".into(),
            prs: vec![PR.into()],
        };
        let token = server.token(&subject);
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
            let result = server.call(&token, "propose", open(&body));
            assert_eq!(result["isError"], true, "{result}");
            assert!(server.proposals().is_empty());
        }

        let body = format!("Fixes the test that fails on {PR}.");
        let result = server.call(&token, "propose", open(&body));
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

    #[test]
    fn a_check_failure_proposal_keeps_the_checks_it_excerpts() {
        let server = Server::new("checks");
        let subject = Subject::CheckFailure {
            pr: PR.into(),
            id: "abc,CR_1".into(),
        };
        let token = server.token(&subject);
        let result = server.call(
            &token,
            "propose",
            json!({
                "pr": PR,
                "summary": "Rerun it",
                "rerun": true,
                "excerpts": [{ "check": "CR_1", "lines": "error: timed out" }],
            }),
        );
        assert_eq!(result["isError"], Value::Null, "{result}");
        let [proposal] = &server.proposals()[..] else {
            panic!("{:?}", server.proposals());
        };
        let failure = proposal.check_failure.as_ref().unwrap();
        assert_eq!(failure.id, "abc,CR_1");
        assert_eq!(failure.checks[0].excerpt, "error: timed out");
    }

    #[test]
    fn an_invalid_proposal_is_a_tool_error_and_saves_nothing() {
        let server = Server::new("invalid");
        let token = server.token(&in_thread());
        let invalid = [
            // No steps.
            json!({ "pr": PR, "summary": "Do nothing" }),
            json!({ "pr": PR, "summary": "Fix it", "patch": "diff --git a/x b/x\n" }),
            json!({ "pr": PR, "summary": "Rerun it", "rerun": true }),
            json!({ "pr": PR, "summary": "Reply", "replyy": "Typo." }),
            json!({ "summary": "Reply", "reply": "No PR." }),
        ];
        for arguments in invalid {
            let result = server.call(&token, "propose", arguments.clone());
            assert_eq!(result["isError"], true, "{arguments} gave {result}");
            assert!(!text(&result).is_empty());
        }
        assert!(server.proposals().is_empty());
    }

    #[test]
    fn a_token_proposes_only_on_its_own_subject() {
        let server = Server::new("scope");
        let token = server.token(&in_thread());
        let result = server.call(
            &token,
            "propose",
            json!({ "pr": OTHER, "summary": "Reply", "reply": "Elsewhere." }),
        );
        assert_eq!(result["isError"], true);
        assert!(text(&result).contains(PR), "{result}");
        assert!(server.proposals().is_empty());

        // Only a check failure's agent can rerun, so a comment's cannot reach one.
        let result = server.call(
            &token,
            "propose",
            json!({
                "pr": PR,
                "summary": "Rerun",
                "rerun": true,
                "excerpts": [{ "check": "CR_1", "lines": "error" }],
            }),
        );
        assert_eq!(result["isError"], true);
        assert!(server.proposals().is_empty());
    }

    #[test]
    fn a_proposal_patches_other_pull_requests_of_its_cluster() {
        let server = Server::new("other-patches");
        let cluster = |urls: &[&str]| PullRequestCluster {
            pull_requests: (urls.iter())
                .map(|url| gem_proto::PullRequest {
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
        let token = server.token(&in_thread());
        let other_patch = |pr: &str| {
            json!({
                "pr": PR,
                "summary": "Port it",
                "reply": "Ported.",
                "other_patches": [{ "pr": pr, "patch": "diff --git a/x b/x\n", "message": "Port it" }],
            })
        };

        for pr in [PR, ELSEWHERE] {
            let result = server.call(&token, "propose", other_patch(pr));
            assert_eq!(result["isError"], true, "{pr} gave {result}");
        }
        let mut twice = other_patch(OTHER);
        let patch = twice["other_patches"][0].clone();
        twice["other_patches"].as_array_mut().unwrap().push(patch);
        let result = server.call(&token, "propose", twice);
        assert_eq!(result["isError"], true, "{result}");
        assert!(server.proposals().is_empty());

        let result = server.call(&token, "propose", other_patch(OTHER));
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

    #[test]
    fn get_cluster_reads_the_snapshot() {
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
                    params![snapshot.to_string()],
                )
            })
            .unwrap();
        let token = server.token(&in_thread());
        let result = server.call(&token, "get_cluster", json!({ "pull_request_url": PR }));
        let cluster = gem_json::parse(text(&result)).unwrap();
        assert_eq!(cluster["name"], "Renames");
        assert_eq!(cluster["pullRequests"][0]["titleHtml"], Value::Null);
    }

    #[test]
    fn search_investigations_reads_the_closed_investigations() {
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
                    params![investigation.to_string()],
                )
            })
            .unwrap();
        let token = server.token(&in_thread());
        let result = server.call(
            &token,
            "search_investigations",
            json!({ "query": "test_join" }),
        );
        let found = gem_json::parse(text(&result)).unwrap();
        assert_eq!(found["investigations"][0]["pullRequestUrls"], json!([PR]));

        let result = server.call(&token, "search_investigations", json!({}));
        assert_eq!(result["isError"], true);
    }
}
