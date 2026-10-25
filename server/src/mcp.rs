// gem's MCP server, so agents can read the clusters gem shows: `server mcp --state <dir>` speaks
// MCP over stdio and answers from the snapshot the running server keeps in <dir>. The server
// registers it with Claude Code, as the user-scope server `gem`, whenever it starts.

use std::{
    env,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};

use crate::proto::{ListOpenPullRequestsResponse, PullRequestCluster};

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
        let response = match handle(&state, &request) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            }
        };
        let _ = writeln!(stdout, "{response}");
        let _ = stdout.flush();
    }
}

fn handle(state: &Path, request: &Value) -> Result<Value, (i64, String)> {
    let params = &request["params"];
    match request["method"].as_str().unwrap_or_default() {
        "initialize" => Ok(json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": NAME, "version": env!("CARGO_PKG_VERSION") },
            "instructions": "gem groups the user's open pull requests into clusters of PRs \
        linked by stacking or mentions. These tools read the clusters as gem last showed them.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let text = call(
                state,
                params["name"].as_str().unwrap_or_default(),
                &params["arguments"],
            );
            Ok(match text {
                Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                Err(text) => {
                    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
                }
            })
        }
        method => Err((-32601, format!("no method {method}"))),
    }
}

fn tools() -> Value {
    json!([
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
        },
        {
            "name": "list_clusters",
            "description": "Every cluster gem shows, most recently updated first: its name and \
    its pull requests' URLs and titles.",
            "inputSchema": { "type": "object", "properties": {} },
        },
    ])
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
