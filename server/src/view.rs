// What frontends show, in an xitdb file of its own, view.xitdb in the server's working
// directory, that each frontend keeps a live replica of and `server mcp` reads. gem.xitdb holds
// agents' bearer tokens, so it is never replicated; this one holds only what frontends render.
//
// Its root is a `History` like gem.xitdb's, kept by the same `Db`. Each snapshot is written as
// one moment, and an unchanged snapshot commits nothing. Since the file only grows, the bytes a
// moment appends are all a replica that has the moments before it needs: a frontend is sent the
// whole file once, and then each moment's bytes as it commits. The file is compacted when it
// opens, and whenever it has grown well past its size then, which gives it a new file ID, so
// every replica, including one from before a restart, is sent a whole copy once it reports one
// of another file.
//
// It holds the plain structs of `types`, stored structurally by xitdb's `Structured` codec,
// every field written, so a reader needs no schema. Writing compares with what's stored and writes
// only what differs, so a change appends the values that changed and the blocks along the path to
// them, not the cluster they are in.
//
// A moment holds:
// - `clusters`: each cluster, by its key. Clusters move up and down the list as they're updated,
//   and a list is compared by index, so they're kept in a map that doesn't move them;
// - `order`: the clusters' keys, in the order they show;
// - `login`, `search` and `agents`, the rest of what the UI shows;
// - `pending`: the actions in progress, by the key the frontend names each by, each with its
//   route, so every frontend greys out what's in progress, not only the one that started it.

use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex},
};

use gem_db::{
    Db, Delta, MapMut, Migrations,
    xitdb::{
        Core, Map, ReadHashMap, Result, Structured, WriteHashMap, get_structured, put_structured,
    },
};
use tokio::sync::broadcast;

use crate::proto::ListOpenPullRequestsResponse;

mod types;
pub use types::*;

// In the server's working directory.
pub const FILE: &str = "view.xitdb";

const CLUSTERS: Map<str, Structured<Cluster>> = Map::new("clusters");
const ORDER: &str = "order";
const LOGIN: &str = "login";
const SEARCH: &str = "search";
const AGENTS: &str = "agents";
const PENDING: Map<str, String> = Map::new("pending");

// How much the file grows past twice its compacted size before it is compacted again. A
// frontend's first copy is the whole file, so it is at most about twice the view.
const GROWTH: i64 = 1 << 20;

// The view's layout has had no versions yet: it holds what the server writes, which a reader of
// another build of gem refuses only once a version says it changed.
pub fn migrations() -> Migrations {
    Migrations::new([]).expect("migrations are in order")
}

// The clusters in the order they show, as the view last held them.
pub fn clusters<C: Core>(moment: &ReadHashMap<'_, C>) -> Result<Vec<Cluster>> {
    let order: Vec<String> = get_structured(moment, ORDER)?.unwrap_or_default();
    let clusters = order.iter().map(|key| CLUSTERS.get(moment, key));
    clusters.filter_map(Result::transpose).collect()
}

// How many changes a stay in the room can fall behind on sending before it skips some, and the
// frontends that missed them sync.
const BACKLOG: usize = 16;

// The view, shared by the snapshot loop, the actions and frontends' syncs, and the changes to send
// to every frontend.
#[derive(Clone)]
pub struct Live {
    db: Db,
    changes: broadcast::Sender<Arc<Delta>>,
    // Held from a write until its change is sent, so changes are sent in the order they commit.
    publishing: Arc<Mutex<()>>,
}

