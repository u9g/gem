// What frontends show, in an xitdb database of its own that each frontend keeps a live replica
// of. gem.xitdb holds agents' bearer tokens, so it is never replicated; this one holds only what
// frontends render, in memory, and is made again when the server starts.
//
// Its root is a `History` like gem.xitdb's. Each snapshot is written as one moment, and an
// unchanged snapshot commits nothing. Since the file only grows, the bytes a moment appends are
// all a replica that has the moments before it needs: a frontend is sent the whole database once,
// and then each moment's bytes as it commits.
//
// Everything in it is stored structurally, by xitdb's `Structured` codec: a message is a hash map
// from each field's name, as pbjson names it (camelCase), to its value, a repeated field is an
// array list, and a string, an enum (pbjson writes its name) or a 64-bit integer (pbjson writes it
// as a decimal string) is bytes. A bool is the text `true` or `false`, tagged `bl`. pbjson leaves
// out fields that hold their default, so those are missing keys. Writing compares with what's
// stored and writes only what differs, so a change appends the values that changed and the
// blocks along the path to them, not the cluster they are in.
//
// A moment holds:
// - `clusters`: each cluster, by `key`. Clusters move up and down the list as they're updated,
//   and a list is compared by index, so they're kept in a map that doesn't move them;
// - `order`: the clusters' keys, in the order they show;
// - `login`, `search` and `agents`, the rest of the `ListOpenPullRequestsResponse`.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use gem_db::xitdb::{
    Core, CoreMemory, Database, History, Map, MemoryOffsetMap, ReplicaState, Result, Structured,
    WriteHashMap, put_structured,
};
use gem_room::Stream;
use tokio::sync::broadcast;

use crate::proto::{ListOpenPullRequestsResponse, PullRequestCluster};

const CLUSTERS: Map<str, Structured<PullRequestCluster>> = Map::new("clusters");
const ORDER: &str = "order";
const LOGIN: &str = "login";
const SEARCH: &str = "search";
const AGENTS: &str = "agents";

// How much the database grows past twice its compacted size before it is compacted again. A
// frontend's first copy is the whole database, so it is at most about twice the view.
const GROWTH: i64 = 1 << 20;

pub struct View {
    db: Database<CoreMemory>,
    // Its committed size when it was last compacted.
    compacted: i64,
}

// The committed bytes from `start` to `end` of the database with `file_id`, for a replica whose
// committed size is `start`, or for any replica, which they replace, when `start` is 0.
#[derive(Debug, Clone, PartialEq)]
pub struct Delta {
    pub file_id: u64,
    pub start: i64,
    pub end: i64,
    pub bytes: Vec<u8>,
}

impl Delta {
    // As a frontend reads it: the file ID as a decimal string, since it is a u64 and JavaScript's
    // numbers are not.
    pub fn attributes(&self) -> HashMap<String, String> {
        HashMap::from([
            ("fileId".to_string(), self.file_id.to_string()),
            ("start".to_string(), self.start.to_string()),
            ("end".to_string(), self.end.to_string()),
        ])
    }
}

impl View {
    pub fn new() -> Result<View> {
        let db = Database::in_memory()?;
        let compacted = db.committed_size()?;
        Ok(View { db, compacted })
    }

    // Writes `response` as the latest moment, and returns the bytes that appended, or None when
    // nothing changed.
    pub fn write(&mut self, response: &ListOpenPullRequestsResponse) -> Result<Option<Delta>> {
        let start = self.db.committed_size()?;
        History::new(&self.db)?.transact(|moment| write_view(moment, response))?;
        let end = self.db.committed_size()?;
        if end == start {
            return Ok(None);
        }
        let delta = self.read(start, end)?;
        // After the bytes are read: a replica that applies them has the old file's last moment,
        // the same as the compacted one's, and is replaced when it next asks for a sync.
        if end > 2 * self.compacted + GROWTH {
            self.compact()?;
        }
        Ok(Some(delta))
    }

    // The bytes a replica in `replica` lacks: from its committed size when it is a prefix of this
    // database, and otherwise all of it.
    pub fn sync(&self, replica: Option<ReplicaState>) -> Result<Delta> {
        let start = match replica {
            None => 0,
            Some(replica) => self.db.replication_start(replica)?,
        };
        self.read(start, self.db.committed_size()?)
    }

    // Replaces the database with one holding only its last moment, with a file ID of its own, so
    // every replica is replaced when it next syncs.
    fn compact(&mut self) -> Result<()> {
        self.db = self.db.compact(CoreMemory::new(), &mut MemoryOffsetMap::new())?;
        self.compacted = self.db.committed_size()?;
        Ok(())
    }

    fn read(&self, start: i64, end: i64) -> Result<Delta> {
        let mut bytes = Vec::with_capacity((end - start) as usize);
        self.db.read_committed(start, end, &mut bytes)?;
        let file_id = self.db.header().file_id;
        Ok(Delta { file_id, start, end, bytes })
    }
}

// How many changes a stay in the room can fall behind on sending before it skips some, and the
// frontends that missed them sync.
const BACKLOG: usize = 16;

