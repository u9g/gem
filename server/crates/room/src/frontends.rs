// What the server knows of each frontend's replica of the view, and what it has queued for each,
// to send each the bytes it lacks.
//
// The server drives syncing. Each frontend has one queue of byte streams, sent in order: the view
// once the frontend is active, every change as it commits, and what its reports show it lacks.
// After each stream, the frontend reports its replica's state, which acknowledges what was sent
// it. A report that's behind is usually behind only by what's still on its way, so what it lacks
// is queued at once only when nothing on its way would bring it up to date: no stream of what it
// lacks, which is read as it's sent and so brings any replica up to date, and, for a replica of
// this file, no change past it. Otherwise it's queued once the oldest stream the frontend hasn't
// acknowledged has had time to arrive, which also covers lost streams and lost reports.
//
// Deciding needs only where a replica is in the file: a trailer's read. The bytes are read by the
// frontend's queue as it sends them, off the server's event loop.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use gem_db::{Db, Delta, Replica};

// How long a frontend has to report a stream once it was sent, doubling with each resend in a row
// that didn't bring it up to date, up to MAX_WAIT, so a frontend that never catches up isn't sent
// copy after copy.
const WAIT: Duration = Duration::from_secs(2);
const MAX_WAIT: Duration = Duration::from_secs(60);
// A stream has been sent once LiveKit has taken its chunks, which can be well before a phone on a
// slow link has them all, so a frontend also has the time a stream takes to arrive at this rate,
// about a megabit a second: 13 seconds more for a 1.6 MB copy.
const BYTES_PER_SECOND: u64 = 128 << 10;

