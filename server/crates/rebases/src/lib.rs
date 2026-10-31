// A T3 Code agent rebasing pull requests onto their base branches. Rebases are kept in the server's
// database, one per pull request and sharing the agent's thread, until the thread settles or stops,
// so a pull request keeps its one rebase across presses, frontends and server restarts. GitHub and
// T3 Code are reached through `Host`.

use std::{
    collections::BTreeMap,
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gem_db::{
    Db, MapMut,
    xitdb::{Index, IndexedMap, Map, Record},
};
use tokio::sync::Mutex;

// In the database's moment, each rebase by the URL of its pull request,
const MAP: Map<str, Stored> = Map::new("rebases");
// and the URLs of the rebases by the thread of their agent.
const BY_THREAD: Index<str, Stored, str> =
    Index::new(MAP, "rebases-by-thread", |rebase| rebase.thread_id.clone());
const REBASES: IndexedMap<str, Stored> = IndexedMap::new(MAP, &[&BY_THREAD]);

// Builds the index of rebases by thread from the rebases, for the migration that adds it.
pub fn rebuild_index(moment: &mut MapMut) -> gem_db::Result<bool> {
    REBASES.rebuild(moment)
}

// Reading a thread is a local CLI call.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);
// How many reads of a thread can fail in a row, e.g. after it is deleted, before its rebase is
// dropped and the pull request can be rebased again.
const MAX_FAILED_READS: u32 = 30;

// The agent can ask the user only in its reply: gem shows the reply and sends the answer back as
// the user's next message.
const ASK: &str = "If you need the user to decide something, ask it in your reply and end your \
turn without settling this thread: their answer arrives as your next message. Never ask with a \
tool.";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

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
    // The agent's thread, None while it starts.
    pub thread_id: Option<String>,
    pub title: String,
    // Seconds since the epoch.
    pub started_at: i64,
    pub question: bool,
    // The agent pushed the rebased branch, and the pull request is still listed with its old head.
    pub pushed: bool,
}

// A rebase as stored, as a hash map of its fields: with the head commit when it started.
#[derive(Clone, Debug, PartialEq, Record)]
#[xitdb(crate = "gem_db::xitdb")]
struct Stored {
    head: String,
    #[xitdb(rename = "thread")]
    thread_id: Option<String>,
    title: String,
    #[xitdb(rename = "started-at")]
    started_at: i64,
    question: bool,
    pushed: bool,
}

impl Stored {
    fn rebase(self) -> Rebase {
        Rebase {
            thread_id: self.thread_id,
            title: self.title,
            started_at: self.started_at,
            question: self.question,
            pushed: self.pushed,
        }
    }
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
}

