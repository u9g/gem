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
        &["thread", "start", ".", "--title", title, "--wait", "--json", "-"],
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

// T3 Code installs its CLI here and keeps it pointing at the running app; on Windows it is a
// batch file. Batch files cannot take arguments with newlines, so `stdin` carries any message.
async fn t3(args: &[&str], stdin: &str) -> Option<Vec<u8>> {
    let name = if cfg!(windows) { "t3.cmd" } else { "t3" };
    let cli = PathBuf::from(env::var("HOME").ok()?).join(".t3/bin").join(name);
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
