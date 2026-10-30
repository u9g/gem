use std::{env, path::PathBuf, process::Stdio, time::Duration};

use serde::Deserialize;
use tokio::{io::AsyncWriteExt, process::Command};

#[derive(Deserialize)]
struct Run {
    #[serde(rename = "threadId")]
    thread_id: String,
    status: String,
    reply: Option<String>,
}

#[derive(Deserialize)]
struct Shown {
    thread: ShownThread,
}

#[derive(Deserialize)]
struct ShownThread {
    settled: bool,
}

// Threads start in T3 Code's scratch space: the server's working directory is in no T3 Code
// project. The prompt must tell the agent to settle its own thread: T3 Code does not settle
// threads when they finish.
pub async fn ask(title: &str, prompt: &str) -> Option<String> {
    let scratch = scratch().ok()?;
    let mut args = vec![
        "thread", "start", &scratch, "--title", title, "--wait", "--json",
    ];
    let agent = agent();
    args.extend(agent.iter().map(String::as_str));
    args.push("-");
    let output = t3(&args, prompt).await?;
    let run: Run = serde_json::from_slice(&output).ok()?;

    let shown: Option<Shown> = t3(&["thread", "show", "--json", &run.thread_id], "")
        .await
        .and_then(|output| serde_json::from_slice(&output).ok());
    if !shown.is_some_and(|shown| shown.thread.settled) {
        eprintln!("thread {} ({title}) did not settle", run.thread_id);
    }

    let reply = run.reply?.trim().to_string();
    (run.status == "completed" && !reply.is_empty()).then_some(reply)
}

// Starts an agent on `prompt` without waiting for it, in T3 Code's scratch space for threads
// outside any project. Returns the thread's id. `mcp` adds MCP servers to the thread's sessions,
// as Claude Code's `--mcp-config` takes them: JSON, or the path of a file holding it.
pub async fn start(title: &str, prompt: &str, mcp: Option<&str>) -> Result<String, String> {
    start_in(&scratch()?, title, prompt, mcp).await
}

// The provider instance agents run on, from GEM_T3_PROVIDER, such as `claudeCloud`, with the model
// T3 Code picks for it. Without it, agents run on the project's default.
fn agent() -> Vec<String> {
    match env::var("GEM_T3_PROVIDER") {
        Ok(provider) if !provider.trim().is_empty() => vec!["--provider".into(), provider],
        _ => Vec::new(),
    }
}

// The T3 Code app agents run in, by its home: GEM_T3_HOME, such as ~/.t3-cloud for a variant
// build running beside the regular app, or ~/.t3.
fn home() -> Result<PathBuf, String> {
    match env::var("GEM_T3_HOME") {
        Ok(home) if !home.trim().is_empty() => Ok(PathBuf::from(home)),
        _ => Ok(PathBuf::from(env::var("HOME").map_err(|e| e.to_string())?).join(".t3")),
    }
}

// T3 Code's scratch space for threads outside any project.
fn scratch() -> Result<String, String> {
    Ok(home()?.join("scratch").to_string_lossy().into_owned())
}

// Starts an agent on `prompt` without waiting for it, in the T3 Code project containing
// `project`. Returns the thread's id. `mcp` is as for `start`.
pub async fn start_in(
    project: &str,
    title: &str,
    prompt: &str,
    mcp: Option<&str>,
) -> Result<String, String> {
    let agent = agent();
    let args = start_args(project, title, &agent, mcp);
    let output = t3(&args, prompt).await.ok_or("t3: could not run the CLI")?;
    #[derive(Deserialize)]
    struct Started {
        #[serde(rename = "threadId")]
        thread_id: String,
    }
    // The thread exists once the JSON is printed, so a line printed around it, e.g. a warning,
    // must not lose it: a caller that thought it failed would start another.
    let output = String::from_utf8_lossy(&output);
    (output.lines().rev())
        .find_map(|line| serde_json::from_str::<Started>(line.trim()).ok())
        .map(|started| started.thread_id)
        .ok_or_else(|| format!("t3: {}", output.trim()))
}

// T3 Code reads the prompt from stdin, after `-`.
fn start_args<'a>(
    project: &'a str,
    title: &'a str,
    agent: &'a [String],
    mcp: Option<&'a str>,
) -> Vec<&'a str> {
    let mut args = vec!["thread", "start", project, "--title", title, "--json"];
    args.extend(agent.iter().map(String::as_str));
    if let Some(mcp) = mcp {
        args.extend(["--mcp-config", mcp]);
    }
    args.push("-");
    args
}