pub struct Rebases<H> {
    db: Db,
    host: Arc<H>,
    // Held for the whole of a start, so two presses of Rebase start one agent, and the rebases whose
    // thread is starting are that start's.
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
        Ok(Rebases {
            db,
            host: Arc::new(host),
            starting: Arc::new(Mutex::new(())),
        })
    }

    // Starts one agent rebasing the pull requests at `urls`, called `name` in its thread's title,
    // unless one of them already is being rebased. The agent settles its thread when done: T3 Code
    // does not settle threads when they finish.
    pub async fn start(&self, urls: &[String], name: &str) -> Result<(), String> {
        let _starting = self.starting.lock().await;
        let running = self.db.read(|moment| {
            for url in urls {
                if REBASES.contains(moment, url)? {
                    return Ok(true);
                }
            }
            Ok(false)
        })?;
        if running {
            return Ok(());
        }

        let mut heads = Vec::new();
        for url in urls {
            let refs = (self.host.refs(url).await).ok_or(format!("gh: cannot read {url}"))?;
            heads.push(refs.head);
        }
        let prompt = format!(
            "rebase these PRs: {}

{ASK}

When you are done, call the t3_thread_organize tool with action \"settle\" and no threadId, to \
settle this thread.",
            urls.join(", ")
        );
        let title = format!("Rebase {name}");
        self.db.write(|moment| {
            for (url, head) in urls.iter().zip(heads) {
                let rebase = Stored {
                    head,
                    thread_id: None,
                    title: title.clone(),
                    started_at: now(),
                    question: false,
                    pushed: false,
                };
                REBASES.put(moment, url, &rebase)?;
            }
            Ok(())
        })?;
        let started = self.host.start_thread(&title, &prompt).await;
        let thread_id = match started {
            Ok(thread_id) => thread_id,
            Err(error) => {
                self.db.write(remove_starting)?;
                return Err(error);
            }
        };
        self.db.write(|moment| {
            let starting = |_: &String, rebase: &Stored| rebase.thread_id.is_none();
            let start = |rebase: &mut Stored| rebase.thread_id = Some(thread_id.clone());
            REBASES.update_where(moment, starting, start)
        })?;
        tokio::spawn(self.clone().watch(thread_id));
        Ok(())
    }

    // Watches the rebases a previous run of the server left. One whose thread never started is
    // dropped: its server stopped while starting it.
    pub fn resume(&self) -> Result<(), String> {
        let rebases = self.db.write(|moment| {
            remove_starting(moment)?;
            BY_THREAD.keys(moment)
        })?;
        for thread_id in rebases {
            tokio::spawn(self.clone().watch(thread_id));
        }
        Ok(())
    }

    // By pull request URL.
    pub fn all(&self) -> Result<BTreeMap<String, Rebase>, String> {
        let rebases =
            self.db.read(|moment| REBASES.iter(moment)?.collect::<gem_db::Result<Vec<_>>>())?;
        Ok(rebases.into_iter().map(|(url, rebase)| (url, rebase.rebase())).collect())
    }

    // A pushed rebase lasts until the pull request is listed with another head, so its old head's
    // checks and how far behind it was do not show as the new head's.
    async fn watch(self, thread_id: String) {
        let mut failed_reads = 0;
        loop {
            tokio::time::sleep(REFRESH_INTERVAL).await;
            let Some(progress) = self.host.progress(&thread_id).await else {
                failed_reads += 1;
                if failed_reads >= MAX_FAILED_READS {
                    eprintln!("rebase: cannot read thread {thread_id}; dropping it");
                    break;
                }
                continue;
            };
            failed_reads = 0;
            if progress.done {
                break;
            }
            // Writes nothing, so the history doesn't grow, unless the question changed.
            let _ = self.db.write(|moment| {
                let ours =
                    |_: &String, rebase: &Stored| rebase.thread_id.as_deref() == Some(&*thread_id);
                REBASES.update_where(moment, ours, |rebase| rebase.question = progress.question)
            });
        }
        let rebases = self.db.read(|moment| {
            let rebases = BY_THREAD.entries(moment, &thread_id)?.into_iter();
            Ok(rebases.map(|(url, rebase)| (url, rebase.head)).collect::<Vec<_>>())
        });
        let rebases = rebases.unwrap_or_else(|error| {
            eprintln!("{error}");
            Vec::new()
        });
        let mut pushed = Vec::new();
        for (url, head) in rebases {
            if self.host.refs(&url).await.is_some_and(|refs| refs.head != head) {
                let _ = self.db.write(|moment| {
                    let this = |other: &String, _: &Stored| *other == url;
                    REBASES.update_where(moment, this, |rebase| {
                        rebase.question = false;
                        rebase.pushed = true;
                    })
                });
                pushed.push((url, head));
            } else if let Err(error) = self.remove(&url) {
                eprintln!("{error}");
            }
        }
        for (url, head) in pushed {
            while (self.host.listed_head(&url).await).is_some_and(|listed| listed == head) {
                tokio::time::sleep(REFRESH_INTERVAL).await;
            }
            if let Err(error) = self.remove(&url) {
                eprintln!("{error}");
            }
        }
    }

    // Stops showing the rebase whose agent runs in `thread_id`, as the user asked. Its pull requests
    // can be rebased again; the agent keeps running in T3 Code.
    pub fn forget(&self, thread_id: &str) -> Result<(), String> {
        self.db.write(|moment| {
            let urls = BY_THREAD.get(moment, thread_id)?;
            REBASES.remove_all(moment, urls).map(drop)
        })
    }

    fn remove(&self, url: &str) -> Result<(), String> {
        self.db.write(|moment| REBASES.remove(moment, url)).map(drop)
    }
}

