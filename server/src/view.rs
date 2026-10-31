// What frontends show, in an xitdb database of its own that each frontend keeps a live replica
// of. gem.xitdb holds agents' bearer tokens, so it is never replicated; this one holds only what
// frontends render, in memory, and is made again when the server starts.
//
// Its root is a `History` like gem.xitdb's. Each snapshot is written as one moment, and an
// unchanged snapshot commits nothing. Since the file only grows, the bytes a moment appends are
// all a replica that has the moments before it needs: a frontend is sent the whole database once,
// and then each moment's bytes as it commits.
//
// A moment holds:
// - `clusters`: each cluster's JSON, by `key`;
// - `order`: the clusters' keys, as a JSON array, in the order they show;
// - `login`, `search` and `agents`, the rest of the `ListOpenPullRequestsResponse`, `agents` as a
//   JSON array.

use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
};

use gem_db::xitdb::{
    Core, CoreMemory, Data, Database, History, JSON_FORMAT_TAG, Json, Map, MemoryOffsetMap,
    ReplicaState, Result, WriteHashMap,
};
use gem_room::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::proto::{ListOpenPullRequestsResponse, PullRequestCluster};

const CLUSTERS: Map<str, Json<PullRequestCluster>> = Map::new("clusters");
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

// What a frontend's replica has, as it sends it: `null` for none yet, and otherwise its committed
// size and its checksum, a u64 as a decimal string.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Replica {
    pub committed_size: i64,
    pub checksum: String,
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
    pub fn sync(&self, replica: Option<&Replica>) -> Result<Delta> {
        let start = match replica {
            None => 0,
            Some(replica) => {
                let Ok(checksum) = replica.checksum.parse() else {
                    return self.read(0, self.db.committed_size()?);
                };
                let committed_size = replica.committed_size;
                self.db.replication_start(ReplicaState { committed_size, checksum })?
            }
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

    // Answers a frontend's view-sync request, whose payload is its `Replica` or `null`, with
    // which bytes it is sent, and them.
    pub fn sync(&self, payload: &str) -> Result<(String, Stream), (u32, String)> {
        let replica: Option<Replica> = serde_json::from_str(payload)
            .map_err(|error| (400, format!("not a replica's state: {error}")))?;
        let delta = (self.view.lock().unwrap().sync(replica.as_ref()))
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
        write_cluster(moment, key, cluster)?;
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
    put_json(moment, ORDER, &order)?;
    moment.put_if_changed(LOGIN, response.login.as_str())?;
    moment.put_if_changed(SEARCH, response.search.as_str())?;
    put_json(moment, AGENTS, &response.agents)?;
    Ok(())
}

// Writes the cluster at `key` only when its JSON changed, so a moment appends only the clusters
// that did. The JSON is what `Json<PullRequestCluster>` writes; a structural codec that writes only
// the fields that changed can replace it here.
fn write_cluster<C: Core>(
    moment: &mut WriteHashMap<'_, C>,
    key: &str,
    cluster: &PullRequestCluster,
) -> Result<bool> {
    let json = serde_json::to_vec(cluster).map_err(io::Error::from)?;
    let data = Data::tagged_bytes(json, JSON_FORMAT_TAG);
    moment.put_path_if_changed(&[CLUSTERS.name(), key], data)
}

fn put_json<C: Core>(
    moment: &mut WriteHashMap<'_, C>,
    key: &str,
    value: &impl Serialize,
) -> Result<bool> {
    let json = serde_json::to_vec(value).map_err(io::Error::from)?;
    moment.put_if_changed(key, Data::tagged_bytes(json, JSON_FORMAT_TAG))
}

#[cfg(test)]
mod tests {
    use gem_db::xitdb::ReadHashMap;

    use super::*;
    use crate::proto::PullRequest;

    fn cluster(url: &str, title: &str) -> PullRequestCluster {
        let pr = |n: u32| PullRequest {
            url: format!("{url}{n}"),
            title: format!("{title} {n}"),
            // Like a real pull request's description, the bulk of a cluster's JSON.
            no_proposal_reason: "a reason that takes some room to explain. ".repeat(20),
            ..Default::default()
        };
        PullRequestCluster {
            pull_requests: (0..3).map(pr).collect(),
            name: title.to_string(),
            ..Default::default()
        }
    }

    fn response(clusters: usize) -> ListOpenPullRequestsResponse {
        ListOpenPullRequestsResponse {
            clusters: (0..clusters)
                .map(|i| cluster(&format!("https://github.com/o/r/pull/{i}/"), "title"))
                .collect(),
            login: "u9g".to_string(),
            search: "org:livekit".to_string(),
            agents: Vec::new(),
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

        fn replica(&self) -> Option<Replica> {
            self.file_id?;
            let state = self.db.replica_state().unwrap();
            let checksum = state.checksum.to_string();
            Some(Replica { committed_size: state.committed_size, checksum })
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
            let delta = view.sync(self.replica().as_ref()).unwrap();
            assert!(self.receive(&delta));
            delta
        }

        // The response as the frontend rebuilds it.
        fn shown(&self) -> ListOpenPullRequestsResponse {
            let history = History::new(&self.db).unwrap();
            let moment: ReadHashMap<_> = history.latest().unwrap();
            let json = |key: &str| {
                let cursor = moment.get_cursor(key).unwrap().unwrap();
                serde_json::from_slice::<serde_json::Value>(&cursor.read_bytes(None).unwrap())
                    .unwrap()
            };
            let order: Vec<String> = serde_json::from_value(json(ORDER)).unwrap();
            let clusters = order.iter().map(|key| CLUSTERS.get(&moment, key).unwrap().unwrap());
            ListOpenPullRequestsResponse {
                clusters: clusters.collect(),
                login: moment.get_string(LOGIN, None).unwrap().unwrap(),
                search: moment.get_string(SEARCH, None).unwrap().unwrap(),
                agents: serde_json::from_value(json(AGENTS)).unwrap(),
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
    fn a_changed_cluster_appends_little_more_than_itself() {
        let mut view = View::new().unwrap();
        let mut shown = response(50);
        let whole = view.write(&shown).unwrap().unwrap();
        shown.clusters[7].name = "renamed".to_string();
        let delta = view.write(&shown).unwrap().unwrap();
        let one = serde_json::to_vec(&shown.clusters[7]).unwrap().len();
        assert!(delta.bytes.len() < whole.bytes.len() / 10, "{} bytes", delta.bytes.len());
        assert!(delta.bytes.len() < 2 * one + 4096, "{} bytes", delta.bytes.len());
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
        shown.login = "other".to_string();
        let delta = view.write(&shown).unwrap().unwrap();
        assert!(frontend.receive(&delta));
        assert_eq!(frontend.shown(), shown);
        // One it already has, from a sync that raced the change, is not applied twice.
        let synced = view.sync(frontend.replica().as_ref()).unwrap();
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
        for i in 0..2000 {
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
    // saying which bytes each is and what the frontend should show after each.
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
