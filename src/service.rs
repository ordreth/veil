//! What the daemon tells its service manager (a systemd unit with
//! `Type=notify`, `contrib/systemd/veil.service`): that it is ready (`READY=1`)
//! once every output found at start-up shows its wallpaper or has failed to,
//! a status line (`STATUS=`) for start-up and shutdown, that it is stopping
//! (`STOPPING=1`), and that its event loop is alive (`WATCHDOG=1`, when the
//! unit has `WatchdogSec=`).
//!
//! Without a service manager (`NOTIFY_SOCKET` unset) every call does nothing.
//! [`Readiness`] is pure; the event loop feeds it (`AppState::check_ready`).

use std::fmt::Display;
use std::time::Duration;

pub use sd_notify::NotifyState;

/// Sends `states` to the service manager, if the daemon runs under one.
///
/// `NOTIFY_SOCKET` is never unset: `remove_var` is unsound once other threads
/// exist (GStreamer's, the bus threads), and the daemon notifies more than once.
/// systemd ignores messages from other processes of the unit by default
/// (`NotifyAccess=main`), so a child that inherits the variable is harmless.
pub fn notify(states: &[NotifyState]) {
    if let Err(e) = sd_notify::notify(false, states) {
        tracing::debug!("cannot notify the service manager: {e}");
    }
}

/// How often to tell the service manager that the event loop is alive, if the
/// unit has a watchdog (`WatchdogSec=`, which systemd passes in
/// `WATCHDOG_USEC` and `WATCHDOG_PID`).
pub fn watchdog_interval() -> Option<Duration> {
    let mut usec = 0;
    if !sd_notify::watchdog_enabled(false, &mut usec) {
        return None;
    }
    ping_interval(usec)
}

/// Half the watchdog timeout of `usec` microseconds, as `sd_watchdog_enabled(3)`
/// advises, or `None` for a timeout of zero (no watchdog).
pub fn ping_interval(usec: u64) -> Option<Duration> {
    (usec > 0).then(|| Duration::from_micros(usec / 2).max(Duration::from_millis(1)))
}

/// Whether stderr goes to the journal: systemd sets `JOURNAL_STREAM` to the
/// device and inode of the stream it connected, and stderr is still that
/// stream (it was not redirected by whoever started the process). The journal
/// stamps every line with its own time.
pub fn stderr_is_journal() -> bool {
    use std::os::fd::AsFd;
    use std::os::unix::fs::MetadataExt;

    let Some(stream) = std::env::var_os("JOURNAL_STREAM") else {
        return false;
    };
    let Ok(fd) = std::io::stderr().as_fd().try_clone_to_owned() else {
        return false;
    };
    let Ok(stderr) = std::fs::File::from(fd).metadata() else {
        return false;
    };
    stream
        .to_str()
        .is_some_and(|stream| is_stream(stream, stderr.dev(), stderr.ino()))
}

/// Whether `journal_stream` (`JOURNAL_STREAM`, `<device>:<inode>`) names the
/// file with `dev` and `ino`.
pub fn is_stream(journal_stream: &str, dev: u64, ino: u64) -> bool {
    journal_stream
        .split_once(':')
        .is_some_and(|(d, i)| d.parse() == Ok(dev) && i.parse() == Ok(ino))
}

/// Where an output found at start-up stands, as the event loop sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Its wallpaper is not shown yet: the surface is not configured, or a
    /// video waits for its first frame.
    Waiting,
    /// Its wallpaper is on screen.
    Shown,
    /// It has no surface any more (unplugged, or closed by the compositor).
    Gone,
}

/// Decides when the daemon is ready: when every output it found at start-up
/// shows its wallpaper, has failed to, or is gone. Outputs that come later do
/// not count, and nothing is decided again after that: `READY=1` is sent once.
#[derive(Debug)]
pub struct Readiness<K> {
    waiting: Vec<K>,
    shown: Vec<K>,
    failed: Vec<(K, String)>,
}

impl<K: Clone + PartialEq + Display> Readiness<K> {
    /// Waits for each of `outputs`.
    pub fn new(outputs: Vec<K>) -> Self {
        Self {
            waiting: outputs,
            shown: Vec::new(),
            failed: Vec::new(),
        }
    }

    /// The status line while the daemon starts.
    pub fn status(&self) -> String {
        if self.waiting.is_empty() {
            "Starting".into()
        } else {
            format!(
                "Starting; waiting for the first frame on {}",
                join(&self.waiting)
            )
        }
    }

    /// The wallpaper of `output` could not be shown. It is not waited for any
    /// more; the status line names it with `reason`.
    pub fn failed(&mut self, output: &K, reason: &dyn Display) {
        if let Some(index) = self.waiting.iter().position(|k| k == output) {
            let output = self.waiting.remove(index);
            self.failed.push((output, reason.to_string()));
        }
    }

