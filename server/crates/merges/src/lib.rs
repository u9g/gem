// A T3 Code agent merging a cluster's pull requests. Merges are kept in the server's database, one
// per pull request and sharing the agent's thread, until the thread settles or stops, so a cluster
// keeps its one merge across presses, frontends and server restarts. T3 Code is reached through
// `Host`.

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

// In the database's moment, the hash map from the URL of each pull request being merged to a hash
// map of its merge: its `title`, `started-at` in seconds since the epoch, `question` as 1 while the
// agent waits on the user, and its `thread` once it started.
const MERGES: &str = "merges";

// Reading a thread is a local CLI call.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);
// How many reads of a thread can fail in a row, e.g. after it is deleted, before its merge is
// dropped and the cluster can be merged again.
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

pub struct Progress {
    // The thread settled or stopped.
    pub done: bool,
    // The agent waits on the user to answer a question.
    pub question: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Merge {
    // The agent's thread, None while it starts.
    pub thread_id: Option<String>,
    pub title: String,
    // Seconds since the epoch.
    pub started_at: i64,
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
}

pub struct Merges<H> {
    db: Db,
    host: Arc<H>,
    // Held for the whole of a start, so two presses of Merge start one agent, and the merges whose
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
        let running = self.db.read(|moment| {
            let Some(merges) = get_map(moment, MERGES)? else {
                return Ok(false);
            };
            for url in urls {
                if merges.get_cursor(url)?.is_some() {
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

{ASK}

When you are done, call the t3_thread_organize tool with action \"settle\" and no threadId, to \
settle this thread."
        );
        let title = format!("Merge {name}");
        self.db.write(|moment| {
            let mut merges = map_mut(moment, MERGES)?;
            for url in urls {
                merges.remove(url)?;
                let mut merge = map_mut(&mut merges, url)?;
                merge.put("title", &*title)?;
                merge.put("started-at", now())?;
            }
            Ok(())
        })?;
        let started = self.host.start_thread(&title, &prompt).await;
        let thread_id = match started {
            Ok(thread_id) => thread_id,
            Err(error) => {
                self.db.write(|moment| remove(moment, |merge| merge.thread_id.is_none()))?;
                return Err(error);
            }
        };
        self.db.write(|moment| {
            for url in urls_where(moment, |merge| merge.thread_id.is_none())? {
                map_mut(&mut map_mut(moment, MERGES)?, &url)?.put("thread", &*thread_id)?;
            }
            Ok(())
        })?;
        tokio::spawn(self.clone().watch(thread_id));
        Ok(())
    }

    // Watches the merges a previous run of the server left. One whose thread never started is
    // dropped: its server stopped while starting it.
    pub fn resume(&self) -> Result<(), String> {
        let threads = self.db.write(|moment| {
            remove(moment, |merge| merge.thread_id.is_none())?;
            let merges = merges(moment)?.into_values();
            Ok(merges.filter_map(|merge| merge.thread_id).collect::<BTreeSet<_>>())
        })?;
        for thread_id in threads {
            tokio::spawn(self.clone().watch(thread_id));
        }
        Ok(())
    }

    // By pull request URL.
    pub fn all(&self) -> Result<BTreeMap<String, Merge>, String> {
        self.db.read(merges)
    }

    // Stops showing the merge whose agent runs in `thread_id`, as the user asked. Its cluster can be
    // merged again; the agent keeps running in T3 Code.
    pub fn forget(&self, thread_id: &str) -> Result<(), String> {
        let thread_id = Some(thread_id);
        (self.db).write(|moment| remove(moment, |merge| merge.thread_id.as_deref() == thread_id))
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
            let _ = self.db.write(|moment| {
                let ours = |merge: &Merge| merge.thread_id.as_deref() == Some(&*thread_id);
                for url in urls_where(moment, ours)? {
                    let mut merge = map_mut(&mut map_mut(moment, MERGES)?, &url)?;
                    merge.put("question", u64::from(progress.question))?;
                }
                Ok(())
            });
        }
        if let Err(error) = self.forget(&thread_id) {
            eprintln!("{error}");
        }
    }
}

// Each merge in `moment`, by pull request URL.
fn merges(moment: &Map) -> gem_db::Result<BTreeMap<String, Merge>> {
    let Some(merges) = moment.get_cursor(MERGES)? else {
        return Ok(BTreeMap::new());
    };
    (entries(&merges)?.into_iter()).map(|(url, merge)| Ok((url, merge_at(merge)?))).collect()
}

fn merge_at(cursor: Cursor) -> gem_db::Result<Merge> {
    let merge = ReadHashMap::new(cursor)?;
    let started_at = merge.get_cursor("started-at")?.map(|at| at.read_int()).transpose()?;
    let question = merge.get_cursor("question")?.map(|q| q.read_uint()).transpose()?;
    Ok(Merge {
        thread_id: get_text(&merge, "thread")?,
        title: get_text(&merge, "title")?.unwrap_or_default(),
        started_at: started_at.unwrap_or_default(),
        question: question.is_some_and(|question| question != 0),
    })
}

// The URLs of the merges that `matches`.
fn urls_where(moment: &Map, matches: impl Fn(&Merge) -> bool) -> gem_db::Result<Vec<String>> {
    let merges = merges(moment)?.into_iter();
    Ok(merges.filter(|(_, merge)| matches(merge)).map(|(url, _)| url).collect())
}

fn remove(moment: &mut MapMut, matches: impl Fn(&Merge) -> bool) -> gem_db::Result<()> {
    for url in urls_where(moment, matches)? {
        map_mut(moment, MERGES)?.remove(&url)?;
    }
    Ok(())
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
        assert!(all.values().all(|m| m.thread_id.as_deref() == Some("thread-1")));
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
                .all(|m| m.thread_id.is_none())
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
        db.write(|moment| {
            let mut merges = map_mut(moment, MERGES)?;
            for url in ["https://github.com/o/r/pull/1", "https://github.com/o/s/pull/2"] {
                map_mut(&mut merges, url)?.put("thread", "t")?;
            }
            map_mut(&mut merges, "https://github.com/o/t/pull/3").map(drop)
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