// Whether the thread's agent is running, or about to run on the latest message, and its latest
// reply, empty before its first.
pub async fn show(thread_id: &str) -> Option<(bool, String)> {
    #[derive(Deserialize)]
    struct Shown {
        thread: Thread,
        messages: Vec<Message>,
    }
    #[derive(Deserialize)]
    struct Thread {
        status: String,
        #[serde(rename = "activeRunId")]
        active_run_id: Option<String>,
    }
    #[derive(Deserialize)]
    struct Message {
        role: String,
        text: String,
    }
    let output = t3(
        &["thread", "show", "--json", "--messages", "20", thread_id],
        "",
    )
    .await?;
    let shown: Shown = serde_json::from_slice(&output).ok()?;
    let stopped = matches!(
        shown.thread.status.as_str(),
        "failed" | "cancelled" | "interrupted"
    );
    let unanswered = shown.messages.last().is_some_and(|m| m.role == "user");
    let working = shown.thread.active_run_id.is_some() || (unanswered && !stopped);
    let reply = (shown.messages.into_iter().rev())
        .find(|message| message.role == "assistant")
        .map(|message| message.text)
        .unwrap_or_default();
    Some((working, reply))
}

// Returns the thread's latest reply once its agent stops working: it finished, stopped, or waits
// on the user. Returns nothing once the thread cannot be read 3 times in a row, such as when it
// was deleted.
pub async fn wait(thread_id: &str) -> String {
    let mut unread = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let Some((working, reply)) = show(thread_id).await else {
            unread += 1;
            if unread == 3 {
                return String::new();
            }
            continue;
        };
        unread = 0;
        if !working {
            return reply;
        }
    }
}

// Whether the thread is done, settled or stopped, and whether its agent waits on the user to
// answer a question.
pub async fn progress(thread_id: &str) -> Option<(bool, bool)> {
    #[derive(Deserialize)]
    struct Shown {
        thread: Thread,
    }
    #[derive(Deserialize)]
    struct Thread {
        status: String,
        settled: bool,
        #[serde(rename = "pendingRequestCount")]
        pending_request_count: u32,
    }
    let output = t3(
        &["thread", "show", "--json", "--messages", "0", thread_id],
        "",
    )
    .await?;
    let thread = serde_json::from_slice::<Shown>(&output).ok()?.thread;
    let stopped = matches!(
        thread.status.as_str(),
        "failed" | "cancelled" | "interrupted"
    );
    Some((thread.settled || stopped, thread.pending_request_count > 0))
}

// A busy thread gets the message as its next turn. Fails when the thread is gone.
pub async fn send(thread_id: &str, message: &str) -> Result<(), String> {
    let output = run(&["thread", "send", "--json", thread_id, "-"], message)
        .await
        .ok_or("t3: could not run the CLI")?;
    match output.status.success() {
        true => Ok(()),
        false => Err(format!("t3: thread send {}", output.status)),
    }
}

// The CLI's stdout, whether it succeeded or not.
async fn t3(args: &[&str], stdin: &str) -> Option<Vec<u8>> {
    Some(run(args, stdin).await?.stdout)
}

// T3 Code installs its CLI here and keeps it pointing at the running app; on Windows it is a
// batch file. Batch files cannot take arguments with newlines, so `stdin` carries any message.
async fn run(args: &[&str], stdin: &str) -> Option<std::process::Output> {
    let name = if cfg!(windows) { "t3.cmd" } else { "t3" };
    let cli = home().ok()?.join("bin").join(name);
    let mut child = Command::new(cli)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let mut input = child.stdin.take()?;
    input.write_all(stdin.as_bytes()).await.ok()?;
    drop(input);
    child.wait_with_output().await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_passes_the_mcp_config_before_the_prompt() {
        let agent = vec!["--provider".to_string(), "claudeCloud".to_string()];
        let args = start_args("C:/p", "Fix", &agent, Some("C:/state/agent.mcp.json"));
        assert_eq!(
            args,
            [
                "thread",
                "start",
                "C:/p",
                "--title",
                "Fix",
                "--json",
                "--provider",
                "claudeCloud",
                "--mcp-config",
                "C:/state/agent.mcp.json",
                "-",
            ]
        );
    }

    #[test]
    fn start_without_an_mcp_config_passes_none() {
        let args = start_args("C:/p", "Rebase", &[], None);
        assert!(!args.contains(&"--mcp-config"));
        assert_eq!(args.last(), Some(&"-"));
    }
}
