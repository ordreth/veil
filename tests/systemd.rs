//! The systemd user unit (`contrib/systemd/veil.service`) agrees with the
//! binary: how it reports readiness, which exit statuses are not worth a
//! restart, and the allocator setting of known issue 3.

use veil::error::{EXIT_ALREADY_RUNNING, EXIT_COMPOSITOR_GONE, EXIT_FAILURE, EXIT_USAGE};

const UNIT: &str = include_str!("../contrib/systemd/veil.service");

/// The value of `key` in the unit (the last one, as systemd reads it), with
/// comments left out.
fn value(key: &str) -> Option<&'static str> {
    UNIT.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .filter(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim())
        .next_back()
}

#[test]
fn the_unit_waits_for_the_daemon_to_be_ready() {
    assert_eq!(value("Type"), Some("notify"));
    let exec = value("ExecStart").expect("ExecStart");
    assert!(exec.ends_with("/veil daemon"), "{exec}");
    // The event loop pings at half of it; there must be one to ping.
    assert!(value("WatchdogSec").is_some_and(|v| v != "0"));
}

#[test]
fn the_unit_restarts_only_what_a_restart_can_mend() {
    assert_eq!(value("Restart"), Some("on-failure"));
    let prevented: Vec<u8> = value("RestartPreventExitStatus")
        .expect("RestartPreventExitStatus")
        .split_whitespace()
        .map(|status| status.parse().expect("a numeric exit status"))
        .collect();
    // A command line or a configuration file that is wrong.
    for status in [EXIT_USAGE, EXIT_ALREADY_RUNNING, EXIT_COMPOSITOR_GONE] {
        assert!(prevented.contains(&status), "{status} in {prevented:?}");
    }
    // Every other failure (a crash, a compositor that is not there yet) is
    // restarted.
    assert!(!prevented.contains(&EXIT_FAILURE));
}

#[test]
fn the_unit_limits_the_allocator_arenas() {
    // docs/known-issues.md, entry 3: glibc keeps what the threads of a
    // replaced video freed, in an arena per thread; with one it stays flat.
    assert_eq!(value("Environment"), Some("MALLOC_ARENA_MAX=1"));
}
