use std::{env, path::PathBuf};

use serde::Deserialize;
use tokio::process::Command;

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
    let output = t3(&[
        "thread", "start", ".", "--title", title, "--wait", "--json", prompt,
    ])
    .await?;
    let run: Run = serde_json::from_slice(&output).ok()?;

    let shown: Option<Shown> = t3(&["thread", "show", "--json", &run.thread_id])
        .await
        .and_then(|output| serde_json::from_slice(&output).ok());
    if !shown.is_some_and(|shown| shown.thread.settled) {
        eprintln!("thread {} ({title}) did not settle", run.thread_id);
    }

    let reply = run.reply?.trim().to_string();
    (run.status == "completed" && !reply.is_empty()).then_some(reply)
}

// T3 Code installs its CLI here and keeps it pointing at the running app.
async fn t3(args: &[&str]) -> Option<Vec<u8>> {
    let cli = PathBuf::from(env::var("HOME").ok()?).join(".t3/bin/t3");
    Some(Command::new(cli).args(args).output().await.ok()?.stdout)
}
