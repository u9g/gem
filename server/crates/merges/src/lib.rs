// A T3 Code agent merging a cluster's pull requests. Merges are rows in the server's database, one
// per pull request and sharing the agent's thread, until the thread settles or stops, so a cluster
// keeps its one merge across presses, frontends and server restarts. T3 Code is reached through
// `Host`.

use std::{collections::BTreeMap, future::Future, sync::Arc, time::Duration};

use gem_db::{
    Db,
    rusqlite::{self, OptionalExtension, params},
};
use tokio::sync::Mutex;

// Reading a thread is a local CLI call.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);
// How many reads of a thread can fail in a row, e.g. after it is deleted, before its merge is
// dropped and the cluster can be merged again.
const MAX_FAILED_READS: u32 = 30;

const MIGRATIONS: &[&str] = &["
    CREATE TABLE merges (
        url TEXT PRIMARY KEY,
        -- Shared by the cluster's pull requests. NULL while the thread is starting.
        thread_id TEXT,
        question INTEGER NOT NULL DEFAULT 0
    );
"];

pub struct Progress {
    // The thread settled or stopped.
    pub done: bool,
    // The agent waits on the user to answer a question.
    pub question: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Merge {
    // Opens the agent's thread in T3 Code; empty while it starts.
    pub thread_url: String,
    pub question: bool,
}

pub trait Host: Send + Sync + 'static {
    // Starts an agent on `prompt` without waiting for it. Returns the thread's id.
    fn start_thread(
        &self,
        title: &str,
        prompt: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;
    // None when the thread cannot be read.
    fn progress(&self, thread_id: &str) -> impl Future<Output = Option<Progress>> + Send;
    fn thread_url(&self, thread_id: &str) -> String;
}

pub struct Merges<H> {
    db: Db,
    host: Arc<H>,
    // Held for the whole of a start, so two presses of Merge start one agent, and the rows whose
    // thread is starting are that start's.
    starting: Arc<Mutex<()>>,
}

impl<H> Clone for Merges<H> {
    fn clone(&self) -> Self {
        Merges {
            db: self.db.clone(),
            host: self.host.clone(),
            starting: self.starting.clone(),
        }
    }
}

impl<H: Host> Merges<H> {
    pub fn new(db: Db, host: H) -> Result<Self, String> {
        db.migrate("merges", MIGRATIONS)?;
        Ok(Merges {
            db,
            host: Arc::new(host),
            starting: Arc::new(Mutex::new(())),
        })
    }

    // Starts an agent merging the pull requests at `urls`, oldest first, called `name` in its
    // thread's title, unless one of them is already being merged. It forces those in `admin` in
    // past review, as the user chose to, and no others. The agent settles its thread when done:
    // T3 Code does not settle threads when they finish.
    pub async fn start(&self, urls: &[String], admin: &[String], name: &str) -> Result<(), String> {
        let _starting = self.starting.lock().await;
        let running = self.db.with(|db| {
            let mut rows = db.prepare("SELECT 1 FROM merges WHERE url = ?1")?;
            for url in urls {
                if rows.query_row([url], |_| Ok(())).optional()?.is_some() {
                    return Ok(true);
                }
            }
            Ok(false)
        })?;
        if running {
            return Ok(());
        }

        let list: String = (urls.iter())
            .map(|url| match admin.contains(url) {
                true => format!("- {url} (not approved: force it in with `gh pr merge --admin`)\n"),
                false => format!("- {url}\n"),
            })
            .collect();
        let prompt = format!(
            "merge these pull requests, which belong together:
{list}
Merge each with `gh pr merge`, using a merge method its repository allows, in an order that keeps \
every base: a pull request whose base branch is another's head branch merges after that one, \
retargeted with `gh pr edit --base` onto the base it merged into if GitHub did not do it. Merge \
the rest in the order listed. Before each merge, check that its checks pass and it merges cleanly, \
and that it is still approved unless it is marked to be forced in. The user chose to force the \
marked ones in past review as an admin: merge only those with `--admin`, never the others. If one \
does not pass these checks, stop and say why, without merging the ones after it.

When you are done, call the t3_thread_organize tool with action \"settle\" and no threadId, to \
settle this thread."
        );
        self.db.with(|db| {
            let mut insert = db.prepare("INSERT INTO merges (url) VALUES (?1)")?;
            for url in urls {
                insert.execute([url])?;
            }
            Ok(())
        })?;
        let started = self
            .host
            .start_thread(&format!("Merge {name}"), &prompt)
            .await;
        let thread_id = match started {
            Ok(thread_id) => thread_id,
            Err(error) => {
                self.db
                    .with(|db| db.execute("DELETE FROM merges WHERE thread_id IS NULL", []))?;
                return Err(error);
            }
        };
        self.db.with(|db| {
            db.execute(
                "UPDATE merges SET thread_id = ?1 WHERE thread_id IS NULL",
                [&thread_id],
            )
        })?;
        tokio::spawn(self.clone().watch(thread_id));
        Ok(())
    }

    // Watches the merges a previous run of the server left. One whose thread never started is
    // dropped: its server stopped while starting it.
    pub fn resume(&self) -> Result<(), String> {
        let threads = self.db.with(|db| {
            db.execute("DELETE FROM merges WHERE thread_id IS NULL", [])?;
            let mut rows = db.prepare("SELECT DISTINCT thread_id FROM merges")?;
            rows.query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()
        })?;
        for thread_id in threads {
            tokio::spawn(self.clone().watch(thread_id));
        }
        Ok(())
    }

    // By pull request URL.
    pub fn all(&self) -> Result<BTreeMap<String, Merge>, String> {
        self.db.with(|db| {
            let mut rows = db.prepare("SELECT url, thread_id, question FROM merges")?;
            rows.query_map([], |row| {
                let thread_id: Option<String> = row.get(1)?;
                let merge = Merge {
                    thread_url: (thread_id.as_deref())
                        .map(|id| self.host.thread_url(id))
                        .unwrap_or_default(),
                    question: row.get(2)?,
                };
                Ok((row.get(0)?, merge))
            })?
            .collect()
        })
    }

    async fn watch(self, thread_id: String) {
        let mut failed_reads = 0;
        loop {
            tokio::time::sleep(REFRESH_INTERVAL).await;
            let Some(progress) = self.host.progress(&thread_id).await else {
                failed_reads += 1;
                if failed_reads >= MAX_FAILED_READS {
                    eprintln!("merge: cannot read thread {thread_id}; dropping it");
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
                    "UPDATE merges SET question = ?2 WHERE thread_id = ?1",
                    params![thread_id, progress.question],
                )
            });
        }
        let removed = (self.db)
            .with(|db| db.execute("DELETE FROM merges WHERE thread_id = ?1", [&thread_id]));
        if let Err(error) = removed {
            eprintln!("{error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;

    fn urls() -> Vec<String> {
        vec![
            "https://github.com/o/r/pull/1".into(),
            "https://github.com/o/s/pull/2".into(),
        ]
    }

    #[derive(Default)]
    struct State {
        started: Vec<String>,
        prompts: Vec<String>,
        fail_start: bool,
        // None reads as an unreadable thread.
        progress: Option<(bool, bool)>,
    }

    #[derive(Clone, Default)]
    struct FakeHost(Arc<StdMutex<State>>);

    impl FakeHost {
        fn new() -> Self {
            let host = FakeHost::default();
            host.set(|s| s.progress = Some((false, false)));
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
        async fn start_thread(&self, title: &str, prompt: &str) -> Result<String, String> {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let mut state = self.0.lock().unwrap();
            if state.fail_start {
                return Err("t3: could not run the CLI".into());
            }
            state.started.push(title.to_string());
            state.prompts.push(prompt.to_string());
            Ok(format!("thread-{}", state.started.len()))
        }

        async fn progress(&self, _: &str) -> Option<Progress> {
            let (done, question) = self.0.lock().unwrap().progress?;
            Some(Progress { done, question })
        }

        fn thread_url(&self, thread_id: &str) -> String {
            format!("t3://{thread_id}")
        }
    }

    fn merges(host: &FakeHost) -> Merges<FakeHost> {
        Merges::new(Db::open_in_memory().unwrap(), host.clone()).unwrap()
    }

    // Lets the watcher take a few turns of REFRESH_INTERVAL.
    async fn refreshes(count: u32) {
        tokio::time::sleep(REFRESH_INTERVAL * count + Duration::from_secs(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn two_presses_start_one_agent_for_the_whole_cluster() {
        let host = FakeHost::new();
        let merges = merges(&host);
        let urls = urls();
        let (first, second) =
            tokio::join!(merges.start(&urls, &[], "c"), merges.start(&urls, &[], "c"));
        first.unwrap();
        second.unwrap();
        assert_eq!(host.0.lock().unwrap().started, ["Merge c"]);
        // A cluster that grew around a pull request being merged starts nothing either.
        let grown = [urls, vec!["https://github.com/o/t/pull/3".into()]].concat();
        merges.start(&grown, &[], "c").await.unwrap();
        assert_eq!(host.started(), 1);
        let all = merges.all().unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.values().all(|m| m.thread_url == "t3://thread-1"));
    }

    #[tokio::test(start_paused = true)]
    async fn forces_in_only_the_pull_requests_the_user_chose_to() {
        let host = FakeHost::new();
        let merges = merges(&host);
        let urls = urls();
        merges.start(&urls, &urls[1..], "c").await.unwrap();
        let prompt = host.0.lock().unwrap().prompts[0].clone();
        assert!(prompt.contains(&format!("- {}
", urls[0])));
        assert!(prompt.contains(&format!("- {} (not approved: force it in", urls[1])));
    }

    #[tokio::test(start_paused = true)]
    async fn shows_while_its_thread_starts() {
        let host = FakeHost::new();
        let merges = merges(&host);
        let starting = tokio::spawn({
            let merges = merges.clone();
            async move { merges.start(&urls(), &[], "c").await }
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            merges
                .all()
                .unwrap()
                .values()
                .all(|m| m.thread_url.is_empty())
        );
        starting.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_start_can_be_retried() {
        let host = FakeHost::new();
        let merges = merges(&host);
        host.set(|s| s.fail_start = true);
        assert!(merges.start(&urls(), &[], "c").await.is_err());
        assert!(merges.all().unwrap().is_empty());
        host.set(|s| s.fail_start = false);
        merges.start(&urls(), &[], "c").await.unwrap();
        assert_eq!(host.started(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_merge_shows_its_question_and_ends_with_its_thread() {
        let host = FakeHost::new();
        let merges = merges(&host);
        merges.start(&urls(), &[], "c").await.unwrap();
        host.set(|s| s.progress = Some((false, true)));
        refreshes(1).await;
        assert!(merges.all().unwrap().values().all(|m| m.question));
        host.set(|s| s.progress = Some((true, false)));
        refreshes(1).await;
        assert!(merges.all().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn an_unreadable_thread_is_dropped() {
        let host = FakeHost::new();
        let merges = merges(&host);
        merges.start(&urls(), &[], "c").await.unwrap();
        host.set(|s| s.progress = None);
        refreshes(MAX_FAILED_READS - 1).await;
        assert!(!merges.all().unwrap().is_empty());
        refreshes(1).await;
        assert!(merges.all().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_restarted_server_resumes_its_merges() {
        let host = FakeHost::new();
        let db = Db::open_in_memory().unwrap();
        Merges::new(db.clone(), host.clone()).unwrap();
        db.with(|db| {
            db.execute_batch(
                "INSERT INTO merges (url, thread_id) VALUES ('https://github.com/o/r/pull/1', 't');
                 INSERT INTO merges (url, thread_id) VALUES ('https://github.com/o/s/pull/2', 't');
                 INSERT INTO merges (url) VALUES ('https://github.com/o/t/pull/3');",
            )
        })
        .unwrap();

        let merges = Merges::new(db, host.clone()).unwrap();
        merges.resume().unwrap();
        // The one whose thread never started is gone; the other is watched again.
        assert_eq!(merges.all().unwrap().len(), 2);
        host.set(|s| s.progress = Some((true, false)));
        refreshes(1).await;
        assert!(merges.all().unwrap().is_empty());
    }
}