impl Live {
    // Opens the view at `path`, or makes it. An action a stopped server left pending isn't
    // running anymore, so nothing is pending when it opens.
    pub fn open(path: impl AsRef<Path>) -> Result<Live, String> {
        let (db, _) = Db::open_growing(path, &migrations(), GROWTH)?;
        db.write(|moment| moment.remove(PENDING.name()).map(drop))?;
        let (changes, _) = broadcast::channel(BACKLOG);
        Ok(Live { db, changes, publishing: Arc::default() })
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub fn changes(&self) -> broadcast::Sender<Arc<Delta>> {
        self.changes.clone()
    }

    // Writes `response` and sends every frontend the bytes that appended. Returns whether any
    // did.
    pub fn publish(&self, response: &ListOpenPullRequestsResponse) -> bool {
        let shown = Shown::from(response);
        self.commit(|moment| write_view(moment, &shown)).is_ok_and(|((), sent)| sent)
    }

    // Marks the action `key` pending, as `route`, and sends every frontend the change, unless
    // it's pending already, which is None. The key stays pending until the returned `Pending` is
    // finished or dropped.
    pub fn begin(&self, key: &str, route: &str) -> Result<Option<Pending>, String> {
        let started = self.commit(|moment| {
            if PENDING.contains(moment, key)? {
                return Ok(false);
            }
            PENDING.put(moment, key, route).map(|()| true)
        })?;
        // Made only once it started: dropping one takes its key out of `pending`.
        Ok(started.0.then(|| Pending { live: self.clone(), key: key.to_string(), done: false }))
    }

    // The keys of the actions in progress.
    #[cfg(test)]
    pub fn pending(&self) -> Vec<String> {
        let keys = self.db.read(|moment| PENDING.iter(moment)?.map(|e| Ok(e?.0)).collect());
        keys.unwrap_or_default()
    }

    // Writes what `write` writes and sends every frontend the bytes that appended, returning
    // what `write` returned and whether any were. The change is sent before the next write
    // commits, so changes are sent in the order they commit.
    fn commit<T>(&self, write: impl FnOnce(&mut MapMut) -> Result<T>) -> Result<(T, bool), String> {
        let _publishing = self.publishing.lock().unwrap_or_else(|error| error.into_inner());
        let written = self.db.write_replicated(write);
        let written = written.inspect_err(|error| tracing::warn!(%error, "cannot write the view"));
        let (value, delta) = written?;
        let sent = delta.is_some();
        if let Some(delta) = delta {
            // No receiver while no frontend is in the room.
            let _ = self.changes.send(Arc::new(delta));
        }
        Ok((value, sent))
    }
}

// An action in progress. Finishing it, or dropping it, as when its handler fails, panics or is
// cancelled, takes its key out of `pending`.
pub struct Pending {
    live: Live,
    key: String,
    done: bool,
}

impl Pending {
    // Takes the key out of `pending` in the same moment as `response`, the view after the action,
    // so frontends see the action's change and the end of its spinner together.
    pub fn finish(mut self, response: Option<&ListOpenPullRequestsResponse>) {
        self.done = true;
        let key = std::mem::take(&mut self.key);
        let shown = response.map(Shown::from);
        let _ = self.live.commit(|moment| {
            if let Some(shown) = &shown {
                write_view(moment, shown)?;
            }
            PENDING.remove(moment, &key).map(drop)
        });
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.live.commit(|moment| PENDING.remove(moment, &self.key).map(drop));
        }
    }
}