// The view, shared by the snapshot loop and frontends' syncs, and the changes to send to every
// frontend.
#[derive(Clone)]
pub struct Live {
    view: Arc<Mutex<View>>,
    changes: broadcast::Sender<Arc<Stream>>,
}

impl Live {
    pub fn new() -> Live {
        let view = View::new().unwrap_or_else(|error| panic!("the view: {error}"));
        let (changes, _) = broadcast::channel(BACKLOG);
        Live { view: Arc::new(Mutex::new(view)), changes }
    }

    pub fn changes(&self) -> broadcast::Sender<Arc<Stream>> {
        self.changes.clone()
    }

    // Writes `response` and sends every frontend the bytes that appended. Returns whether any
    // did. The change is sent while the view is locked, so changes are sent in the order they
    // commit.
    pub fn publish(&self, response: &ListOpenPullRequestsResponse) -> bool {
        let mut view = self.view.lock().unwrap();
        match view.write(response) {
            Ok(Some(delta)) => {
                let stream = Stream { attributes: delta.attributes(), bytes: delta.bytes };
                // No receiver while no frontend is in the room.
                let _ = self.changes.send(Arc::new(stream));
                true
            }
            Ok(None) => false,
            Err(error) => {
                tracing::warn!(%error, "cannot write the view");
                false
            }
        }
    }

    // Answers a frontend's view-sync request with which bytes it is sent, and them. Its payload is
    // `null` for a new replica, or its `ReplicaState` as xitdb's serde writes it,
    // `{"committedSize":123,"checksum":"456"}`, the checksum a u64 as a decimal string.
    pub fn sync(&self, payload: &str) -> Result<(String, Stream), (u32, String)> {
        let replica: Option<ReplicaState> = serde_json::from_str(payload)
            .map_err(|error| (400, format!("not a replica's state: {error}")))?;
        let delta = (self.view.lock().unwrap().sync(replica))
            .map_err(|error| (500, format!("cannot read the view: {error}")))?;
        let attributes = delta.attributes();
        let reply = serde_json::to_string(&attributes).unwrap();
        Ok((reply, Stream { attributes, bytes: delta.bytes }))
    }
}

// A cluster keeps its key while pull requests join it and titles change: it is the URL of its
// oldest pull request, which `cluster` and `agents::join` put first. (A name's `names::key`
// changes with every title.) A pull request is in one cluster, so no two share a key.
fn key(cluster: &PullRequestCluster) -> &str {
    cluster.pull_requests.first().map_or("", |pr| pr.url.as_str())
}

