// A T3 Code agent rebasing a pull request onto its base branch. Rebases are rows in the server's
// database until the agent's thread settles or stops, so a pull request keeps its one rebase across
// presses, frontends and server restarts. GitHub and T3 Code are reached through `Host`.

use std::{collections::BTreeMap, future::Future, sync::Arc, time::Duration};

use gem_db::{
    Db,
    rusqlite::{self, OptionalExtension, params},
};
use tokio::sync::Mutex;

// Reading a thread is a local CLI call.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);
// How many reads of a thread can fail in a row, e.g. after it is deleted, before its rebase is
// dropped and the pull request can be rebased again.
const MAX_FAILED_READS: u32 = 30;

const MIGRATIONS: &[&str] = &["
    CREATE TABLE rebases (
        url TEXT PRIMARY KEY,
        -- The head commit when the rebase started.
        head TEXT NOT NULL,
        -- NULL while the thread is starting.
        thread_id TEXT,
        question INTEGER NOT NULL DEFAULT 0,
        pushed INTEGER NOT NULL DEFAULT 0
    );
"];

pub struct Refs {
    pub base: String,
    pub branch: String,
    pub head: String,
}

pub struct Progress {
    // The thread settled or stopped.
    pub done: bool,
    // The agent waits on the user to answer a question.
    pub question: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rebase {
    // Opens the agent's thread in T3 Code; empty while it starts.
    pub thread_url: String,
    pub question: bool,
    // The agent pushed the rebased branch, and the pull request is still listed with its old head.
    pub pushed: bool,
}

pub trait Host: Send + Sync + 'static {
    // The pull request's base and head branches and head commit, as GitHub has them now.
    fn refs(&self, url: &str) -> impl Future<Output = Option<Refs>> + Send;
    // Starts an agent on `prompt` without waiting for it. Returns the thread's id.
    fn start_thread(
        &self,
        title: &str,
        prompt: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;
    // None when the thread cannot be read.
    fn progress(&self, thread_id: &str) -> impl Future<Output = Option<Progress>> + Send;
    // The head commit gem lists the pull request with, None once it is not listed. Search keeps
    // listing the old head until its next refresh.
    fn listed_head(&self, url: &str) -> impl Future<Output = Option<String>> + Send;
    fn thread_url(&self, thread_id: &str) -> String;
}

pub struct Rebases<H> {
    db: Db,
    host: Arc<H>,
    // Held from checking for a rebase until its row is written, so two presses of Rebase start
    // one agent.
    starting: Arc<Mutex<()>>,
}

impl<H> Clone for Rebases<H> {
    fn clone(&self) -> Self {
        Rebases {
            db: self.db.clone(),
            host: self.host.clone(),
            starting: self.starting.clone(),
        }
    }
}

impl<H: Host> Rebases<H> {
    pub fn new(db: Db, host: H) -> Result<Self, String> {
        db.migrate("rebases", MIGRATIONS)?;
        Ok(Rebases {
            db,
            host: Arc::new(host),
            starting: Arc::new(Mutex::new(())),
        })
    }

    // Starts an agent rebasing the pull request at `url`, called `name` in its thread's title,
    // unless one already is. The agent settles its thread when done: T3 Code does not settle
    // threads when they finish.
    pub async fn start(&self, url: &str, name: &str) -> Result<(), String> {
        let _starting = self.starting.lock().await;
        let running = self.db.with(|db| {
            db.query_row("SELECT 1 FROM rebases WHERE url = ?1", [url], |_| Ok(()))
                .optional()
        })?;
        if running.is_some() {
            return Ok(());
        }

        let Refs { base, branch, head } = (self.host.refs(url).await)
            .ok_or(format!("gh: cannot read {url}"))?;
        // The lease keeps the agent from overwriting a push made after the rebase was asked for.
        let prompt = format!(
            "rebase on {base}: {url}

Its branch {branch} was at {head} when this was asked. If it has moved since, or it is no longer \
behind {base}, stop without pushing. Push with `git push --force-with-lease={branch}:{head}`.

When you are done, call the t3_thread_organize tool with action \"settle\" and no threadId, to \
settle this thread."
        );
        self.db.with(|db| {
            db.execute(
                "INSERT INTO rebases (url, head) VALUES (?1, ?2)",
                params![url, head],
            )
        })?;
        let started = self.host.start_thread(&format!("Rebase {name}"), &prompt).await;
        let thread_id = match started {
            Ok(thread_id) => thread_id,
            Err(error) => {
                self.remove(url)?;
                return Err(error);
            }
        };
        self.db.with(|db| {
            db.execute(
                "UPDATE rebases SET thread_id = ?2 WHERE url = ?1",
                params![url, thread_id],
            )
        })?;
        tokio::spawn(self.clone().watch(url.to_string(), head, thread_id));
        Ok(())
    }

    // Watches the rebases a previous run of the server left. One whose thread never started is
    // dropped: its server stopped while starting it.
    pub fn resume(&self) -> Result<(), String> {
        let rebases = self.db.with(|db| {
            db.execute("DELETE FROM rebases WHERE thread_id IS NULL", [])?;
            let mut rows = db.prepare("SELECT url, head, thread_id FROM rebases")?;
            rows.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<(String, String, String)>>>()
        })?;
        for (url, head, thread_id) in rebases {
            tokio::spawn(self.clone().watch(url, head, thread_id));
        }
        Ok(())
    }

    // By pull request URL.
    pub fn all(&self) -> Result<BTreeMap<String, Rebase>, String> {
        self.db.with(|db| {
            let mut rows = db.prepare("SELECT url, thread_id, question, pushed FROM rebases")?;
            rows.query_map([], |row| {
                let thread_id: Option<String> = row.get(1)?;
                let rebase = Rebase {
                    thread_url: (thread_id.as_deref())
                        .map(|id| self.host.thread_url(id))
                        .unwrap_or_default(),
                    question: row.get(2)?,
                    pushed: row.get(3)?,
                };
                Ok((row.get(0)?, rebase))
            })?
            .collect()
        })
    }

    // A pushed rebase lasts until the pull request is listed with another head, so its old head's
    // checks and how far behind it was do not show as the new head's.
    async fn watch(self, url: String, head: String, thread_id: String) {
        let mut failed_reads = 0;
        loop {
            tokio::time::sleep(REFRESH_INTERVAL).await;
            let Some(progress) = self.host.progress(&thread_id).await else {
                failed_reads += 1;
                if failed_reads >= MAX_FAILED_READS {
                    eprintln!("rebase of {url}: cannot read thread {thread_id}; dropping it");
                    break;
                }
                continue;
            };
            failed_reads = 0;
            if progress.done {
                break;
            }
            let _ = self.db.with(|db| {
                db.execute(
                    "UPDATE rebases SET question = ?2 WHERE url = ?1",
                    params![url, progress.question],
                )
            });
        }
        let pushed = self.host.refs(&url).await.is_some_and(|refs| refs.head != head);
        if pushed {
            let _ = self.db.with(|db| {
                db.execute(
                    "UPDATE rebases SET question = 0, pushed = 1 WHERE url = ?1",
                    [&url],
                )
            });
            while (self.host.listed_head(&url).await).is_some_and(|listed| listed == head) {
                tokio::time::sleep(REFRESH_INTERVAL).await;
            }
        }
        if let Err(error) = self.remove(&url) {
            eprintln!("{error}");
        }
    }

    fn remove(&self, url: &str) -> Result<(), String> {
        self.db
            .with(|db| db.execute("DELETE FROM rebases WHERE url = ?1", [url]))
            .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    const URL: &str = "https://github.com/o/r/pull/1";

    #[derive(Default)]
    struct State {
        head: String,
        listed_head: Option<String>,
        started: Vec<String>,
        fail_start: bool,
        // None reads as an unreadable thread.
        progress: Option<(bool, bool)>,
    }

    #[derive(Clone, Default)]
    struct FakeHost(Arc<StdMutex<State>>);

    impl FakeHost {
        fn new() -> Self {
            let host = FakeHost::default();
            host.set(|s| {
                s.head = "old".into();
                s.listed_head = Some("old".into());
                s.progress = Some((false, false));
            });
            host
        }

        fn set(&self, change: impl FnOnce(&mut State)) {
            change(&mut self.0.lock().unwrap());
        }

        fn started(&self) -> usize {
            self.0.lock().unwrap().started.len()
        }
    }

    impl Host for FakeHost {
        async fn refs(&self, _: &str) -> Option<Refs> {
            // gh takes a while, so a second press can arrive before the first writes its row.
            tokio::time::sleep(Duration::from_millis(100)).await;
            Some(Refs {
                base: "main".into(),
                branch: "fix".into(),
                head: self.0.lock().unwrap().head.clone(),
            })
        }

        async fn start_thread(&self, title: &str, _: &str) -> Result<String, String> {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let mut state = self.0.lock().unwrap();
            if state.fail_start {
                return Err("t3: could not run the CLI".into());
            }
            state.started.push(title.to_string());
            Ok(format!("thread-{}", state.started.len()))
        }

        async fn progress(&self, _: &str) -> Option<Progress> {
            let (done, question) = self.0.lock().unwrap().progress?;
            Some(Progress { done, question })
        }

        async fn listed_head(&self, _: &str) -> Option<String> {
            self.0.lock().unwrap().listed_head.clone()
        }

        fn thread_url(&self, thread_id: &str) -> String {
            format!("t3://{thread_id}")
        }
    }

    fn rebases(host: &FakeHost) -> Rebases<FakeHost> {
        Rebases::new(Db::open_in_memory().unwrap(), host.clone()).unwrap()
    }

    fn rebase(rebases: &Rebases<FakeHost>) -> Option<Rebase> {
        rebases.all().unwrap().remove(URL)
    }

    // Lets the watcher take a few turns of REFRESH_INTERVAL.
    async fn refreshes(count: u32) {
        tokio::time::sleep(REFRESH_INTERVAL * count + Duration::from_secs(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn two_presses_start_one_agent() {
        let host = FakeHost::new();
        let rebases = rebases(&host);
        let (first, second) = tokio::join!(rebases.start(URL, "r#1"), rebases.start(URL, "r#1"));
        first.unwrap();
        second.unwrap();
        assert_eq!(host.0.lock().unwrap().started, ["Rebase r#1"]);
        // Pressing again while the agent works does nothing either.
        rebases.start(URL, "r#1").await.unwrap();
        assert_eq!(host.started(), 1);
        assert_eq!(rebase(&rebases).unwrap().thread_url, "t3://thread-1");
    }

    #[tokio::test(start_paused = true)]
    async fn shows_while_its_thread_starts() {
        let host = FakeHost::new();
        let rebases = rebases(&host);
        let starting = tokio::spawn({
            let rebases = rebases.clone();
            async move { rebases.start(URL, "r#1").await }
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(rebase(&rebases).unwrap().thread_url, "");
        starting.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_start_can_be_retried() {
        let host = FakeHost::new();
        let rebases = rebases(&host);
        host.set(|s| s.fail_start = true);
        assert!(rebases.start(URL, "r#1").await.is_err());
        assert_eq!(rebase(&rebases), None);
        host.set(|s| s.fail_start = false);
        rebases.start(URL, "r#1").await.unwrap();
        assert_eq!(host.started(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_pushed_rebase_lasts_until_the_new_head_is_listed() {
        let host = FakeHost::new();
        let rebases = rebases(&host);
        rebases.start(URL, "r#1").await.unwrap();

        host.set(|s| s.progress = Some((false, true)));
        refreshes(1).await;
        assert!(rebase(&rebases).unwrap().question);

        host.set(|s| {
            s.head = "new".into();
            s.progress = Some((true, false));
        });
        refreshes(1).await;
        let pushed = rebase(&rebases).unwrap();
        assert!(pushed.pushed && !pushed.question);

        host.set(|s| s.listed_head = Some("new".into()));
        refreshes(1).await;
        assert_eq!(rebase(&rebases), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_rebase_that_pushed_nothing_ends_with_its_thread() {
        let host = FakeHost::new();
        let rebases = rebases(&host);
        rebases.start(URL, "r#1").await.unwrap();
        host.set(|s| s.progress = Some((true, false)));
        refreshes(1).await;
        assert_eq!(rebase(&rebases), None);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unreadable_thread_is_dropped() {
        let host = FakeHost::new();
        let rebases = rebases(&host);
        rebases.start(URL, "r#1").await.unwrap();
        host.set(|s| s.progress = None);
        refreshes(MAX_FAILED_READS - 1).await;
        assert!(rebase(&rebases).is_some());
        refreshes(1).await;
        assert_eq!(rebase(&rebases), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_restarted_server_resumes_its_rebases() {
        let host = FakeHost::new();
        let db = Db::open_in_memory().unwrap();
        Rebases::new(db.clone(), host.clone()).unwrap();
        db.with(|db| {
            db.execute_batch(
                "INSERT INTO rebases (url, head, thread_id) VALUES ('https://github.com/o/r/pull/1', 'old', 't');
                 INSERT INTO rebases (url, head) VALUES ('https://github.com/o/r/pull/2', 'old');",
            )
        })
        .unwrap();

        let rebases = Rebases::new(db, host.clone()).unwrap();
        rebases.resume().unwrap();
        // The one whose thread never started is gone; the other is watched again.
        assert_eq!(rebases.all().unwrap().len(), 1);
        rebases.start(URL, "r#1").await.unwrap();
        assert_eq!(host.started(), 0);
        host.set(|s| s.progress = Some((true, false)));
        refreshes(1).await;
        assert_eq!(rebase(&rebases), None);
    }
}