fn write_view<C: Core>(moment: &mut WriteHashMap<'_, C>, shown: &Shown) -> Result<()> {
    // A cluster keeps its key while pull requests join it and titles change: it is the URL of its
    // oldest pull request, which `cluster` and `agents::join` put first. (A name's `names::key`
    // changes with every title.) A pull request is in one cluster, so no two share a key.
    let order: Vec<&str> = (shown.clusters.iter())
        .map(|cluster| cluster.pull_requests.first().map_or("", |pr| pr.url.as_str()))
        .collect();
    for (key, cluster) in order.iter().zip(&shown.clusters) {
        // Writes only what changed in the cluster, and nothing when it didn't.
        CLUSTERS.put(moment, key, cluster)?;
    }
    // The clusters that are gone are the ones the last order showed and this one doesn't.
    let kept: HashSet<&str> = order.iter().copied().collect();
    let before: Vec<String> = get_structured(moment, ORDER)?.unwrap_or_default();
    let gone = before.iter().map(String::as_str).filter(|key| !kept.contains(key));
    CLUSTERS.remove_all(moment, gone)?;
    put_structured(moment, ORDER, &order)?;
    put_structured(moment, LOGIN, &shown.login)?;
    put_structured(moment, SEARCH, &shown.search)?;
    put_structured(moment, AGENTS, &shown.agents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gem_db::xitdb::{CoreMemory, Database, History};
    use pbjson_types::Timestamp;

    use super::*;
    use crate::proto::{
        Agent, AgentState, CheckFailure, Checks, Comment, Event, EventKind, FailedCheck, MergeBy,
        Proposal, PullRequest, PullRequestCluster, Repository,
    };

    // A pull request as gem shows one: enums, timestamps, bools both ways, fields left at their
    // defaults (which pbjson leaves out), and a workflow run ID past 2^53, which pbjson writes
    // as a string.
    fn pull_request(url: &str, n: u32) -> PullRequest {
        let even = n.is_multiple_of(2);
        let failed = FailedCheck {
            id: format!("check-{n}"),
            name: "test (ubuntu-latest)".to_string(),
            conclusion: "FAILURE".to_string(),
            url: format!("https://github.com/u9g/gem/actions/runs/{n}"),
            workflow_run_id: (1 << 60) + u64::from(n),
            excerpt: "assertion failed: left == right\n".repeat(8),
        };
        let proposal = Proposal {
            id: format!("proposal-{n}"),
            summary: "Fix the flaky test by waiting for the room to empty".to_string(),
            pull_request_url: format!("{url}{n}"),
            reply: "The test raced the server leaving the room. ".repeat(6),
            rerun: true,
            check_failure: Some(CheckFailure {
                id: format!("failure-{n}"),
                head_oid: "0123456789abcdef0123456789abcdef01234567".to_string(),
                checks: vec![failed],
            }),
            ..Default::default()
        };
        PullRequest {
            repository: Some(Repository { owner: "u9g".to_string(), name: "gem".to_string() }),
            number: 100 + n,
            title: format!("Make the view live, part {n}"),
            url: format!("{url}{n}"),
            approved: even,
            merge_by: MergeBy::Approval as i32,
            checks_passing: even,
            checks_failing: !even,
            proposals: vec![proposal],
            checks: Some(Checks { total: 14, done: 14, failed: n % 2 }),
            head_oid: "89abcdef0123456789abcdef0123456789abcdef".to_string(),
            base: "main".to_string(),
            created_at: Some(Timestamp { seconds: 1_790_000_000 + i64::from(n), nanos: 0 }),
            events: vec![Event {
                kind: EventKind::Reviewed as i32,
                actor: "theo".to_string(),
                at: Some(Timestamp { seconds: 1_790_000_100 + i64::from(n), nanos: 0 }),
                url: format!("{url}{n}#pullrequestreview-{n}"),
                comments: vec![Comment { id: format!("review-comment-{n}"), ..Default::default() }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn cluster(url: &str, title: &str) -> PullRequestCluster {
        let agent = Agent {
            id: format!("{url}agent"),
            title: format!("Answer the review of {title}"),
            state: AgentState::Waiting as i32,
            pull_request_urls: vec![format!("{url}0")],
            reply: "Done: the comments are answered.".to_string(),
            started_at: Some(Timestamp { seconds: 1_790_000_500, nanos: 0 }),
            ..Default::default()
        };
        PullRequestCluster {
            pull_requests: (0..3).map(|n| pull_request(url, n)).collect(),
            updated_at: Some(Timestamp { seconds: 1_790_001_000, nanos: 0 }),
            name: title.to_string(),
            merge_by: MergeBy::Admin as i32,
            agents: vec![agent],
        }
    }

    fn response(clusters: usize) -> ListOpenPullRequestsResponse {
        ListOpenPullRequestsResponse {
            clusters: (0..clusters)
                .map(|i| cluster(&format!("https://github.com/o/r/pull/{i}/"), "title"))
                .collect(),
            login: "u9g".to_string(),
            search: "org:livekit".to_string(),
            agents: vec![Agent {
                id: "lone".to_string(),
                state: AgentState::Working as i32,
                started_at: Some(Timestamp { seconds: 1_790_000_900, nanos: 0 }),
                ..Default::default()
            }],
        }
    }

    // The view in a file of its own, as the server keeps it.
    struct View {
        live: Live,
        dir: PathBuf,
    }

    impl View {
        fn new() -> Result<View, String> {
            let dir = crate::tokens::tests::temp_dir("view");
            Ok(View { live: Live::open(dir.join(FILE))?, dir })
        }

        // What the write appended.
        fn write(&mut self, response: &ListOpenPullRequestsResponse) -> Result<Option<Delta>> {
            let shown = Shown::from(response);
            let written = self.live.db.write_replicated(|moment| write_view(moment, &shown));
            Ok(written.unwrap().1)
        }
    }

    impl Drop for View {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    // A replica as a frontend keeps it: it applies a delta that follows on from it, ignores one it
    // already has, and otherwise reports what it has, to be sent what it lacks.
    struct Frontend {
        db: Database<CoreMemory>,
        file_id: Option<u64>,
    }

    impl Frontend {
        // What it reports, through xitdb's serde form of its state.
        fn replica(&self) -> Option<gem_db::Replica> {
            let json = serde_json::to_string(&self.db.replica_state().unwrap()).unwrap();
            let state = serde_json::from_str(&json).unwrap();
            Some(gem_db::Replica { file_id: self.file_id?, state })
        }

        // Whether it applied the delta; a frontend that didn't is sent what it lacks.
        fn receive(&mut self, delta: &Delta) -> bool {
            let follows = self.file_id == Some(delta.file_id)
                && delta.start == self.db.committed_size().unwrap();
            if !(follows || delta.start == 0) {
                return false;
            }
            let length = delta.end - delta.start;
            let mut bytes = delta.bytes.as_slice();
            self.db.apply_replicated(delta.start, &mut bytes, length).unwrap();
            self.file_id = Some(delta.file_id);
            true
        }

        fn sync(&mut self, view: &View) -> Delta {
            let delta = view.live.db.replicate(self.replica()).unwrap();
            assert!(self.receive(&delta));
            delta
        }

        // The response as the frontend rebuilds it.
        fn shown(&self) -> Shown {
            let history = History::new(&self.db).unwrap();
            let moment: ReadHashMap<_> = history.latest().unwrap();
            Shown {
                clusters: clusters(&moment).unwrap(),
                login: get_structured(&moment, LOGIN).unwrap().unwrap(),
                search: get_structured(&moment, SEARCH).unwrap().unwrap(),
                agents: get_structured(&moment, AGENTS).unwrap().unwrap(),
            }
        }
    }

    // The structs read back as they were written, every field there, so the stored data describes
    // itself: only an unset `Option` is left out.
    #[test]
    fn the_view_holds_every_field() {
        let mut view = View::new().unwrap();
        let mut shown = response(1);
        // Every field at its default, which pbjson would have left out.
        shown.clusters[0].pull_requests.push(PullRequest {
            url: "https://github.com/o/r/pull/9".into(),
            ..Default::default()
        });
        view.write(&shown).unwrap();
        let mut frontend = Frontend { db: Database::in_memory().unwrap(), file_id: None };
        frontend.sync(&view);
        assert_eq!(frontend.shown(), Shown::from(&shown));
        let history = History::new(&frontend.db).unwrap();
        let moment = history.latest().unwrap();
        let clusters = ReadHashMap::new(moment.get_cursor("clusters").unwrap().unwrap()).unwrap();
        let pair = clusters.iter().unwrap().next().unwrap().unwrap().read_key_value_pair();
        let cluster = ReadHashMap::new(pair.unwrap().value_cursor).unwrap();
        let prs = cluster.get_cursor("pullRequests").unwrap().unwrap();
        let prs = gem_db::xitdb::ReadArrayList::new(prs).unwrap();
        let pr = ReadHashMap::new(prs.get_cursor(-1).unwrap().unwrap()).unwrap();
        let mut keys: Vec<String> = (pr.entries().unwrap())
            .map(|entry| entry.unwrap().key_cursor.read_bytes(None).unwrap())
            .map(|key| String::from_utf8(key).unwrap())
            .collect();
        keys.sort();
        let mut fields: Vec<String> = serde_json::to_value(super::PullRequest::default())
            .unwrap()
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(key, _)| key.clone())
            .collect();
        fields.sort();
        assert_eq!(keys, fields);
        // The Nones: the repository, checks and creation time, which this one hasn't set.
        assert_eq!(fields.len() + 3, 21);
        assert!(keys.contains(&"approved".to_string()) && keys.contains(&"number".to_string()));
    }

    #[test]
    fn an_unchanged_view_commits_nothing() {
        let mut view = View::new().unwrap();
        assert!(view.write(&response(5)).unwrap().is_some());
        let size = view.live.db.replicate(None).unwrap().end;
        assert_eq!(view.write(&response(5)).unwrap(), None);
        assert_eq!(view.live.db.replicate(None).unwrap().end, size);
    }

    #[test]
    fn a_changed_check_appends_a_small_delta() {
        let mut view = View::new().unwrap();
        let mut shown = response(50);
        let whole = view.write(&shown).unwrap().unwrap();
        let pr = &mut shown.clusters[7].pull_requests[1];
        pr.checks = Some(Checks { total: 14, done: 13, failed: 0 });
        pr.checks_failing = false;
        let delta = view.write(&shown).unwrap().unwrap();
        let cluster = serde_json::to_vec(&Cluster::from(&shown.clusters[7])).unwrap().len();
        eprintln!(
            "whole view {} bytes, one cluster's JSON {cluster} bytes, delta {} bytes",
            whole.bytes.len(),
            delta.bytes.len()
        );
        assert!(delta.bytes.len() < 4096, "{} bytes", delta.bytes.len());
        assert!(delta.bytes.len() < cluster, "{} bytes", delta.bytes.len());
    }

    // As the server builds the view each tick, from the cached pull requests with their diffs
    // filled in, a restart and a diff that can't be fetched show the files shown before: a view
    // that showed none for a while and then them again wrote every PR's files again. Production's
    // view.xitdb did on every restart, appending 130 KB to a 660 KB view.
    #[tokio::test]
    async fn a_restart_or_a_failed_fetch_keeps_the_files_shown() {
        // Built fresh each tick: no diffs in it yet, and proposals about files. With no
        // repository, fetching a diff fails at once, without asking GitHub.
        let fresh = || {
            let mut response = response(3);
            let prs = response.clusters.iter_mut().flat_map(|c| &mut c.pull_requests);
            for pr in prs {
                pr.url = format!("{}/files-kept", pr.url);
                pr.repository = None;
                pr.proposals[0].comment = Some(crate::proto::Comment {
                    path: "server/src/view.rs".to_string(),
                    ..Default::default()
                });
            }
            response
        };
        let mut view = View::new().unwrap();
        let mut shown = fresh();
        for pr in shown.clusters.iter_mut().flat_map(|c| &mut c.pull_requests) {
            pr.files = vec![crate::proto::FileDiff {
                path: "server/src/view.rs".to_string(),
                patch: "@@ -1 +1 @@
-a
+b
".repeat(200),
            }];
        }
        view.write(&shown).unwrap().unwrap();
        // The server restarts with the view it showed.
        crate::files::seed(&view.live.db.read(clusters).unwrap()).await;
        for _ in 0..3 {
            let mut response = fresh();
            crate::files::fill(&mut response.clusters).await;
            assert_eq!(view.write(&response).unwrap(), None);
            // Each failed fetch is noted before the next tick.
            tokio::task::yield_now().await;
        }
    }

    #[test]
    fn a_new_replica_is_sent_the_whole_view() {
        let mut view = View::new().unwrap();
        view.write(&response(3)).unwrap();
        let mut frontend = Frontend { db: Database::in_memory().unwrap(), file_id: None };
        let delta = frontend.sync(&view);
        assert_eq!((delta.start, delta.end), (0, view.live.db.replicate(None).unwrap().end));
        assert_eq!(frontend.shown(), Shown::from(&response(3)));
    }

    #[test]
    fn a_replica_applies_each_change() {
        let mut view = View::new().unwrap();
        view.write(&response(3)).unwrap();
        let mut frontend = Frontend { db: Database::in_memory().unwrap(), file_id: None };
        frontend.sync(&view);
        let mut shown = response(3);
        shown.clusters.remove(1);
        shown.clusters.push(cluster("https://github.com/o/r/pull/9/", "new"));
        shown.clusters[0].pull_requests[0].approved = false;
        shown.clusters[0].agents.clear();
        shown.login = "other".to_string();
        let delta = view.write(&shown).unwrap().unwrap();
        assert!(frontend.receive(&delta));
        assert_eq!(frontend.shown(), Shown::from(&shown));
        // One it already has, from a sync that raced the change, is not applied twice.
        let synced = view.live.db.replicate(frontend.replica()).unwrap();
        assert_eq!(synced.start, synced.end);
        assert!(!frontend.receive(&delta));
    }

    #[test]
    fn a_replica_that_missed_a_change_syncs_what_it_lacks() {
        let mut view = View::new().unwrap();
        view.write(&response(3)).unwrap();
        let mut frontend = Frontend { db: Database::in_memory().unwrap(), file_id: None };
        frontend.sync(&view);
        let before = frontend.db.committed_size().unwrap();
        view.write(&response(4)).unwrap().unwrap();
        let next = view.write(&response(5)).unwrap().unwrap();
        assert!(!frontend.receive(&next));
        let delta = frontend.sync(&view);
        assert_eq!(delta.start, before);
        assert_eq!(frontend.shown(), Shown::from(&response(5)));
    }

    #[test]
    fn a_compacted_view_replaces_every_replica() {
        let mut view = View::new().unwrap();
        view.write(&response(3)).unwrap();
        let mut frontend = Frontend { db: Database::in_memory().unwrap(), file_id: None };
        frontend.sync(&view);
        let file_id = view.live.db.replicate(None).unwrap().file_id;
        // Growing past the threshold compacts it; each change is still sent from the old file.
        let mut shown = response(3);
        for i in 0..20_000 {
            shown.clusters[0].name = format!("name {i}");
            let delta = view.write(&shown).unwrap().unwrap();
            assert!(frontend.receive(&delta));
            if view.live.db.replicate(None).unwrap().file_id != file_id {
                break;
            }
        }
        assert_ne!(view.live.db.replicate(None).unwrap().file_id, file_id);
        assert!(view.live.db.replicate(None).unwrap().end < GROWTH);
        let delta = frontend.sync(&view);
        assert_eq!(delta.start, 0);
        assert_eq!(frontend.shown(), Shown::from(&shown));
        // Later changes follow on from the compacted copy.
        shown.clusters[1].name = "after".to_string();
        assert!(frontend.receive(&view.write(&shown).unwrap().unwrap()));
        assert_eq!(frontend.shown(), Shown::from(&shown));
    }

    #[test]
    fn gone_clusters_are_the_ones_the_last_order_showed() {
        let mut view = View::new().unwrap();
        view.write(&response(3)).unwrap();
        // A cluster the order never showed is never looked at, so it stays.
        let stray = cluster("https://github.com/o/r/pull/stray/", "stray");
        let stray = Cluster::from(&stray);
        (view.live.db.write(|moment| CLUSTERS.put(moment, "stray", &stray))).unwrap();
        view.write(&response(2)).unwrap().unwrap();
        let (gone, kept) = (view.live.db)
            .read(|moment| {
                let gone = CLUSTERS.contains(moment, "https://github.com/o/r/pull/2/0")?;
                Ok((gone, CLUSTERS.contains(moment, "stray")?))
            })
            .unwrap();
        assert!(!gone && kept);
    }

    #[test]
    fn publishing_sends_each_change_to_every_frontend() {
        let view = View::new().unwrap();
        let mut changes = view.live.changes().subscribe();
        let before = view.live.db.replicate(None).unwrap().end;
        assert!(view.live.publish(&response(2)));
        let delta = changes.try_recv().unwrap();
        assert_eq!((delta.start, delta.end), (before, view.live.db.replicate(None).unwrap().end));
        assert!(!view.live.publish(&response(2)));
        assert!(changes.try_recv().is_err());
    }

    #[test]
    fn a_restarted_server_has_nothing_pending() {
        let mut view = View::new().unwrap();
        let pending = view.live.begin("p1", "/proposals/approve").unwrap().unwrap();
        // Two can't be pending under one key.
        assert!(view.live.begin("p1", "/proposals/approve").unwrap().is_none());
        assert_eq!(view.live.pending(), ["p1"]);
        drop(pending);
        assert!(view.live.pending().is_empty());
        // As when the server stops while an action runs.
        (view.live.db.write(|moment| PENDING.put(moment, "p2", "/proposals/approve"))).unwrap();
        let path = view.dir.join(FILE);
        view.live = Live::open(view.dir.join("other.xitdb")).unwrap();
        view.live = Live::open(&path).unwrap();
        assert!(view.live.pending().is_empty());
    }

    #[test]
    fn a_restarted_server_sends_every_replica_a_whole_copy() {
        let mut view = View::new().unwrap();
        view.write(&response(3)).unwrap();
        let mut frontend = Frontend { db: Database::in_memory().unwrap(), file_id: None };
        frontend.sync(&view);
        let file_id = view.live.db.replicate(None).unwrap().file_id;
        // The file outlives the server, and is compacted, with a new file ID, when it opens.
        let path = view.dir.join(FILE);
        view.live = Live::open(view.dir.join("other.xitdb")).unwrap();
        view.live = Live::open(&path).unwrap();
        assert_ne!(view.live.db.replicate(None).unwrap().file_id, file_id);
        let delta = frontend.sync(&view);
        assert_eq!(delta.start, 0);
        assert_eq!(frontend.shown(), Shown::from(&response(3)));
    }

    // With GEM_VIEW_FIXTURE set to a directory, writes the bytes the frontend's view-replica test
    // replays: `full.bin`, a whole view, then `delta.bin`, a change to it, with `fixture.json`
    // saying which bytes each is and what the frontend should show after each, as pbjson's JSON.
    #[test]
    fn writes_a_fixture_for_the_frontend() {
        let Some(dir) = std::env::var_os("GEM_VIEW_FIXTURE") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut view = View::new().unwrap();
        let first = response(3);
        let full = {
            view.write(&first).unwrap();
            view.live.db.replicate(None).unwrap()
        };
        let mut second = first.clone();
        second.clusters.swap(0, 2);
        second.clusters[1].name = "renamed".to_string();
        second.clusters[1].pull_requests[0].approved = false;
        second.clusters[1].pull_requests[2].checks = None;
        second.clusters.pop();
        let delta = view.write(&second).unwrap().unwrap();
        std::fs::write(dir.join("full.bin"), &full.bytes).unwrap();
        std::fs::write(dir.join("delta.bin"), &delta.bytes).unwrap();
        // As the room sends their attributes.
        let attributes = |delta: &Delta| {
            serde_json::json!({
                "fileId": delta.file_id.to_string(),
                "start": delta.start.to_string(),
                "end": delta.end.to_string(),
            })
        };
        // What the view holds of `response`, as the view's structs serialize.
        let json = |response: &ListOpenPullRequestsResponse| {
            let shown = Shown::from(response);
            serde_json::json!({
                "clusters": shown.clusters,
                "login": shown.login,
                "search": shown.search,
                "agents": shown.agents,
            })
        };
        let fixture = serde_json::json!({
            "full": attributes(&full),
            "delta": attributes(&delta),
            "first": json(&first),
            "second": json(&second),
        });
        std::fs::write(dir.join("fixture.json"), fixture.to_string()).unwrap();
    }
}
