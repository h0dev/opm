//! The lock-free snapshot of the processes a daemon exit must signal.

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

const MAX_TRACKED_PIDS: usize = 128;

/// Fixed slots so the fallback can walk them without allocating. `0` marks an
/// empty slot.
pub struct PidSnapshot {
    targets: [AtomicI64; MAX_TRACKED_PIDS],
    target_count: AtomicUsize,
    own_pid: AtomicI64,
    own_session: AtomicI64,
}

impl PidSnapshot {
    pub const fn new() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const EMPTY: AtomicI64 = AtomicI64::new(0);
        Self {
            targets: [EMPTY; MAX_TRACKED_PIDS],
            target_count: AtomicUsize::new(0),
            own_pid: AtomicI64::new(0),
            own_session: AtomicI64::new(0),
        }
    }

    /// Install a snapshot; `session` is the daemon's own session, which must
    /// never be signalled because `opm` CLI invocations share it.
    pub fn store(&self, targets: &[i64], session: Option<i64>) {
        let own_pid = std::process::id() as i64;
        self.own_pid.store(own_pid, Ordering::SeqCst);
        self.own_session.store(session.unwrap_or(0), Ordering::SeqCst);

        let mut written = 0;
        for &pid in targets {
            if written == MAX_TRACKED_PIDS {
                break;
            }
            if pid > 0 && pid != own_pid && Some(pid) != session {
                self.targets[written].store(pid, Ordering::SeqCst);
                written += 1;
            }
        }
        for slot in self.targets.iter().skip(written) {
            slot.store(0, Ordering::SeqCst);
        }
        self.target_count.store(written, Ordering::SeqCst);
    }

    pub fn snapshot(&self) -> Vec<i64> {
        (0..self.target_count.load(Ordering::SeqCst).min(MAX_TRACKED_PIDS))
            .map(|index| self.targets[index].load(Ordering::SeqCst))
            .filter(|&pid| pid > 0)
            .collect()
    }

    /// SIGTERM every slot, skipping the daemon itself. Async-signal-safe: one
    /// load and one `kill` per slot.
    pub fn signal_all(&self) -> i32 {
        let own_pid = self.own_pid.load(Ordering::SeqCst);
        let own_session = self.own_session.load(Ordering::SeqCst);
        let mut signalled = 0;

        for index in 0..self.target_count.load(Ordering::SeqCst).min(MAX_TRACKED_PIDS) {
            let pid = self.targets[index].load(Ordering::SeqCst);
            if pid > 0 && pid != own_pid && pid != own_session {
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGTERM);
                }
                signalled += 1;
            }
        }
        signalled
    }
}
