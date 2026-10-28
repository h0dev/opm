//! Exit paths of the daemon that may run in signal context.
//!
//! The monitor loop observes [`SHUTDOWN_REQUESTED`] at the top of every pass and
//! tears the daemon down from there, where allocation, locking and logging are
//! fine. The signal handler only sets that flag and arms a bounded fallback, so
//! a signal that interrupts the loop cannot deadlock it against the state lock.
//!
//! The fallback makes no allocation, takes no lock and logs nothing. It signals
//! a snapshot of the tracked processes that is refreshed on every state change
//! (and written to `~/.opm/termination_targets`, so a daemon killed without any
//! exit path does not leave the next one with nothing to clean up). It runs
//! either on a second signal, on the alarm armed by the first one when the loop
//! never gets back to its teardown, or when the loop finds the grace spent.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::Duration;

use super::pids::PidSnapshot;

/// Set by the signal handler: the monitor loop does the teardown.
pub static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);
/// Set by the first signal: a second one means the loop is not finishing, so it
/// exits at once instead of waiting out the grace.
pub static FALLBACK_ARMED: AtomicBool = AtomicBool::new(false);
/// Whether the loop has taken the shutdown over, so its first pass after the
/// signal spends the grace exactly once.
pub static LOOP_SHUTDOWN: AtomicBool = AtomicBool::new(false);
/// When the fallback gives up if the loop has not torn down (0: no clock yet).
pub static FALLBACK_DEADLINE_US: AtomicI64 = AtomicI64::new(0);
/// How long the fallback leaves the loop, after a signal, to tear down safely
/// (microseconds, the unit of [`elapsed_us`]). A pass in progress at the signal
/// finishes on its own and the shutdown waits up to `super::TERMINATION_GRACE`;
/// this covers both.
pub const MONITOR_GRACE_US: i64 = 5_000_000;

/// The monitor loop advances this at the end of every pass, so a fallback that
/// has no clock armed is judged from a moment the loop really did run.
pub static MONITOR_LOOP_PROGRESS: AtomicUsize = AtomicUsize::new(0);

static PIDS: PidSnapshot = PidSnapshot::new();

/// Replace the snapshot the fallback signals.
pub fn store_pids(targets: &[i64], session: Option<i64>) {
    PIDS.store(targets, session);
}

/// The snapshot the fallback would signal, in insertion order.
pub fn snapshot_pids() -> Vec<i64> {
    PIDS.snapshot()
}

/// The fallback's monotonic clock in microseconds, only ever compared with
/// itself and with [`MONITOR_GRACE_US`].
///
/// A reading of 0 is reserved for "nothing armed", so the clock never reports
/// it.
pub fn elapsed_us() -> i64 {
    raw_us().max(1)
}

/// Microseconds since the first reading, as a monotonic offset. Microseconds
/// rather than milliseconds because a deadline derived from two identical
/// readings would never be reached.
#[cfg(target_os = "linux")]
fn raw_us() -> i64 {
    use std::time::Instant;

    static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let origin = ORIGIN.get_or_init(Instant::now);
    origin.elapsed().as_micros() as i64
}

#[cfg(not(target_os = "linux"))]
fn raw_us() -> i64 {
    0
}

/// Called from the monitor loop at the end of every pass.
pub fn monitor_loop_progress() {
    MONITOR_LOOP_PROGRESS.store(elapsed_us() as usize, Ordering::SeqCst);
}

/// Called by the loop at the top of a pass once the shutdown request is visible.
///
/// A pass means the loop is not blocked. The first such pass spends the grace:
/// the fallback then gives up after [`MONITOR_GRACE_US`] however many further
/// passes come, so a loop that keeps ticking but never reaches its teardown
/// cannot postpone the exit.
pub fn clear_fallback_after_a_shutdown_request() {
    let requested = SHUTDOWN_REQUESTED.load(Ordering::SeqCst);
    let now = elapsed_us();

    if requested {
        // Spent once: a further pass must not push the deadline out.
        if !LOOP_SHUTDOWN.swap(true, Ordering::SeqCst) {
            FALLBACK_DEADLINE_US.store(now + MONITOR_GRACE_US, Ordering::SeqCst);
        }
    }
    MONITOR_LOOP_PROGRESS.store(now as usize, Ordering::SeqCst);
}