// A byte stream for a frontend's queue to send.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    // A change committed to the view.
    Change(Arc<Delta>),
    // What `replica` lacks, read as it is sent: all of the view for none, or for a copy of
    // another file. It is sent even when that's nothing, so a frontend learns it is up to date.
    Lacking(Option<Replica>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Queued {
    // Says which stream a send that finished was.
    pub seq: u64,
    pub out: Out,
}

#[derive(Default)]
pub struct Frontends {
    frontends: HashMap<String, Frontend>,
    seq: u64,
}

#[derive(Default)]
struct Frontend {
    // Its replica as it last reported it: None while it is empty, or before it reports.
    report: Option<Replica>,
    // Whether it can be sent data, which it can only once it is active.
    active: bool,
    // What was queued for it and not yet acknowledged by a report, oldest first.
    unacked: VecDeque<Sent>,
    // How many times in a row what it lacks was queued again because it didn't catch up.
    resends: u32,
}

struct Sent {
    seq: u64,
    // The file ID and size its replica has once it has applied the stream: known for a change,
    // and for what a replica lacked once it is read.
    target: Option<(u64, i64)>,
    // Whether it brings any replica up to date: what a replica lacks, read as it is sent.
    lacking: bool,
    // When it finished sending, and how long the frontend then has to report it.
    finished: Option<(Instant, Duration)>,
}

impl Frontend {
    fn queue(&mut self, seq: &mut u64, out: Out) -> Queued {
        *seq += 1;
        let (target, lacking) = match &out {
            Out::Change(delta) => (Some((delta.file_id, delta.end)), false),
            Out::Lacking(_) => (None, true),
        };
        self.unacked.push_back(Sent { seq: *seq, target, lacking, finished: None });
        Queued { seq: *seq, out }
    }

    // Whether the oldest stream it hasn't acknowledged has had time to arrive.
    fn late(&self, now: Instant) -> bool {
        let finished = self.unacked.front().and_then(|sent| sent.finished);
        finished.is_some_and(|(at, wait)| now >= at + wait)
    }

    // As if a stream finished sending now, so what it lacks is queued again after the wait: when
    // reading the view to decide failed.
    fn retry(&mut self, now: Instant) {
        let finished = Some((now, WAIT));
        self.unacked.push_front(Sent { seq: 0, target: None, lacking: true, finished });
    }
}

impl Frontends {
    // A frontend that became active is sent what it lacks: all of the view, unless it already
    // reported a replica. Nothing, if something it hasn't acknowledged is queued for it already.
    pub fn active(&mut self, id: &str) -> Option<Queued> {
        let Frontends { frontends, seq } = self;
        let frontend = frontends.entry(id.to_string()).or_default();
        frontend.active = true;
        if !frontend.unacked.is_empty() {
            return None;
        }
        let report = frontend.report;
        Some(frontend.queue(seq, Out::Lacking(report)))
    }

    // Notes the replica a frontend reported, and returns what to queue for it now, if anything.
    pub fn report(
        &mut self,
        id: &str,
        replica: Option<Replica>,
        view: &Db,
        now: Instant,
    ) -> Result<Option<Queued>, String> {
        let Frontends { frontends, seq } = self;
        // A frontend that reports before it is active, as when the server rejoins, is sent what
        // its report shows it lacks once it is.
        let frontend = frontends.entry(id.to_string()).or_default();
        frontend.report = replica;
        if !frontend.active {
            return Ok(None);
        }
        let lacks = match view.replicate(replica, false) {
            Ok(lacks) => lacks,
            Err(error) => {
                frontend.retry(now);
                return Err(error);
            }
        };
        if lacks.start == lacks.end {
            frontend.unacked.clear();
            frontend.resends = 0;
            return Ok(None);
        }
        if let Some(replica) = replica {
            frontend.unacked.retain(|sent| {
                let (file_id, size) = (replica.file_id, replica.state.committed_size);
                !sent.target.is_some_and(|(file, end)| file == file_id && size >= end)
            });
        }
        // A replica is part of this file up to its size unless it's sent all of the view, as one
        // of another file, or one whose checksum doesn't match this file's bytes, is.
        let part = lacks.start > 0;
        let coming = (frontend.unacked.iter()).any(|sent| {
            sent.lacking || part && sent.target.is_some_and(|(file, _)| file == lacks.file_id)
        });
        let late = frontend.late(now);
        if coming && !late {
            return Ok(None);
        }
        if late {
            frontend.resends += 1;
        }
        frontend.unacked.clear();
        Ok(Some(frontend.queue(seq, Out::Lacking(replica))))
    }

    // Notes that the stream `seq` queued for `id` was sent, to the file ID and size `sent`, the
    // size in bytes, or failed to be when None. A stream queued again since is waited for alone.
    pub fn finished(&mut self, id: &str, seq: u64, sent: Option<(u64, i64, usize)>, now: Instant) {
        let Some(frontend) = self.frontends.get_mut(id) else {
            return;
        };
        let wait = (WAIT * 2u32.saturating_pow(frontend.resends)).min(MAX_WAIT);
        let report = frontend.report;
        let Some(index) = frontend.unacked.iter().position(|sent| sent.seq == seq) else {
            return;
        };
        let stream = &mut frontend.unacked[index];
        let size = sent.map_or(0, |(_, _, size)| size as u64);
        let arriving = Duration::from_millis(size * 1000 / BYTES_PER_SECOND);
        stream.finished = Some((now, wait + arriving));
        stream.target = stream.target.or(sent.map(|(file_id, end, _)| (file_id, end)));
        // The frontend can report having it before the server hears that it finished.
        let had = report.is_some_and(|replica| {
            let (file_id, size) = (replica.file_id, replica.state.committed_size);
            stream.target.is_some_and(|(file, end)| file == file_id && size >= end)
        });
        if had {
            frontend.unacked.remove(index);
        }
    }

    // Queues `delta`, just committed, for every active frontend.
    pub fn changed(&mut self, delta: &Arc<Delta>) -> Vec<(String, Queued)> {
        let Frontends { frontends, seq } = self;
        (frontends.iter_mut().filter(|(_, frontend)| frontend.active))
            .map(|(id, frontend)| (id.clone(), frontend.queue(seq, Out::Change(delta.clone()))))
            .collect()
    }

    // Queues what each active frontend lacks, once changes were committed and never queued, as
    // when the server fell behind on them.
    pub fn lagged(&mut self) -> Vec<(String, Queued)> {
        let Frontends { frontends, seq } = self;
        let active = frontends.iter_mut().filter(|(_, frontend)| frontend.active);
        (active.map(|(id, frontend)| {
            frontend.unacked.clear();
            let report = frontend.report;
            (id.clone(), frontend.queue(seq, Out::Lacking(report)))
        }))
        .collect()
    }

    // What to queue for each frontend that hasn't caught up in time, and the errors reading the
    // view for any, which are each due again after the wait.
    pub fn due(&mut self, view: &Db, now: Instant) -> (Vec<(String, Queued)>, Vec<String>) {
        let Frontends { frontends, seq } = self;
        let (mut due, mut errors) = (Vec::new(), Vec::new());
        for (id, frontend) in frontends.iter_mut() {
            if !frontend.active || !frontend.late(now) {
                continue;
            }
            match view.replicate(frontend.report, false) {
                Err(error) => {
                    frontend.unacked.clear();
                    frontend.retry(now);
                    errors.push(error);
                }
                Ok(lacks) if lacks.start == lacks.end => {
                    frontend.unacked.clear();
                    frontend.resends = 0;
                }
                Ok(_) => {
                    frontend.resends += 1;
                    frontend.unacked.clear();
                    let report = frontend.report;
                    due.push((id.clone(), frontend.queue(seq, Out::Lacking(report))));
                }
            }
        }
        (due, errors)
    }

    pub fn left(&mut self, id: &str) {
        self.frontends.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use gem_db::xitdb::{CoreMemory, Database};

    use super::*;

    // A view with some moments in it.
    fn view() -> Db {
        let view = Db::open_in_memory().unwrap();
        for n in 0..3 {
            view.write(|moment| moment.put("n", &*n.to_string())).unwrap();
        }
        view
    }

    fn commit(view: &Db, value: &str) -> Arc<Delta> {
        Arc::new(view.write_replicated(|moment| moment.put("n", value)).unwrap().1.unwrap())
    }

    // A frontend: its replica, and its queue, which the test sends from.
    struct Replicated {
        db: Database<CoreMemory>,
        file_id: Option<u64>,
        queue: VecDeque<Queued>,
    }

    impl Replicated {
        fn new() -> Replicated {
            let db = Database::in_memory().unwrap();
            Replicated { db, file_id: None, queue: VecDeque::new() }
        }

        fn report(&self) -> Option<Replica> {
            let state = self.db.replica_state().unwrap();
            Some(Replica { file_id: self.file_id?, state })
        }

        // Sends the next stream in its queue, as its queue's task does, telling `frontends` it
        // finished at `now`, and returns it.
        fn send(&mut self, frontends: &mut Frontends, view: &Db, now: Instant) -> Arc<Delta> {
            let Queued { seq, out } = self.queue.pop_front().unwrap();
            let delta = match out {
                Out::Change(delta) => delta,
                Out::Lacking(replica) => Arc::new(view.replicate(replica, true).unwrap()),
            };
            let sent = Some((delta.file_id, delta.end, delta.bytes.len()));
            frontends.finished("f", seq, sent, now);
            delta
        }

        // Applies what it was sent, as the frontend does, and reports, returning what to queue.
        fn apply(&mut self, frontends: &mut Frontends, view: &Db, delta: &Delta, now: Instant) {
            let size = self.db.committed_size().unwrap();
            let same = self.file_id == Some(delta.file_id);
            if (same && delta.start <= size && delta.end > size) || (!same && delta.start == 0) {
                let skip = if same { size - delta.start } else { 0 };
                let start = delta.start + skip;
                let mut bytes = &delta.bytes[skip as usize..];
                self.db.apply_replicated(start, &mut bytes, delta.end - start).unwrap();
                self.file_id = Some(delta.file_id);
            }
            let queued = frontends.report("f", self.report(), view, now).unwrap();
            self.queue.extend(queued);
        }

        // Sends and applies everything queued.
        fn catch_up(&mut self, frontends: &mut Frontends, view: &Db, now: Instant) {
            while !self.queue.is_empty() {
                let delta = self.send(frontends, view, now);
                self.apply(frontends, view, &delta, now);
            }
        }

        fn current(&self, view: &Db) -> bool {
            view.replicate(self.report(), false).is_ok_and(|lacks| lacks.start == lacks.end)
        }
    }

    // A frontend that became active and caught up.
    fn caught_up(frontends: &mut Frontends, view: &Db, now: Instant) -> Replicated {
        let mut replica = Replicated::new();
        replica.queue.extend(frontends.active("f"));
        replica.catch_up(frontends, view, now);
        assert!(replica.current(view));
        replica
    }

    fn queue(frontends: &mut Frontends, replica: &mut Replicated, delta: &Arc<Delta>) {
        replica.queue.extend(frontends.changed(delta).into_iter().map(|(_, queued)| queued));
    }

    #[test]
    fn a_frontend_that_becomes_active_is_sent_the_whole_view() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let queued = frontends.active("f").unwrap();
        assert_eq!(queued.out, Out::Lacking(None));
        // Not again, as when it's active both when the server joins and in an event after.
        assert_eq!(frontends.active("f"), None);
        // Nor for its first report, from before the copy arrived.
        assert_eq!(frontends.report("f", None, &view, now).unwrap(), None);
        frontends.finished("f", queued.seq, Some((0, 1, 1)), now);
        assert_eq!(frontends.report("f", None, &view, now).unwrap(), None);
        assert_eq!(frontends.due(&view, now + WAIT / 2).0, []);
    }

    #[test]
    fn a_report_behind_with_nothing_on_its_way_is_sent_what_it_lacks() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let mut replica = caught_up(&mut frontends, &view, now);
        // A change never queued for it.
        commit(&view, "lost");
        let queued = frontends.report("f", replica.report(), &view, now).unwrap().unwrap();
        assert_eq!(queued.out, Out::Lacking(replica.report()));
        replica.queue.push_back(queued);
        let delta = replica.send(&mut frontends, &view, now);
        assert_ne!(delta.start, 0);
        replica.apply(&mut frontends, &view, &delta, now);
        assert!(replica.current(&view));
    }

    #[test]
    fn a_report_of_another_file_or_other_bytes_is_sent_the_whole_view() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let mut replica = caught_up(&mut frontends, &view, now);
        // A change on its way doesn't fix either.
        queue(&mut frontends, &mut replica, &commit(&view, "4"));
        let mut other = replica.report().unwrap();
        other.file_id += 1;
        let queued = frontends.report("f", Some(other), &view, now).unwrap().unwrap();
        assert_eq!(queued.out, Out::Lacking(Some(other)));
        let (mut frontends, now) = (Frontends::default(), now + WAIT);
        let mut replica = caught_up(&mut frontends, &view, now);
        queue(&mut frontends, &mut replica, &commit(&view, "5"));
        let mut mismatched = replica.report().unwrap();
        mismatched.state.checksum ^= 1;
        let queued = frontends.report("f", Some(mismatched), &view, now).unwrap().unwrap();
        let whole = view.replicate(Some(mismatched), false).unwrap();
        assert_eq!((queued.out, whole.start), (Out::Lacking(Some(mismatched)), 0));
    }

    #[test]
    fn a_report_behind_a_change_on_its_way_waits_for_it() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let mut replica = caught_up(&mut frontends, &view, now);
        let before = replica.report();
        queue(&mut frontends, &mut replica, &commit(&view, "4"));
        // Its report from before the change raced the change, which is still on its way.
        assert_eq!(frontends.report("f", before, &view, now).unwrap(), None);
        replica.catch_up(&mut frontends, &view, now);
        assert!(replica.current(&view));
        assert_eq!(frontends.due(&view, now + MAX_WAIT).0, []);
    }

    // The wait runs from the oldest stream not acknowledged, whatever is sent after it.
    #[test]
    fn a_lost_change_is_sent_again_while_changes_keep_coming() {
        let (view, mut frontends, start) = (view(), Frontends::default(), Instant::now());
        let mut replica = caught_up(&mut frontends, &view, start);
        queue(&mut frontends, &mut replica, &commit(&view, "lost"));
        replica.send(&mut frontends, &view, start);
        let mut resent = None;
        for n in 1..10 {
            let now = start + Duration::from_secs(n);
            let (due, _) = frontends.due(&view, now);
            if !due.is_empty() {
                resent = Some(now);
                break;
            }
            // A change a second, which doesn't follow on from the replica.
            queue(&mut frontends, &mut replica, &commit(&view, &n.to_string()));
            let delta = replica.send(&mut frontends, &view, now);
            replica.apply(&mut frontends, &view, &delta, now);
        }
        assert!(resent.is_some_and(|at| at <= start + WAIT + Duration::from_secs(1)), "{resent:?}");
    }

    #[test]
    fn changes_lagged_on_are_sent_to_every_frontend() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let mut replica = caught_up(&mut frontends, &view, now);
        // Changes committed and never queued, and nothing after them.
        commit(&view, "4");
        commit(&view, "5");
        replica.queue.extend(frontends.lagged().into_iter().map(|(_, queued)| queued));
        replica.catch_up(&mut frontends, &view, now);
        assert!(replica.current(&view));
    }

    #[test]
    fn a_change_after_a_whole_copy_follows_it_in_order() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let mut replica = Replicated::new();
        replica.queue.extend(frontends.active("f"));
        // The copy is read as it's sent, and a change commits after.
        let copy = replica.send(&mut frontends, &view, now);
        queue(&mut frontends, &mut replica, &commit(&view, "4"));
        replica.apply(&mut frontends, &view, &copy, now);
        let change = replica.send(&mut frontends, &view, now);
        assert_eq!(change.start, copy.end);
        replica.apply(&mut frontends, &view, &change, now);
        assert!(replica.current(&view) && replica.queue.is_empty());
        // A change committed after the copy was queued and before it was read is in it, and had.
        let (mut frontends, mut replica) = (Frontends::default(), Replicated::new());
        replica.queue.extend(frontends.active("f"));
        queue(&mut frontends, &mut replica, &commit(&view, "5"));
        replica.catch_up(&mut frontends, &view, now);
        assert!(replica.current(&view));
        assert_eq!(frontends.due(&view, now + MAX_WAIT).0, []);
    }

    #[test]
    fn a_frontend_up_to_date_when_the_server_rejoins_is_sent_no_copy() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let mut replica = caught_up(&mut frontends, &view, now);
        // The server rejoins: its frontends are new to it, and one reports before it's active.
        let mut frontends = Frontends::default();
        assert_eq!(frontends.report("f", replica.report(), &view, now).unwrap(), None);
        let queued = frontends.active("f").unwrap();
        assert_eq!(queued.out, Out::Lacking(replica.report()));
        replica.queue.push_back(queued);
        let sent = replica.send(&mut frontends, &view, now);
        assert!(sent.start > 0 && sent.bytes.is_empty());
    }

    #[test]
    fn a_send_queued_again_since_is_not_finished_by_the_one_before() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let mut replica = caught_up(&mut frontends, &view, now);
        queue(&mut frontends, &mut replica, &commit(&view, "lost"));
        let lost = replica.queue.pop_front().unwrap();
        frontends.finished("f", lost.seq, None, now);
        let (due, _) = frontends.due(&view, now + WAIT);
        assert_eq!(due.len(), 1);
        // The resend is still being sent when an old send's completion comes in late.
        frontends.finished("f", lost.seq, None, now + WAIT);
        assert_eq!(frontends.due(&view, now + MAX_WAIT).0, []);
    }

    #[test]
    fn a_big_copy_has_time_to_arrive() {
        let view = view();
        let big = "x".repeat(4 << 20);
        view.write(|moment| moment.put("big", &*big)).unwrap();
        let (mut frontends, now) = (Frontends::default(), Instant::now());
        let queued = frontends.active("f").unwrap();
        // Still being sent, however long that takes.
        assert_eq!(frontends.due(&view, now + MAX_WAIT).0, []);
        let whole = view.replicate(None, false).unwrap();
        frontends.finished("f", queued.seq, Some((whole.file_id, whole.end, 4 << 20)), now);
        assert_eq!(frontends.due(&view, now + WAIT).0, []);
        assert_eq!(frontends.due(&view, now + 10 * WAIT).0, []);
        let (due, _) = frontends.due(&view, now + WAIT + Duration::from_secs(33));
        assert_eq!(due.len(), 1);
    }

    #[test]
    fn a_whole_copy_never_reported_is_sent_again_and_again_less_often() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let queued = frontends.active("f").unwrap();
        frontends.finished("f", queued.seq, None, now);
        let (due, _) = frontends.due(&view, now + WAIT);
        assert_eq!(due[0].1.out, Out::Lacking(None));
        frontends.finished("f", due[0].1.seq, None, now + WAIT);
        assert_eq!(frontends.due(&view, now + 2 * WAIT).0, []);
        assert_eq!(frontends.due(&view, now + 3 * WAIT).0.len(), 1);
    }

    #[test]
    fn caught_up_the_wait_starts_over() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let mut replica = caught_up(&mut frontends, &view, now);
        for n in 0..3 {
            queue(&mut frontends, &mut replica, &commit(&view, &format!("lost {n}")));
            let lost = replica.queue.pop_front().unwrap();
            frontends.finished("f", lost.seq, None, now);
            let (due, _) = frontends.due(&view, now + MAX_WAIT);
            replica.queue.extend(due.into_iter().map(|(_, queued)| queued));
            replica.catch_up(&mut frontends, &view, now);
        }
        // Each lost change was sent again; caught up after each, the next waits only WAIT.
        queue(&mut frontends, &mut replica, &commit(&view, "lost again"));
        let lost = replica.queue.pop_front().unwrap();
        frontends.finished("f", lost.seq, None, now);
        assert_eq!(frontends.due(&view, now + WAIT).0.len(), 1);
    }

    #[test]
    fn a_failed_read_is_tried_again_after_the_wait() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let broken = Db::open_reader(std::env::temp_dir().join("gem-no-view.xitdb"), {
            gem_db::Migrations::new([]).unwrap()
        });
        let mut replicas = Vec::new();
        for id in ["f", "g"] {
            let queued = frontends.active(id).unwrap();
            frontends.finished(id, queued.seq, None, now);
            replicas.push(queued);
        }
        // Both fail, and neither is left waiting on nothing.
        let (due, errors) = frontends.due(&broken, now + WAIT);
        assert_eq!((due.len(), errors.len()), (0, 2));
        assert_eq!(frontends.due(&view, now + WAIT + WAIT / 2).0, []);
        assert_eq!(frontends.due(&view, now + 2 * WAIT).0.len(), 2);
        // As is a report that can't be read.
        let mut frontends = Frontends::default();
        frontends.active("f");
        assert!(frontends.report("f", None, &broken, now).is_err());
        assert_eq!(frontends.due(&view, now + WAIT).0.len(), 1);
    }

    #[test]
    fn a_frontend_that_leaves_is_forgotten() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        caught_up(&mut frontends, &view, now);
        frontends.left("f");
        assert_eq!(frontends.changed(&commit(&view, "4")), []);
        assert_eq!(frontends.due(&view, now + MAX_WAIT).0, []);
    }
}
