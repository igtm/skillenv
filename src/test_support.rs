//! Test helpers shared across modules.
//!
//! This exists to hold one thing: the `HOME` guard. `HOME` is process-global, so
//! a test that points it at a temporary directory changes it for every test
//! running concurrently. The guard serializes those tests against each other.
//!
//! The guard used to live inside `lib.rs`'s own test module, which meant
//! `remote.rs`'s tests could not reach it — and they read `HOME` indirectly,
//! through `load_config`. So while one test had `HOME` redirected, another would
//! read the redirected value and load a different configuration. The suite passed
//! serially and failed roughly two runs in three under parallel execution.
//!
//! Every test that reads or writes `HOME`, directly or through `load_config`, must
//! hold this guard.
#![cfg(test)]

use std::env;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

fn home_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Point `HOME` at `home` (or unset it) until the guard drops.
///
/// Holds a process-wide lock for its lifetime, so concurrent tests that also
/// touch `HOME` wait rather than observing each other's value.
pub(crate) fn set_home_for_test(home: Option<&Path>) -> HomeEnvGuard {
    // A poisoned lock only means some other test panicked while holding it; the
    // environment is still ours to set, so recovering is better than cascading
    // the panic into every later test.
    let lock = home_env_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let previous = env::var_os("HOME");
    apply(home);
    HomeEnvGuard {
        previous,
        _lock: lock,
    }
}

fn apply(home: Option<&Path>) {
    match home {
        // SAFETY: the lock above makes this the only thread mutating HOME.
        Some(path) => unsafe { env::set_var("HOME", path) },
        None => unsafe { env::remove_var("HOME") },
    }
}

pub(crate) struct HomeEnvGuard {
    previous: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl Drop for HomeEnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            // SAFETY: still holding the lock, so no other thread is reading HOME.
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }
    }
}
