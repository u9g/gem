// What the server knows of each frontend's replica of the view, to send each the bytes it lacks.
//
// The server drives syncing: it sends a frontend a whole copy once it is active, and every change
// as it commits, and the frontend reports its replica's state after each stream it is sent. A
// report that's behind is usually behind only by what's still on its way, so the bytes it lacks
// are sent at once only when nothing is: when it reports a copy of another file, or one that
// doesn't match this file's bytes, with no whole copy on its way, or reports having all it was
// sent and still lacks some. Otherwise they're sent once it hasn't caught up for `wait` after the
// last bytes it awaited were sent, which also covers lost reports, and the wait doubles each time
// up to MAX_WAIT, so a frontend that never catches up isn't sent copy after copy.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use gem_db::{Db, Delta, Replica};

// How long a frontend has to catch up after the bytes it awaits were sent.
pub const WAIT: Duration = Duration::from_secs(2);
const MAX_WAIT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct Frontends(HashMap<String, Frontend>);

struct Frontend {
    // Its replica as it last reported it: None while it is empty, or before it reports.
    replica: Option<Replica>,
    // Where its replica will be once it has applied everything sent to it: the file ID and size.
    expect: Option<(u64, i64)>,
    // Whether what it was sent starts a new replica, of the file `expect` names, which it hasn't
    // reported having yet.
    whole: bool,
    // When the bytes it awaits were sent: None while they are being sent, or when it awaits none.
    sent_at: Option<Instant>,
    sending: bool,
    wait: Duration,
}

impl Frontends {
    // A frontend that became active is sent what it lacks: the whole view, unless it already
    // reported a replica.
    pub fn active(&mut self, id: &str, view: &Db) -> Result<Option<Delta>, String> {
        let frontend = self.0.entry(id.to_string()).or_insert(Frontend {
            replica: None,
            expect: None,
            whole: false,
            sent_at: None,
            sending: false,
            wait: WAIT,
        });
        let delta = view.replicate(frontend.replica)?;
        if delta.bytes.is_empty() || frontend.sending {
            return Ok(None);
        }
        frontend.expect = Some((delta.file_id, delta.end));
        frontend.whole = delta.start == 0;
        frontend.sending = true;
        Ok(Some(delta))
    }

    // Notes the replica a frontend reported, and returns the bytes to send it now, if any.
    pub fn report(
        &mut self,
        id: &str,
        replica: Option<Replica>,
        view: &Db,
        now: Instant,
    ) -> Result<Option<Delta>, String> {
        // One that reports before it is active is sent the whole view once it is.
        let Some(frontend) = self.0.get_mut(id) else {
            return Ok(None);
        };
        frontend.replica = replica;
        if let (Some((file_id, _)), Some(replica)) = (frontend.expect, replica) {
            frontend.whole &= replica.file_id != file_id;
        }
        let delta = view.replicate(replica)?;
        if delta.bytes.is_empty() {
            frontend.sent_at = None;
            frontend.wait = WAIT;
            return Ok(None);
        }
        // Bytes of the file it has are on their way, or a whole copy of another.
        let awaiting = match (frontend.expect, replica) {
            (Some((file_id, end)), Some(replica)) if replica.file_id == file_id => {
                replica.state.committed_size < end
            }
            _ => frontend.whole,
        };
        let late = frontend.sent_at.is_some_and(|at| now >= at + frontend.wait);
        if frontend.sending || (awaiting && !late) {
            return Ok(None);
        }
        if awaiting {
            frontend.wait = (2 * frontend.wait).min(MAX_WAIT);
        }
        frontend.expect = Some((delta.file_id, delta.end));
        frontend.whole = delta.start == 0;
        frontend.sending = true;
        Ok(Some(delta))
    }

    // Notes that the bytes sent to `id` alone were sent, or failed to be.
    pub fn sent(&mut self, id: &str, now: Instant) {
        if let Some(frontend) = self.0.get_mut(id) {
            frontend.sending = false;
            frontend.sent_at = Some(now);
        }
    }

    // Notes that `delta` was sent to every frontend: the ones it follows on for await it.
    pub fn changed(&mut self, delta: &Delta, now: Instant) {
        for frontend in self.0.values_mut() {
            if frontend.expect == Some((delta.file_id, delta.start)) {
                frontend.expect = Some((delta.file_id, delta.end));
                if !frontend.sending {
                    frontend.sent_at = Some(now);
                }
            }
        }
    }

    // The bytes to send each frontend that hasn't caught up in time, by its ID.
    pub fn due(&mut self, view: &Db, now: Instant) -> Result<Vec<(String, Delta)>, String> {
        let mut due = Vec::new();
        for (id, frontend) in &mut self.0 {
            if frontend.sending || frontend.sent_at.is_none_or(|at| now < at + frontend.wait) {
                continue;
            }
            let delta = view.replicate(frontend.replica)?;
            if delta.bytes.is_empty() {
                frontend.sent_at = None;
                continue;
            }
            frontend.wait = (2 * frontend.wait).min(MAX_WAIT);
            frontend.expect = Some((delta.file_id, delta.end));
            frontend.whole = delta.start == 0;
            frontend.sending = true;
            due.push((id.clone(), delta));
        }
        Ok(due)
    }