// Drops the rebases whose thread never started: their server stopped, or failed, while starting it.
fn remove_starting(moment: &mut MapMut) -> gem_db::Result<()> {
    REBASES.retain(moment, |_, rebase| rebase.thread_id.is_some()).map(drop)
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
            // gh takes a while, so a second press can arrive before the first saves its rebase.
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
        let urls = [URL.to_string()];
        let (first, second) =
            tokio::join!(rebases.start(&urls, "r#1"), rebases.start(&urls, "r#1"));
        first.unwrap();
        second.unwrap();
        assert_eq!(host.0.lock().unwrap().started, ["Rebase r#1"]);
        // Pressing again while the agent works does nothing either.
        rebases.start(&[URL.into()], "r#1").await.unwrap();
        assert_eq!(host.started(), 1);
        let started = rebase(&rebases).unwrap();
        assert_eq!(started.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(started.title, "Rebase r#1");
    }

    #[tokio::test(start_paused = true)]
    async fn a_cluster_is_rebased_by_one_agent() {
        let host = FakeHost::new();
        let rebases = rebases(&host);
        let other = "https://github.com/o/s/pull/2";
        rebases
            .start(&[URL.into(), other.into()], "c")
            .await
            .unwrap();
        assert_eq!(host.started(), 1);
        // Neither can be rebased again while the agent works.
        rebases.start(&[other.into()], "s#2").await.unwrap();
        assert_eq!(host.started(), 1);
        let all = rebases.all().unwrap();
        assert_eq!(all[URL].thread_id.as_deref(), Some("thread-1"));
        assert_eq!(all[other].thread_id.as_deref(), Some("thread-1"));

        host.set(|s| s.progress = Some((true, false)));
        refreshes(1).await;
        assert!(rebases.all().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn shows_while_its_thread_starts() {
        let host = FakeHost::new();
        let rebases = rebases(&host);
        let starting = tokio::spawn({
            let rebases = rebases.clone();
            async move { rebases.start(&[URL.into()], "r#1").await }
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(rebase(&rebases).unwrap().thread_id, None);
        starting.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_start_can_be_retried() {
        let host = FakeHost::new();
        let rebases = rebases(&host);
        host.set(|s| s.fail_start = true);
        assert!(rebases.start(&[URL.into()], "r#1").await.is_err());
        assert_eq!(rebase(&rebases), None);
        host.set(|s| s.fail_start = false);
        rebases.start(&[URL.into()], "r#1").await.unwrap();
        assert_eq!(host.started(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_pushed_rebase_lasts_until_the_new_head_is_listed() {
        let host = FakeHost::new();
        let rebases = rebases(&host);
        rebases.start(&[URL.into()], "r#1").await.unwrap();

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
        rebases.start(&[URL.into()], "r#1").await.unwrap();
        host.set(|s| s.progress = Some((true, false)));
        refreshes(1).await;
        assert_eq!(rebase(&rebases), None);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unreadable_thread_is_dropped() {
        let host = FakeHost::new();
        let rebases = rebases(&host);
        rebases.start(&[URL.into()], "r#1").await.unwrap();
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
        // As a server from before the index wrote them, and then the migration indexed them.
        db.write(|moment| {
            let mut rebases = moment.put_map("rebases")?;
            let mut started = rebases.put_map("https://github.com/o/r/pull/1")?;
            started.put("head", "old")?;
            started.put("thread", "t")?;
            rebases.put_map("https://github.com/o/r/pull/2")?.put("head", "old")?;
            rebuild_index(moment).map(drop)
        })
        .unwrap();

        let rebases = Rebases::new(db, host.clone()).unwrap();
        rebases.resume().unwrap();
        // The one whose thread never started is gone; the other is watched again.
        assert_eq!(rebases.all().unwrap().len(), 1);
        rebases.start(&[URL.into()], "r#1").await.unwrap();
        assert_eq!(host.started(), 0);
        host.set(|s| s.progress = Some((true, false)));
        refreshes(1).await;
        assert_eq!(rebase(&rebases), None);
    }
}
