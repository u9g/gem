use std::{env, path::PathBuf, process::Stdio};

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

// Threads start in the T3 Code project containing the working directory. The prompt must tell
// the agent to settle its own thread: T3 Code does not settle threads when they finish.
pub async fn ask(title: &str, prompt: &str) -> Option<String> {
    let output = t3(
        &[
            "thread", "start", ".", "--title", title, "--wait", "--json", "-",
        ],
        prompt,
    )
    .await?;
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
// outside any project. Returns the thread's id.
pub async fn start(title: &str, prompt: &str) -> Result<String, String> {
    let scratch = PathBuf::from(env::var("HOME").map_err(|e| e.to_string())?).join(".t3/scratch");
    let scratch = scratch.to_string_lossy();
    let output = t3(
        &["thread", "start", &scratch, "--title", title, "--json", "-"],
        prompt,
    )
    .await
    .ok_or("t3: could not run the CLI")?;
    #[derive(Deserialize)]
    struct Started {
        #[serde(rename = "threadId")]
        thread_id: String,
    }
    serde_json::from_slice::<Started>(&output)
        .map(|started| started.thread_id)
        .map_err(|_| format!("t3: {}", String::from_utf8_lossy(&output).trim()))
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

// Opens the thread in T3 Code's hosted app, which reaches this machine's T3 Code from a phone too.
pub fn url(thread_id: &str) -> Option<String> {
    let environment = std::fs::read_to_string(
        PathBuf::from(env::var("HOME").ok()?).join(".t3/userdata/environment-id"),
    )
    .ok()?;
    Some(format!(
        "https://app.t3.codes/{}/{thread_id}",
        environment.trim()
    ))
}

// A busy thread gets the message as its next turn.
pub async fn send(thread_id: &str, message: &str) -> Result<(), String> {
    t3(&["thread", "send", "--json", thread_id, "-"], message)
        .await
        .map(drop)
        .ok_or("t3: could not run the CLI".into())
}

// T3 Code installs its CLI here and keeps it pointing at the running app; on Windows it is a
// batch file. Batch files cannot take arguments with newlines, so `stdin` carries any message.
async fn t3(args: &[&str], stdin: &str) -> Option<Vec<u8>> {
    let name = if cfg!(windows) { "t3.cmd" } else { "t3" };
    let cli = PathBuf::from(env::var("HOME").ok()?)
        .join(".t3/bin")
        .join(name);
    let mut child = Command::new(cli)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let mut input = child.stdin.take()?;
    input.write_all(stdin.as_bytes()).await.ok()?;
    drop(input);
    Some(child.wait_with_output().await.ok()?.stdout)
}
