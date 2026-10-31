// A T3 Code agent rebasing pull requests onto their base branches. Rebases are kept in the server's
// database, one per pull request and sharing the agent's thread, until the thread settles or stops,
// so a pull request keeps its one rebase across presses, frontends and server restarts. GitHub and
// T3 Code are reached through `Host`.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gem_db::{
    Cursor, Db, Map, MapMut, entries, get_map, get_text, map_mut,
    xitdb::ReadHashMap,
};
use tokio::sync::Mutex;

// In the database's moment, the hash map from the URL of each pull request being rebased to a hash
// map of its rebase: the `head` commit when it started, its `title`, `started-at` in seconds since
// the epoch, `question` as 1 while the agent waits on the user, `pushed` as 1 once it pushed, and
// its `thread` once it started.
const REBASES: &str = "rebases";

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
            let Some(rebases) = get_map(moment, REBASES)? else {
                return Ok(false);
            };
            for url in urls {
                if rebases.get_cursor(url)?.is_some() {
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
            let mut rebases = map_mut(moment, REBASES)?;
            for (url, head) in urls.iter().zip(&heads) {
                rebases.remove(url)?;
                let mut rebase = map_mut(&mut rebases, url)?;
                rebase.put("head", &**head)?;
                rebase.put("title", &*title)?;
                rebase.put("started-at", now())?;
            }
            Ok(())
        })?;
        let started = self.host.start_thread(&title, &prompt).await;
        let thread_id = match started {
            Ok(thread_id) => thread_id,
            Err(error) => {
                self.db.write(|moment| remove(moment, |rebase| rebase.thread_id.is_none()))?;
                return Err(error);
            }
        };
        self.db.write(|moment| {
            for url in urls_where(moment, |rebase| rebase.thread_id.is_none())? {
                map_mut(&mut map_mut(moment, REBASES)?, &url)?.put("thread", &*thread_id)?;
            }
            Ok(())
        })?;
        tokio::spawn(self.clone().watch(thread_id));
        Ok(())
    }

    // Watches the rebases a previous run of the server left. One whose thread never started is
    // dropped: its server stopped while starting it.
    pub fn resume(&self) -> Result<(), String> {
        let rebases = self.db.write(|moment| {
            remove(moment, |rebase| rebase.thread_id.is_none())?;
            let rebases = rebases(moment)?.into_values();
            Ok(rebases.filter_map(|(rebase, _)| rebase.thread_id).collect::<BTreeSet<_>>())
        })?;
        for thread_id in rebases {
            tokio::spawn(self.clone().watch(thread_id));
        }
        Ok(())
    }

    // By pull request URL.
    pub fn all(&self) -> Result<BTreeMap<String, Rebase>, String> {
        let rebases = self.db.read(rebases)?.into_iter();
        Ok(rebases.map(|(url, (rebase, _))| (url, rebase)).collect())
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
            let _ = self.db.write(|moment| {
                let ours = |rebase: &Rebase| rebase.thread_id.as_deref() == Some(&*thread_id);
                for url in urls_where(moment, ours)? {
                    let mut rebase = map_mut(&mut map_mut(moment, REBASES)?, &url)?;
                    rebase.put("question", u64::from(progress.question))?;
                }
                Ok(())
            });
        }
        let rebases = self.db.read(|moment| {
            let rebases = rebases(moment)?.into_iter();
            Ok((rebases.filter(|(_, (rebase, _))| rebase.thread_id.as_deref() == Some(&*thread_id)))
                .map(|(url, (_, head))| (url, head))
                .collect::<Vec<_>>())
        });
        let rebases = rebases.unwrap_or_else(|error| {
            eprintln!("{error}");
            Vec::new()
        });
        let mut pushed = Vec::new();
        for (url, head) in rebases {
            if self.host.refs(&url).await.is_some_and(|refs| refs.head != head) {
                let _ = self.db.write(|moment| {
                    let mut rebase = map_mut(&mut map_mut(moment, REBASES)?, &url)?;
                    rebase.put("question", 0u64)?;
                    rebase.put("pushed", 1u64)
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
        let thread_id = Some(thread_id);
        (self.db).write(|moment| remove(moment, |rebase| rebase.thread_id.as_deref() == thread_id))
    }

    fn remove(&self, url: &str) -> Result<(), String> {
        self.db.write(|moment| map_mut(moment, REBASES)?.remove(url)).map(drop)
    }
}

// Each rebase in `moment`, with the head it started at, by pull request URL.
fn rebases(moment: &Map) -> gem_db::Result<BTreeMap<String, (Rebase, String)>> {
    let Some(rebases) = moment.get_cursor(REBASES)? else {
        return Ok(BTreeMap::new());
    };
    (entries(&rebases)?.into_iter()).map(|(url, rebase)| Ok((url, rebase_at(rebase)?))).collect()
}

fn rebase_at(cursor: Cursor) -> gem_db::Result<(Rebase, String)> {
    let rebase = ReadHashMap::new(cursor)?;
    let flag = |name: &str| -> gem_db::Result<bool> {
        let flag = rebase.get_cursor(name)?.map(|flag| flag.read_uint()).transpose()?;
        Ok(flag.is_some_and(|flag| flag != 0))
    };
    let started_at = rebase.get_cursor("started-at")?.map(|at| at.read_int()).transpose()?;
    let head = get_text(&rebase, "head")?.unwrap_or_default();
    Ok((
        Rebase {
            thread_id: get_text(&rebase, "thread")?,
            title: get_text(&rebase, "title")?.unwrap_or_default(),
            started_at: started_at.unwrap_or_default(),
            question: flag("question")?,
            pushed: flag("pushed")?,
        },
        head,
    ))
}

// The URLs of the rebases that `matches`.
fn urls_where(moment: &Map, matches: impl Fn(&Rebase) -> bool) -> gem_db::Result<Vec<String>> {
    let rebases = rebases(moment)?.into_iter();
    Ok(rebases.filter(|(_, (rebase, _))| matches(rebase)).map(|(url, _)| url).collect())
}

fn remove(moment: &mut MapMut, matches: impl Fn(&Rebase) -> bool) -> gem_db::Result<()> {
    for url in urls_where(moment, matches)? {
        map_mut(moment, REBASES)?.remove(&url)?;
    }
    Ok(())
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
        db.write(|moment| {
            let mut rebases = map_mut(moment, REBASES)?;
            let mut started = map_mut(&mut rebases, "https://github.com/o/r/pull/1")?;
            started.put("head", "old")?;
            started.put("thread", "t")?;
            map_mut(&mut rebases, "https://github.com/o/r/pull/2")?.put("head", "old")
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
