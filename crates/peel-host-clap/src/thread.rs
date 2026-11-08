//! Thread roles for the `clap.thread-check` host extension.
//!
//! CLAP distinguishes the main thread (everything that is not real time)
//! from the audio thread. A plugin may call `is_main_thread` and
//! `is_audio_thread` at any point to assert which one it is on, so the host
//! has to answer truthfully for the calling thread.
//!
//! The host does not guess from thread ids. Every entry point that runs
//! plugin code enters a [`RoleScope`], which sets a thread-local for the
//! duration of the call:
//!
//! - control-side calls (load, scan, state, idle, prepare) are the main thread
//! - `Device::process`, `reset` and the processing start/stop calls are the
//!   audio thread
//! - a thread the plugin created itself has no role, so it is neither
//!
//! Whichever thread holds a device's main-thread lock is, for the duration,
//! that device's main thread. The application is free to run control work
//! from more than one thread as long as it never does so concurrently.

use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    None,
    Main,
    Audio,
}

thread_local! {
    // `const` initialization: reading it on the audio thread never allocates
    // or registers a destructor.
    static ROLE: Cell<Role> = const { Cell::new(Role::None) };
}

pub(crate) fn current() -> Role {
    ROLE.get()
}

pub(crate) fn is_main_thread() -> bool {
    current() == Role::Main
}

pub(crate) fn is_audio_thread() -> bool {
    current() == Role::Audio
}

/// Sets the calling thread's role and restores the previous one on drop.
pub(crate) struct RoleScope {
    previous: Role,
}

impl RoleScope {
    pub(crate) fn enter(role: Role) -> RoleScope {
        RoleScope {
            previous: ROLE.replace(role),
        }
    }

    pub(crate) fn main() -> RoleScope {
        RoleScope::enter(Role::Main)
    }

    pub(crate) fn audio() -> RoleScope {
        RoleScope::enter(Role::Audio)
    }
}

impl Drop for RoleScope {
    fn drop(&mut self) {
        ROLE.set(self.previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_nest_and_restore() {
        assert_eq!(current(), Role::None);
        {
            let _main = RoleScope::main();
            assert!(is_main_thread() && !is_audio_thread());
            {
                let _audio = RoleScope::audio();
                assert!(is_audio_thread() && !is_main_thread());
            }
            assert!(is_main_thread());
        }
        assert_eq!(current(), Role::None);
        assert!(!is_main_thread() && !is_audio_thread());
    }

    #[test]
    fn roles_are_per_thread() {
        let _audio = RoleScope::audio();
        std::thread::spawn(|| assert_eq!(current(), Role::None))
            .join()
            .unwrap();
    }
}