    pub fn left(&mut self, id: &str) {
        self.0.remove(id);
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

    fn commit(view: &Db, value: &str) -> Delta {
        view.write_replicated(|moment| moment.put("n", value)).unwrap().1.unwrap()
    }

    // A frontend's replica, applying what it is sent.
    struct Replicated {
        db: Database<CoreMemory>,
        file_id: u64,
    }

    impl Replicated {
        fn new(whole: &Delta) -> Replicated {
            let replica = Replicated { db: Database::in_memory().unwrap(), file_id: whole.file_id };
            replica.apply(whole);
            replica
        }

        fn apply(&self, delta: &Delta) {
            let length = delta.end - delta.start;
            self.db.apply_replicated(delta.start, &mut delta.bytes.as_slice(), length).unwrap();
        }

        fn report(&self) -> Option<Replica> {
            let state = self.db.replica_state().unwrap();
            Some(Replica { file_id: self.file_id, state })
        }
    }

    // A frontend that became active and applied the whole copy it was sent, at `now`.
    fn caught_up(frontends: &mut Frontends, view: &Db, now: Instant) -> Replicated {
        let whole = frontends.active("f", view).unwrap().unwrap();
        frontends.sent("f", now);
        let replica = Replicated::new(&whole);
        assert_eq!(frontends.report("f", replica.report(), view, now).unwrap(), None);
        replica
    }

    #[test]
    fn a_frontend_that_becomes_active_is_sent_the_whole_view() {
        let (view, mut frontends) = (view(), Frontends::default());
        let whole = frontends.active("f", &view).unwrap().unwrap();
        assert_eq!(whole, view.replicate(None).unwrap());
        // Its first report, from before the copy arrived, isn't sent it again.
        let now = Instant::now();
        frontends.sent("f", now);
        assert_eq!(frontends.report("f", None, &view, now).unwrap(), None);
        assert!(frontends.due(&view, now + WAIT / 2).unwrap().is_empty());
    }

    #[test]
    fn a_report_behind_with_nothing_on_its_way_is_sent_the_bytes_it_lacks() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let replica = caught_up(&mut frontends, &view, now);
        let before = replica.report();
        // A change it is never sent, as when the server's broadcast lagged.
        let lost = commit(&view, "lost");
        let sent = frontends.report("f", before, &view, now).unwrap().unwrap();
        assert_eq!(sent, lost);
    }

    #[test]
    fn a_report_of_another_file_is_sent_the_whole_view() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let replica = caught_up(&mut frontends, &view, now);
        let mut other = replica.report().unwrap();
        other.file_id += 1;
        let sent = frontends.report("f", Some(other), &view, now).unwrap().unwrap();
        assert_eq!(sent, view.replicate(None).unwrap());
    }

    #[test]
    fn a_report_behind_a_change_on_its_way_waits_for_it() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let replica = caught_up(&mut frontends, &view, now);
        let before = replica.report();
        let change = commit(&view, "4");
        frontends.changed(&change, now);
        // Its report from before the change raced the change, which is still on its way.
        assert_eq!(frontends.report("f", before, &view, now).unwrap(), None);
        assert!(frontends.due(&view, now + WAIT / 2).unwrap().is_empty());
        replica.apply(&change);
        assert_eq!(frontends.report("f", replica.report(), &view, now).unwrap(), None);
        assert!(frontends.due(&view, now + 2 * WAIT).unwrap().is_empty());
    }

    #[test]
    fn a_change_never_reported_is_sent_again_after_the_wait() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let replica = caught_up(&mut frontends, &view, now);
        let change = commit(&view, "4");
        frontends.changed(&change, now);
        // The change, or the report of it, is lost.
        assert!(frontends.due(&view, now + WAIT / 2).unwrap().is_empty());
        let due = frontends.due(&view, now + WAIT).unwrap();
        assert_eq!(due, [("f".to_string(), change.clone())]);
        // Lost again, it is sent again after twice as long.
        frontends.sent("f", now + WAIT);
        assert!(frontends.due(&view, now + 2 * WAIT).unwrap().is_empty());
        assert_eq!(frontends.due(&view, now + 3 * WAIT).unwrap().len(), 1);
        frontends.sent("f", now + 3 * WAIT);
        replica.apply(&change);
        frontends.report("f", replica.report(), &view, now + 3 * WAIT).unwrap();
        assert!(frontends.due(&view, now + 10 * WAIT).unwrap().is_empty());
    }

    #[test]
    fn a_whole_copy_never_reported_is_sent_again_after_the_wait() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        let whole = frontends.active("f", &view).unwrap().unwrap();
        // Still being sent, however long that takes.
        assert!(frontends.due(&view, now + 10 * WAIT).unwrap().is_empty());
        frontends.sent("f", now);
        assert!(frontends.due(&view, now + WAIT / 2).unwrap().is_empty());
        assert_eq!(frontends.due(&view, now + WAIT).unwrap(), [("f".to_string(), whole)]);
    }

    #[test]
    fn a_frontend_that_leaves_is_forgotten() {
        let (view, mut frontends, now) = (view(), Frontends::default(), Instant::now());
        caught_up(&mut frontends, &view, now);
        frontends.changed(&commit(&view, "4"), now);
        frontends.left("f");
        assert!(frontends.due(&view, now + 10 * WAIT).unwrap().is_empty());
        assert_eq!(frontends.report("f", None, &view, now).unwrap(), None);
    }
}
