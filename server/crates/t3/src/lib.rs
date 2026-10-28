use std::{

    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};

use gem_json::Value;

// Threads start in T3 Code's scratch space: the server's working directory is in no T3 Code
// project. The prompt must tell the agent to settle its own thread: T3 Code does not settle
// threads when they finish.
pub fn ask(title: &str, prompt: &str) -> Option<String> {
    let scratch = scratch().ok()?;
    let mut args = vec![
        "thread", "start", &scratch, "--title", title, "--wait", "--json",
    ];
    let agent = agent();
    args.extend(agent.iter().map(String::as_str));
    args.push("-");
    let output = t3(&args, prompt)?;
    let run = gem_json::parse_bytes(&output).ok()?;
    let thread_id: String = run.field("threadId").ok()?;
    let status: String = run.field("status").ok()?;
    let reply: Option<String> = run.field("reply").ok()?;

    let shown = t3(&["thread", "show", "--json", &thread_id], "")
        .and_then(|output| gem_json::parse_bytes(&output).ok());
    if !shown.is_some_and(|shown| shown["thread"]["settled"] == true) {
        eprintln!("thread {thread_id} ({title}) did not settle");
    }

    let reply = reply?.trim().to_string();
    (status == "completed" && !reply.is_empty()).then_some(reply)
}

// Starts an agent on `prompt` without waiting for it, in T3 Code's scratch space for threads
// outside any project. Returns the thread's id. `mcp` adds MCP servers to the thread's sessions,
// as Claude Code's `--mcp-config` takes them: JSON, or the path of a file holding it.
pub fn start(title: &str, prompt: &str, mcp: Option<&str>) -> Result<String, String> {
    start_in(&scratch()?, title, prompt, mcp)
}

// The provider instance agents run on, from GEM_T3_PROVIDER, such as `claudeCloud`, with the model
// T3 Code picks for it. Without it, agents run on the project's default.
fn agent() -> Vec<String> {
    match gem_env::var("GEM_T3_PROVIDER") {
        Some(provider) if !provider.trim().is_empty() => vec!["--provider".into(), provider],
        _ => Vec::new(),
    }
}

// The T3 Code app agents run in, by its home: GEM_T3_HOME, such as ~/.t3-cloud for a variant
// build running beside the regular app, or ~/.t3.
fn home() -> Result<PathBuf, String> {
    match gem_env::var("GEM_T3_HOME") {
        Some(home) if !home.trim().is_empty() => Ok(PathBuf::from(home)),
        _ => Ok(PathBuf::from(std::env::var("HOME").map_err(|e| e.to_string())?).join(".t3")),
    }
}

// T3 Code's scratch space for threads outside any project.
fn scratch() -> Result<String, String> {
    Ok(home()?.join("scratch").to_string_lossy().into_owned())
}

// Starts an agent on `prompt` without waiting for it, in the T3 Code project containing
// `project`. Returns the thread's id. `mcp` is as for `start`.
pub fn start_in(
    project: &str,
    title: &str,
    prompt: &str,
    mcp: Option<&str>,
) -> Result<String, String> {
    let agent = agent();
    let args = start_args(project, title, &agent, mcp);
    let output = t3(&args, prompt).ok_or("t3: could not run the CLI")?;
    // The thread exists once the JSON is printed, so a line printed around it, e.g. a warning,
    // must not lose it: a caller that thought it failed would start another.
    let output = String::from_utf8_lossy(&output);
    (output.lines().rev())
        .find_map(|line| gem_json::parse(line.trim()).ok()?.field::<String>("threadId").ok())
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
pub fn show(thread_id: &str) -> Option<(bool, String)> {
    let output = t3(
        &["thread", "show", "--json", "--messages", "20", thread_id],
        "",
    )?;
    let shown = gem_json::parse_bytes(&output).ok()?;
    let thread = &shown["thread"];
    let status: String = thread.field("status").ok()?;
    let active_run_id: Option<String> = thread.field("activeRunId").ok()?;
    // Each message's role and text.
    let messages: Vec<(String, String)> = (shown["messages"].as_array()?.iter())
        .map(|m| Some((m.field("role").ok()?, m.field("text").ok()?)))
        .collect::<Option<_>>()?;
    let stopped = matches!(status.as_str(), "failed" | "cancelled" | "interrupted");
    let unanswered = messages.last().is_some_and(|(role, _)| role == "user");
    let working = active_run_id.is_some() || (unanswered && !stopped);
    let reply = (messages.into_iter().rev())
        .find(|(role, _)| role == "assistant")
        .map(|(_, text)| text)
        .unwrap_or_default();
    Some((working, reply))
}

// Returns the thread's latest reply once its agent stops working: it finished, stopped, or waits
// on the user.
pub fn wait(thread_id: &str) -> String {
    loop {
        std::thread::sleep(Duration::from_secs(10));
        if let Some((false, reply)) = show(thread_id) {
            return reply;
        }
    }
}

// Whether the thread is done, settled or stopped, and whether its agent waits on the user to
// answer a question.
pub fn progress(thread_id: &str) -> Option<(bool, bool)> {
    let output = t3(
        &["thread", "show", "--json", "--messages", "0", thread_id],
        "",
    )?;
    let shown: Value = gem_json::parse_bytes(&output).ok()?;
    let thread = &shown["thread"];
    let status: String = thread.field("status").ok()?;
    let settled: bool = thread.field("settled").ok()?;
    let pending_request_count: u32 = thread.field("pendingRequestCount").ok()?;
    let stopped = matches!(status.as_str(), "failed" | "cancelled" | "interrupted");
    Some((settled || stopped, pending_request_count > 0))
}

// A busy thread gets the message as its next turn.
pub fn send(thread_id: &str, message: &str) -> Result<(), String> {
    t3(&["thread", "send", "--json", thread_id, "-"], message)
        .map(drop)
        .ok_or("t3: could not run the CLI".into())
}

// T3 Code installs its CLI here and keeps it pointing at the running app; on Windows it is a
// batch file. Batch files cannot take arguments with newlines, so `stdin` carries any message.
fn t3(args: &[&str], stdin: &str) -> Option<Vec<u8>> {
    let name = if cfg!(windows) { "t3.cmd" } else { "t3" };
    let cli = home().ok()?.join("bin").join(name);
    let mut child = Command::new(cli)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    // Written from another thread, so a child that prints a lot before it has read all of its
    // input never blocks on a full stdout pipe while this blocks on its full stdin.
    let mut input = child.stdin.take()?;
    let stdin = stdin.to_string();
    let writer = std::thread::spawn(move || input.write_all(stdin.as_bytes()));
    let output = child.wait_with_output().ok()?;
    writer.join().ok()?.ok()?;
    Some(output.stdout)
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
