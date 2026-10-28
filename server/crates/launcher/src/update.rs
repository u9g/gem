// Keeping the checkout on origin/main, and building it.

use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::{log, releases::Build};

#[derive(Default)]
pub struct Updates {
    // The origin/main commit that last failed to fast-forward or build, so it is not retried
    // every minute.
    failed: Option<String>,
}

impl Updates {
    // Builds origin/main when it has moved. Returns the build to restart onto.
    pub fn update(&mut self) -> Option<Build> {
        if !self.fast_forward()? {
            return None;
        }
        self.build()
    }

    // Returns whether HEAD moved.
    pub fn fast_forward(&mut self) -> Option<bool> {
        if git(&["rev-parse", "--abbrev-ref", "HEAD"])? != "main" {
            return Some(false);
        }
        git(&["fetch", "--quiet", "origin", "main"])?;
        let head = git(&["rev-parse", "HEAD"])?;
        let latest = git(&["rev-parse", "origin/main"])?;
        if head == latest || self.failed.as_ref() == Some(&latest) {
            return Some(false);
        }
        log(&format!("updating {} to {}", &head[..7], &latest[..7]));
        if git(&["merge", "--ff-only", "--quiet", "origin/main"]).is_none() {
            log("cannot fast-forward main; staying on the running build");
            self.failed = Some(latest);
            return Some(false);
        }
        // `git diff --quiet` fails when there are differences.
        if git(&["diff", "--quiet", &head, &latest, "--", "crates/launcher"]).is_none() {
            log("this changes the launcher, which runs its old build until it is restarted");
        }
        Some(true)
    }

    // Builds the checkout and returns the paths cargo built the server and gem-native at.
    pub fn build(&mut self) -> Option<Build> {
        log(&format!(
            "building {}",
            git(&["rev-parse", "--short", "HEAD"])?
        ));
        // Diagnostics still render to stderr; stdout carries the messages naming the binaries,
        // each by its target's name.
        let mut cargo = Command::new("cargo")
            .args([
                "build",
                "--release",
                "--message-format=json-render-diagnostics",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        let mut server = None;
        let mut native = None;
        for line in BufReader::new(cargo.stdout.take().unwrap()).lines() {
            let Ok(message) = gem_json::parse(&line.unwrap_or_default()) else {
                continue;
            };
            let Some(path) = message["executable"].as_str() else {
                continue;
            };
            match message["target"]["name"].as_str() {
                Some("server") => server = Some(PathBuf::from(path)),
                Some("gem-native") => native = Some(PathBuf::from(path)),
                _ => {}
            }
        }
        let built = cargo.wait().is_ok_and(|status| status.success());
        let (true, Some(server), Some(native)) = (built, server, native) else {
            log("build failed; staying on the running build");
            self.failed = git(&["rev-parse", "origin/main"]);
            return None;
        };
        Some(Build { server, native })
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
