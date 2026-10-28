//! Test hooks that let the binary's tests drive a renderer that would otherwise
//! load its own state.
//!
//! A renderer prints to stdout, which a unit test in the same process cannot
//! capture, and it loads its own runner. The hooks here stand in for that state
//! and only answer while the reading test asked for them, so the binary's other
//! tests render exactly what they always did.

use std::sync::Mutex;

const READING: &str = "OPM_TEST_LIST_CPU_PERCENT";

fn is_reading() -> bool {
    std::env::var_os(READING).is_some()
}

static HOOKED_PROCESS: Mutex<Option<opm::process::Process>> = Mutex::new(None);

/// Stand in for the process a renderer loads under `id`.
pub fn hook_process(process: opm::process::Process) {
    let mut slot = HOOKED_PROCESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = Some(process);
}

/// The process a test hooked for `id`, if any, while that test is reading.
pub fn hooked_process(id: usize) -> Option<opm::process::Process> {
    if !is_reading() {
        return None;
    }

    let slot = HOOKED_PROCESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    slot.clone().filter(|process| process.id == id)
}
