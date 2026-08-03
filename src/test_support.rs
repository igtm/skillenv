//! Test helpers shared across modules.
//!
//! This exists to hold the process-environment guard. The environment is
//! process-global, so a test that redirects a variable changes it for every test
//! running concurrently. The guard serializes those tests against each other.
//!
//! The guard used to live inside `lib.rs`'s own test module, which meant
//! `remote.rs`'s tests could not reach it — and they read `HOME` indirectly,
//! through `load_config`. So while one test had `HOME` redirected, another would
//! read the redirected value and load a different configuration. The suite passed
//! serially and failed roughly two runs in three under parallel execution.
//!
//! Every test that reads or writes a process-wide variable must hold this guard.
//! There is deliberately one lock rather than one per variable: a test that needs
//! two of them cannot then deadlock against a test acquiring them in the other
//! order. The same failure shape reappeared later with `GIT_COMMITTER_DATE`, which
//! is the only way to give a fixture commit a chosen date — concurrent tests were
//! reading each other's timestamp, so a repository built to be old came out new.
#![cfg(test)]

use std::env;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

fn process_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Point `HOME` at `home` (or unset it) until the guard drops.
///
/// Holds a process-wide lock for its lifetime, so concurrent tests that also
/// touch `HOME` wait rather than observing each other's value.
pub(crate) fn set_home_for_test(home: Option<&Path>) -> EnvGuard {
    set_env_for_test(&[("HOME", home.map(|path| path.as_os_str().to_os_string()))])
}

/// Set (or unset) several process variables until the guard drops.
///
/// Restores exactly what was there before, including "was not set", so a test that
/// unsets a variable does not leave it unset for the next one.
pub(crate) fn set_env_for_test(vars: &[(&'static str, Option<OsString>)]) -> EnvGuard {
    // A poisoned lock only means some other test panicked while holding it; the
    // environment is still ours to set, so recovering is better than cascading
    // the panic into every later test.
    let lock = process_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());

    let mut previous = Vec::new();
    for (key, value) in vars {
        previous.push((*key, env::var_os(key)));
        apply(key, value.as_ref());
    }
    EnvGuard {
        previous,
        _lock: lock,
    }
}

fn apply(key: &str, value: Option<&OsString>) {
    match value {
        // SAFETY: the lock above makes this the only thread mutating the environment.
        Some(value) => unsafe { env::set_var(key, value) },
        None => unsafe { env::remove_var(key) },
    }
}

pub(crate) struct EnvGuard {
    previous: Vec<(&'static str, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            // SAFETY: still holding the lock, so no other thread is reading it.
            apply(key, value.as_ref());
        }
    }
}