fn write_view<C: Core>(
    moment: &mut WriteHashMap<'_, C>,
    response: &ListOpenPullRequestsResponse,
) -> Result<()> {
    let order: Vec<&str> = response.clusters.iter().map(key).collect();
    for (key, cluster) in order.iter().zip(&response.clusters) {
        // Writes only what changed in the cluster, and nothing when it didn't.
        CLUSTERS.put(moment, key, cluster)?;
    }
    let mut gone = Vec::new();
    if let Some(clusters) = CLUSTERS.collection(moment)? {
        for entry in clusters.entries()? {
            let key = entry?.key_cursor.read_bytes(None)?;
            if !order.iter().any(|shown| shown.as_bytes() == key) {
                gone.push(String::from_utf8_lossy(&key).into_owned());
            }
        }
    }
    for key in gone {
        CLUSTERS.remove(moment, &key)?;
    }
    put_structured(moment, ORDER, &order)?;
    put_structured(moment, LOGIN, &response.login)?;
    put_structured(moment, SEARCH, &response.search)?;
    put_structured(moment, AGENTS, &response.agents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use gem_db::xitdb::{ReadHashMap, get_structured};
    use pbjson_types::Timestamp;

    use super::*;
    use crate::proto::{
        Agent, AgentState, CheckFailure, Checks, FailedCheck, MergeBy, Proposal, PullRequest,
        Repository,
    };

    fn at(seconds: i64) -> Option<Timestamp> {
        Some(Timestamp { seconds, nanos: 0 })
    }

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
            created_at: at(1_790_000_000 + i64::from(n)),
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
            started_at: at(1_790_000_500),
            ..Default::default()
        };
        PullRequestCluster {
            pull_requests: (0..3).map(|n| pull_request(url, n)).collect(),
            updated_at: at(1_790_001_000),
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
                started_at: at(1_790_000_900),
                ..Default::default()
            }],
        }
    }

    // A replica as a frontend keeps it: it applies a delta that follows on from it, ignores one it
    // already has, and otherwise asks for a sync.
    struct Frontend {
        db: Database<CoreMemory>,
        file_id: Option<u64>,
    }

    impl Frontend {
        fn new() -> Frontend {
            Frontend { db: Database::in_memory().unwrap(), file_id: None }
        }

        // What it sends, through xitdb's serde form of its state.
        fn replica(&self) -> Option<ReplicaState> {
            self.file_id?;
            let json = serde_json::to_string(&self.db.replica_state().unwrap()).unwrap();
            Some(serde_json::from_str(&json).unwrap())
        }

        // Whether it applied the delta; a frontend that didn't syncs.
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
            let delta = view.sync(self.replica()).unwrap();
            assert!(self.receive(&delta));
            delta
        }

        // The response as the frontend rebuilds it.
        fn shown(&self) -> ListOpenPullRequestsResponse {
            let history = History::new(&self.db).unwrap();
            let moment: ReadHashMap<_> = history.latest().unwrap();
            let order: Vec<String> = get_structured(&moment, ORDER).unwrap().unwrap();
            let clusters = order.iter().map(|key| CLUSTERS.get(&moment, key).unwrap().unwrap());
            ListOpenPullRequestsResponse {
                clusters: clusters.collect(),
                login: get_structured(&moment, LOGIN).unwrap().unwrap(),
                search: get_structured(&moment, SEARCH).unwrap().unwrap(),
                agents: get_structured(&moment, AGENTS).unwrap().unwrap(),
            }
        }
    }

    #[test]
    fn an_unchanged_view_commits_nothing() {
        let mut view = View::new().unwrap();
        assert!(view.write(&response(5)).unwrap().is_some());
        let size = view.db.committed_size().unwrap();
        assert_eq!(view.write(&response(5)).unwrap(), None);
        assert_eq!(view.db.committed_size().unwrap(), size);
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
        let cluster = serde_json::to_vec(&shown.clusters[7]).unwrap().len();
        eprintln!(
            "whole view {} bytes, one cluster's JSON {cluster} bytes, delta {} bytes",
            whole.bytes.len(),
            delta.bytes.len()
        );
        assert!(delta.bytes.len() < 4096, "{} bytes", delta.bytes.len());
        assert!(delta.bytes.len() < cluster, "{} bytes", delta.bytes.len());
    }

    #[test]
    fn a_new_replica_is_sent_the_whole_view() {
        let mut view = View::new().unwrap();
        view.write(&response(3)).unwrap();
        let mut frontend = Frontend::new();
        let delta = frontend.sync(&view);
        assert_eq!((delta.start, delta.end), (0, view.db.committed_size().unwrap()));
        assert_eq!(frontend.shown(), response(3));
    }

    #[test]
    fn a_replica_applies_each_change() {
        let mut view = View::new().unwrap();
        view.write(&response(3)).unwrap();
        let mut frontend = Frontend::new();
        frontend.sync(&view);
        let mut shown = response(3);
        shown.clusters.remove(1);
        shown.clusters.push(cluster("https://github.com/o/r/pull/9/", "new"));
        shown.clusters[0].pull_requests[0].approved = false;
        shown.clusters[0].agents.clear();
        shown.login = "other".to_string();
        let delta = view.write(&shown).unwrap().unwrap();
        assert!(frontend.receive(&delta));
        assert_eq!(frontend.shown(), shown);
        // One it already has, from a sync that raced the change, is not applied twice.
        let synced = view.sync(frontend.replica()).unwrap();
        assert_eq!(synced.start, synced.end);
        assert!(!frontend.receive(&delta));
    }

    #[test]
    fn a_replica_that_missed_a_change_syncs_what_it_lacks() {
        let mut view = View::new().unwrap();
        view.write(&response(3)).unwrap();
        let mut frontend = Frontend::new();
        frontend.sync(&view);
        let before = frontend.db.committed_size().unwrap();
        view.write(&response(4)).unwrap().unwrap();
        let next = view.write(&response(5)).unwrap().unwrap();
        assert!(!frontend.receive(&next));
        let delta = frontend.sync(&view);
        assert_eq!(delta.start, before);
        assert_eq!(frontend.shown(), response(5));
    }

    #[test]
    fn a_compacted_view_replaces_every_replica() {
        let mut view = View::new().unwrap();
        view.write(&response(3)).unwrap();
        let mut frontend = Frontend::new();
        frontend.sync(&view);
        let file_id = view.db.header().file_id;
        // Growing past the threshold compacts it; each change is still sent from the old file.
        let mut shown = response(3);
        for i in 0..20_000 {
            shown.clusters[0].name = format!("name {i}");
            let delta = view.write(&shown).unwrap().unwrap();
            assert!(frontend.receive(&delta));
            if view.db.header().file_id != file_id {
                break;
            }
        }
        assert_ne!(view.db.header().file_id, file_id);
        assert!(view.db.committed_size().unwrap() < GROWTH);
        let delta = frontend.sync(&view);
        assert_eq!(delta.start, 0);
        assert_eq!(frontend.shown(), shown);
        // Later changes follow on from the compacted copy.
        shown.clusters[1].name = "after".to_string();
        assert!(frontend.receive(&view.write(&shown).unwrap().unwrap()));
        assert_eq!(frontend.shown(), shown);
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
            view.sync(None).unwrap()
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
        let fixture = serde_json::json!({
            "full": full.attributes(),
            "delta": delta.attributes(),
            "first": first,
            "second": second,
        });
        std::fs::write(dir.join("fixture.json"), fixture.to_string()).unwrap();
    }
}
