// Building origin/main in a worktree of the checkout's own, deploy/, so the server runs main
// whatever branch the checkout is on or however far it has diverged. Only the launcher moves it.

use std::{
    env,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::log;

const WORKTREE: &str = "deploy";

#[derive(Default)]
pub struct Updates {
    // The origin/main commit that last failed to check out or build, so it is not retried every
    // minute.
    failed: Option<String>,
}

impl Updates {
    // Builds origin/main when it has moved. Returns the build to restart onto.
    pub fn update(&mut self) -> Option<PathBuf> {
        if !self.check_out_latest()? {
            return None;
        }
        self.build()
    }

    // Moves the worktree to origin/main, adding it the first time. Returns whether it moved.
    pub fn check_out_latest(&mut self) -> Option<bool> {
        git(&["fetch", "--quiet", "origin", "main"])?;
        let latest = git(&["rev-parse", "origin/main"])?;
        if !Path::new(WORKTREE).exists() {
            log(&format!("checking out {} in {WORKTREE}/", &latest[..7]));
            git(&["worktree", "add", "--quiet", "--detach", WORKTREE, &latest])?;
            return Some(true);
        }
        let head = git(&["-C", WORKTREE, "rev-parse", "HEAD"])?;
        if head == latest || self.failed.as_ref() == Some(&latest) {
            return Some(false);
        }
        log(&format!("updating {} to {}", &head[..7], &latest[..7]));
        // Forced, so an edit made in the worktree by hand cannot hold it back.
        if git(&["-C", WORKTREE, "checkout", "--quiet", "--force", "--detach", &latest]).is_none() {
            log("cannot check out origin/main; staying on the running build");
            self.failed = Some(latest);
            return Some(false);
        }
        // `git diff --quiet` fails when there are differences.
        let launcher = "server/crates/launcher";
        if git(&["-C", WORKTREE, "diff", "--quiet", &head, &latest, "--", launcher]).is_none() {
            log("this changes the launcher, which runs its old build until it is restarted");
        }
        Some(true)
    }

    // Builds the worktree and returns the path cargo built the binary at. It shares the
    // checkout's target directory, so a build compiles only what changed.
    pub fn build(&mut self) -> Option<PathBuf> {
        log(&format!(
            "building {}",
            git(&["-C", WORKTREE, "rev-parse", "--short", "HEAD"])?
        ));
        let target = env::current_dir().ok()?.join("target");
        // Diagnostics still render to stderr; stdout carries the messages naming the binary.
        let mut cargo = Command::new("cargo")
            .args([
                "build",
                "--release",
                "--message-format=json-render-diagnostics",
                "--target-dir",
            ])
            .arg(target)
            .current_dir(Path::new(WORKTREE).join("server"))
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        let mut executable = None;
        for line in BufReader::new(cargo.stdout.take().unwrap()).lines() {
            let Ok(message) = serde_json::from_str::<serde_json::Value>(&line.unwrap_or_default())
            else {
                continue;
            };
            if let Some(path) = message["executable"].as_str() {
                executable = Some(PathBuf::from(path));
            }
        }
        if !cargo.wait().is_ok_and(|status| status.success()) || executable.is_none() {
            log("build failed; staying on the running build");
            self.failed = git(&["rev-parse", "origin/main"]);
            return None;
        }
        executable
    }
}

pub fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}