/// Whether the fallback should exit now instead of waiting for the loop.
pub fn fallback_grace_expired() -> bool {
    if !FALLBACK_ARMED.load(Ordering::SeqCst) {
        return false;
    }

    let now = elapsed_us();
    let deadline = FALLBACK_DEADLINE_US.load(Ordering::SeqCst);
    if deadline > 0 {
        return now >= deadline;
    }

    // Nothing armed a clock (no `alarm()` yet, or a test): judge from the last
    // pass the loop really ran.
    let progress = MONITOR_LOOP_PROGRESS.load(Ordering::SeqCst) as i64;
    progress == 0 || now - progress >= MONITOR_GRACE_US
}

/// Write the snapshot under `~/.opm` so the next daemon can clean up if this one
/// is killed without running any exit path.
pub fn record_snapshot() {
    let Some(home) = home::home_dir() else {
        return;
    };

    let encoded = snapshot_pids()
        .iter()
        .map(|pid| pid.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let _ = std::fs::write(
        format!("{}/.opm/termination_targets", home.display()),
        encoded,
    );
}

/// Fill the snapshot from the last daemon's file, before this daemon's own
/// state has been written once.
pub fn restore_snapshot() {
    let Some(home) = home::home_dir() else {
        return;
    };

    let Ok(encoded) = std::fs::read_to_string(format!(
        "{}/.opm/termination_targets",
        home.display()
    )) else {
        return;
    };

    let targets: Vec<i64> = encoded
        .split_whitespace()
        .filter_map(|pid| pid.parse().ok())
        .collect();
    store_pids(&targets, None);
}

/// Remove state that marks the daemon as alive, so the next `opm` can start one.
pub fn clear_runtime_files() {
    if let Some(home_dir) = home::home_dir() {
        let _ = std::fs::remove_file(format!("{}/.opm/daemon.lock", home_dir.display()));
        let _ = std::fs::remove_file(format!(
            "{}/.opm/termination_targets",
            home_dir.display()
        ));
    }

    super::pid::remove();
}

/// Signal the snapshot and wait up to `grace` for those pids to stop.
///
/// Called with SIGINT/SIGTERM/SIGALRM blocked, so the sleeps cannot be cut short
/// and a lock taken here cannot deadlock against a handler interrupted mid
/// operation: that handler only runs once this returns.
pub fn terminate_snapshot(grace: Duration) {
    let targets = snapshot_pids();
    if targets.is_empty() {
        return;
    }

    PIDS.signal_all();

    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline
        && targets
            .iter()
            .any(|&pid| opm::process::is_pid_finished(pid))
    {
        sleep(Duration::from_millis(100));
    }
}

/// The last-resort exit. Only plain stores and one `kill` per snapshot slot.
///
/// Signal context cannot log or clear files safely, and it must not take the
/// state lock: the next daemon start removes the stale runtime files anyway.
pub fn run_fallback() -> ! {
    PIDS.signal_all();
    unsafe { libc::_exit(1) }
}

/// Install the handlers.
///
/// The signals are blocked while they are installed so none is handled inside
/// the setup itself; unblocking delivers a pending one to the handler now in
/// place.
pub fn start() {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGALRM);
        libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());

        libc::sigaction(
            libc::SIGINT,
            &action(super::handle_termination_signal),
            std::ptr::null_mut(),
        );
        libc::sigaction(
            libc::SIGTERM,
            &action(super::handle_termination_signal),
            std::ptr::null_mut(),
        );
        libc::sigaction(
            libc::SIGALRM,
            &action(super::handle_alarm),
            std::ptr::null_mut(),
        );
        libc::signal(libc::SIGPIPE, super::handle_sigpipe as *const () as usize);

        libc::sigprocmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
    }
}

fn action(handler: extern "C" fn(libc::c_int)) -> libc::sigaction {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler as *const () as usize;
    action.sa_flags = 0;
    action
}
