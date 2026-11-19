//! Bookkeeping for a restoring paste: the snapshot taken before the transient
//! write is put back after the paste, unless someone copied in between. A quit
//! waits for every paste between its snapshot and its scheduled restore, and
//! for every scheduled restore that has not run yet.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::pasteboard::{Pasteboard, RestoreOutcome, Snapshot, WriteFailure, SNAPSHOT_LIMIT_BYTES};

/// How long the target app gets to read the pasted transcript before the
/// previous clipboard contents return.
const RESTORE_DELAY: Duration = Duration::from_millis(500);

/// The queue for the general pasteboard.
pub(super) static RESTORES: RestoreQueue = RestoreQueue::new(RESTORE_DELAY);

/// Block until every restoring paste in flight has scheduled or abandoned its
/// restore, and every scheduled restore has run, or `timeout` passes. Returns
/// at once when there is none.
pub fn wait_for_pending_restore(timeout: Duration) {
    RESTORES.wait(timeout);
}

struct PendingRestore {
    generation: u64,
    /// Lets the restore thread look the pasteboard up again.
    board: String,
    /// Change count right after our transient write.
    change_count: isize,
    snapshot: Snapshot,
}

struct RestoreSlot {
    /// Pastes between their snapshot and scheduling or abandoning the restore.
    in_flight: usize,
    pending: Option<PendingRestore>,
    next_generation: u64,
}

/// Restores for one pasteboard. A paste that starts while a restore is
/// pending takes that restore over.
pub(super) struct RestoreQueue {
    slot: Mutex<RestoreSlot>,
    finished: Condvar,
    delay: Duration,
}

/// Counts a paste as in flight until it is dropped.
struct InFlight(&'static RestoreQueue);

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut slot = self.0.lock();
        slot.in_flight -= 1;
        drop(slot);
        self.0.finished.notify_all();
    }
}

/// A restoring paste whose transient write succeeded. Dropping it without
/// [`RestoreTicket::schedule`] abandons the restore, so the transcript stays
/// on the clipboard.
pub(super) struct RestoreTicket {
    in_flight: InFlight,
    board: String,
    change_count: isize,
    snapshot: Snapshot,
}

impl RestoreTicket {
    /// Puts the saved contents back after the queue's delay. The paste counts
    /// as in flight until its restore is pending, so a quit always sees one.
    pub(super) fn schedule(self) {
        let queue = self.in_flight.0;
        queue.schedule(self.board, self.change_count, self.snapshot);
    }
}

impl RestoreQueue {
    pub(super) const fn new(delay: Duration) -> Self {
        Self {
            slot: Mutex::new(RestoreSlot { in_flight: 0, pending: None, next_generation: 0 }),
            finished: Condvar::new(),
            delay,
        }
    }

    fn lock(&self) -> MutexGuard<'_, RestoreSlot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Saves the contents of `board` and writes `text` over them marked and
    /// local-only. `Ok(None)` when the contents cannot be saved; the caller
    /// then writes the transcript as a normal copy.
    ///
    /// A restore still pending means `board` may hold the previous transient
    /// transcript. While it does, that restore's snapshot is the user's
    /// clipboard and carries over to this paste.
    pub(super) fn begin(&'static self, board: &Pasteboard, text: &str) -> Result<Option<RestoreTicket>, String> {
        let Some(board_name) = board.name() else { return Ok(None) };
        let carried = {
            let mut slot = self.lock();
            slot.in_flight += 1;
            slot.pending.take().filter(|pending| pending.change_count == board.change_count())
        };
        let in_flight = InFlight(self);
        let (snapshot, carried_change_count) = match carried {
            Some(pending) => (pending.snapshot, Some(pending.change_count)),
            None => match board.snapshot(SNAPSHOT_LIMIT_BYTES) {
                Some(snapshot) => (snapshot, None),
                None => return Ok(None),
            },
        };
        match board.write_transient_text(text) {
            Ok(change_count) => Ok(Some(RestoreTicket { in_flight, board: board_name, change_count, snapshot })),
            Err(WriteFailure::Unchanged) => {
                // The previous transcript is still there; its restore goes back on schedule.
                if let Some(change_count) = carried_change_count {
                    self.schedule(board_name, change_count, snapshot);
                }
                Err("Failed to write to clipboard".into())
            }
            // The caller may copy the transcript next, so the error describes
            // only this write; the log records what the clipboard holds.
            Err(WriteFailure::Cleared) => match board.write_snapshot(&snapshot) {
                Ok(()) => {
                    log::warn!("Clipboard write failed after clearing it; previous contents put back");
                    Err("Failed to write to clipboard".into())
                }
                Err(_) => {
                    log::warn!("Clipboard write failed after clearing it; previous contents lost");
                    Err("Failed to write to clipboard, and its previous contents were lost.".into())
                }
            },
        }
    }

    fn schedule(&'static self, board: String, change_count: isize, snapshot: Snapshot) {
        let generation = {
            let mut slot = self.lock();
            slot.next_generation += 1;
            let generation = slot.next_generation;
            slot.pending = Some(PendingRestore { generation, board, change_count, snapshot });
            generation
        };
        let spawned = std::thread::Builder::new()
            .name("clipboard-restore".into())
            .spawn(move || {
                std::thread::sleep(self.delay);
                self.run(generation);
            });
        if let Err(error) = spawned {
            log::warn!("Could not schedule the clipboard restore: {error}");
            let mut slot = self.lock();
            if slot.pending.as_ref().is_some_and(|pending| pending.generation == generation) {
                slot.pending = None;
            }
            drop(slot);
            self.finished.notify_all();
        }
    }

