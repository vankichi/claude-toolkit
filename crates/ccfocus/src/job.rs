//! One-at-a-time background work for the board: the worktree scan, gh, and
//! a jump's tmux commands each run on a [`Job`] so a slow command never
//! freezes the screen.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// A background computation run at most every `every`, one at a time. The
/// thread ends when its work does (every command has a timeout); a dropped
/// [`Job`] lets a running thread finish and discards its result.
#[derive(Debug)]
pub struct Job<T> {
    every: Duration,
    started: Option<Instant>,
    running: Option<Receiver<T>>,
}

impl<T: Send + 'static> Job<T> {
    #[must_use]
    pub fn new(every: Duration) -> Self {
        Self {
            every,
            started: None,
            running: None,
        }
    }

    /// Start `work` unless one is running or the last start was less than
    /// `every` ago (`force` skips the wait, not the one-at-a-time rule).
    /// `true` when it started.
    pub fn kick(
        &mut self,
        now: Instant,
        force: bool,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> bool {
        let due = force
            || self
                .started
                .is_none_or(|t| now.duration_since(t) >= self.every);
        if self.running.is_some() || !due {
            return false;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        self.started = Some(now);
        self.running = Some(rx);
        true
    }

    /// Work was started and its result hasn't been polled yet (it may have
    /// finished).
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.running.is_some()
    }

    /// Block until the running work is done; its result, if it sent one.
    pub fn wait(&mut self) -> Option<T> {
        self.running.take()?.recv().ok()
    }

    /// The finished result, if the running work is done.
    pub fn poll(&mut self) -> Option<T> {
        let rx = self.running.as_ref()?;
        match rx.try_recv() {
            Ok(v) => {
                self.running = None;
                Some(v)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.running = None;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_runs_one_at_a_time_and_waits_between_runs() {
        let mut job: Job<u32> = Job::new(Duration::from_mins(10));
        let t0 = Instant::now();
        assert!(job.kick(t0, false, || 1));
        assert!(!job.kick(t0, true, || 2), "still running (or unpolled)");
        let mut got = None;
        for _ in 0..200 {
            got = job.poll();
            if got.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got, Some(1));
        job.kick(t0 + Duration::from_secs(1), false, || 3);
        assert!(job.running.is_none(), "not due yet");
        job.kick(t0 + Duration::from_secs(1), true, || 4);
        assert!(job.running.is_some());
    }
}