    /// Asks `progress` about every output still waited for. Returns the status
    /// line to send with `READY=1` when none is left.
    pub fn update(&mut self, progress: impl Fn(&K) -> Progress) -> Option<String> {
        let mut index = 0;
        while let Some(output) = self.waiting.get(index) {
            match progress(output) {
                Progress::Waiting => index += 1,
                Progress::Shown => {
                    let output = self.waiting.remove(index);
                    self.shown.push(output);
                }
                Progress::Gone => {
                    self.waiting.remove(index);
                }
            }
        }
        self.waiting.is_empty().then(|| self.started_status())
    }

    fn started_status(&self) -> String {
        let mut status = String::from("Started");
        if !self.shown.is_empty() {
            status.push_str(&format!("; wallpaper shown on {}", join(&self.shown)));
        }
        for (output, reason) in &self.failed {
            status.push_str(&format!("; not shown on {output}: {reason}"));
        }
        if self.shown.is_empty() && self.failed.is_empty() {
            status.push_str("; no output to draw on yet");
        }
        status
    }
}

fn join<K: Display>(outputs: &[K]) -> String {
    outputs
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readiness(outputs: &[&'static str]) -> Readiness<&'static str> {
        Readiness::new(outputs.to_vec())
    }

    #[test]
    fn ready_when_every_output_shows_its_wallpaper() {
        let mut r = readiness(&["DP-3", "HDMI-A-1"]);
        assert_eq!(
            r.status(),
            "Starting; waiting for the first frame on DP-3, HDMI-A-1"
        );
        // An image on one, a video that has no frame yet on the other.
        let none = r.update(|k| match *k {
            "DP-3" => Progress::Shown,
            _ => Progress::Waiting,
        });
        assert_eq!(none, None);
        assert_eq!(
            r.status(),
            "Starting; waiting for the first frame on HDMI-A-1"
        );
        // DP-3 is no longer asked about: what it does later does not count.
        let ready = r.update(|k| match *k {
            "DP-3" => Progress::Waiting,
            _ => Progress::Shown,
        });
        assert_eq!(
            ready.as_deref(),
            Some("Started; wallpaper shown on DP-3, HDMI-A-1")
        );
    }

    #[test]
    fn a_failed_output_does_not_hold_readiness_back() {
        let mut r = readiness(&["DP-3", "HDMI-A-1"]);
        r.failed(&"HDMI-A-1", &"the video cannot be played");
        // Failing twice, or failing an output nobody waits for, changes nothing.
        r.failed(&"HDMI-A-1", &"again");
        r.failed(&"eDP-1", &"not at start-up");
        // A failed output is not asked about any more.
        let ready = r.update(|k| match *k {
            "DP-3" => Progress::Shown,
            _ => Progress::Waiting,
        });
        assert_eq!(
            ready.as_deref(),
            Some("Started; wallpaper shown on DP-3; not shown on HDMI-A-1: the video cannot be played")
        );

        let mut r = readiness(&["DP-3"]);
        r.failed(&"DP-3", &"no frame within 10 s");
        assert_eq!(
            r.update(|_| Progress::Waiting).as_deref(),
            Some("Started; not shown on DP-3: no frame within 10 s")
        );
    }

    #[test]
    fn outputs_that_are_gone_are_not_waited_for() {
        let mut r = readiness(&["DP-3", "HDMI-A-1"]);
        let ready = r.update(|k| match *k {
            "DP-3" => Progress::Gone,
            _ => Progress::Shown,
        });
        assert_eq!(
            ready.as_deref(),
            Some("Started; wallpaper shown on HDMI-A-1")
        );

        let mut r = readiness(&["DP-3"]);
        assert_eq!(
            r.update(|_| Progress::Gone).as_deref(),
            Some("Started; no output to draw on yet")
        );
    }

    #[test]
    fn nothing_to_wait_for_is_ready_at_once() {
        let mut r = readiness(&[]);
        assert_eq!(r.status(), "Starting");
        assert_eq!(
            r.update(|_| Progress::Waiting).as_deref(),
            Some("Started; no output to draw on yet")
        );
    }

    #[test]
    fn the_journal_stream_is_recognised_by_device_and_inode() {
        assert!(is_stream("8:123456", 8, 123456));
        assert!(!is_stream("8:123456", 8, 123457));
        assert!(!is_stream("9:123456", 8, 123456));
        for nonsense in ["", "8", "8:", ":123456", "x:y", "8:123456:7"] {
            assert!(!is_stream(nonsense, 8, 123456), "{nonsense:?}");
        }
    }

    #[test]
    fn the_watchdog_is_pinged_at_half_its_timeout() {
        assert_eq!(ping_interval(0), None);
        assert_eq!(ping_interval(30_000_000), Some(Duration::from_secs(15)));
        // Never a timer of zero, which would spin the loop.
        assert_eq!(ping_interval(1), Some(Duration::from_millis(1)));
    }
}