    /// Runs under the slot lock, so a paste starting now waits and then saves
    /// the restored contents.
    fn run(&self, generation: u64) {
        let mut slot = self.lock();
        let pending = match slot.pending.take() {
            Some(pending) if pending.generation == generation => Some(pending),
            // A later paste took this restore over before it ran.
            other => {
                slot.pending = other;
                None
            }
        };
        if let Some(pending) = pending {
            let outcome = Pasteboard::named(&pending.board)
                .map(|board| board.restore_if_unchanged(pending.change_count, &pending.snapshot));
            match outcome {
                Some(RestoreOutcome::Restored) => log::info!("Clipboard restored to previous contents"),
                Some(RestoreOutcome::Changed) => log::info!("Clipboard changed after the paste, skipping restore"),
                Some(RestoreOutcome::Failed(WriteFailure::Unchanged)) | None => {
                    log::warn!("Clipboard restore failed; the transcription stays on the clipboard")
                }
                Some(RestoreOutcome::Failed(WriteFailure::Cleared)) => {
                    log::warn!("Clipboard restore failed after clearing the clipboard; it is now empty")
                }
            }
        }
        drop(slot);
        self.finished.notify_all();
    }

    /// `true` when nothing is left to wait for.
    fn wait(&self, timeout: Duration) -> bool {
        let (_slot, wait) = self
            .finished
            .wait_timeout_while(self.lock(), timeout, |slot| slot.in_flight > 0 || slot.pending.is_some())
            .unwrap_or_else(PoisonError::into_inner);
        if wait.timed_out() {
            log::warn!("Clipboard restore did not finish in time; the transcription stays on the clipboard");
        }
        !wait.timed_out()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paste::pasteboard::test_support::{owned, UniquePasteboard};
    use std::time::Instant;

    const ORIGINAL: &[&[(&str, &[u8])]] = &[
        &[("public.utf8-plain-text", b"Copied before dictation"), ("public.html", b"<i>Copied</i> before dictation")],
        &[("com.sottoasr.test.binary", &[0, 255, 7])],
    ];

    #[test]
    fn second_restoring_paste_in_the_restore_window_ends_with_the_original_clipboard() {
        let pasteboard = UniquePasteboard::new();
        pasteboard.write(ORIGINAL);
        static QUEUE: RestoreQueue = RestoreQueue::new(Duration::from_millis(150));
        let queue = &QUEUE;

        let first = queue.begin(&pasteboard.board, "First dictation").unwrap().expect("first snapshot");
        first.schedule();
        let second = queue.begin(&pasteboard.board, "Second dictation").unwrap().expect("second snapshot");
        // The first restore's snapshot carries over, not the first transcript.
        assert_eq!(second.snapshot, owned(ORIGINAL));
        second.schedule();

        assert!(queue.wait(Duration::from_secs(2)));
        assert_eq!(pasteboard.board.snapshot(SNAPSHOT_LIMIT_BYTES).unwrap(), owned(ORIGINAL));
    }

    #[test]
    fn wait_returns_at_once_when_no_restore_is_pending() {
        static QUEUE: RestoreQueue = RestoreQueue::new(Duration::from_millis(20));
        let queue = &QUEUE;
        let started = Instant::now();
        assert!(queue.wait(Duration::from_secs(2)));
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn wait_covers_a_paste_in_flight_and_its_scheduled_restore() {
        let pasteboard = UniquePasteboard::new();
        pasteboard.write(ORIGINAL);
        static QUEUE: RestoreQueue = RestoreQueue::new(Duration::from_millis(20));
        let queue = &QUEUE;
        let ticket = queue.begin(&pasteboard.board, "Dictated text").unwrap().expect("snapshot");

        // The paste worker is still activating the target and posting Cmd+V.
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            ticket.schedule();
        });

        assert!(queue.wait(Duration::from_secs(2)));
        assert_eq!(pasteboard.board.snapshot(SNAPSHOT_LIMIT_BYTES).unwrap(), owned(ORIGINAL));
        worker.join().unwrap();
    }

    #[test]
    fn abandoned_paste_keeps_the_transcript_and_releases_the_wait() {
        let pasteboard = UniquePasteboard::new();
        pasteboard.write(ORIGINAL);
        static QUEUE: RestoreQueue = RestoreQueue::new(Duration::from_millis(20));
        let queue = &QUEUE;
        let ticket = queue.begin(&pasteboard.board, "Dictated text").unwrap().expect("snapshot");

        // Activation failed: the transcript must stay for recovery.
        drop(ticket);

        let started = Instant::now();
        assert!(queue.wait(Duration::from_secs(2)));
        assert!(started.elapsed() < Duration::from_millis(100));
        let current = pasteboard.board.snapshot(SNAPSHOT_LIMIT_BYTES).unwrap();
        assert_eq!(current[0][0], ("public.utf8-plain-text".to_string(), b"Dictated text".to_vec()));
    }
}
