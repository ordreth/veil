//! Desktop notifications for errors.
//!
//! A failure that repeats must not flood the desktop, so every notification
//! passes a [`Throttle`] first: the same message is shown once per minute, and
//! no more than a few messages are shown per minute whatever they say. What is
//! held back is still in the log, where the caller has written it.

use notify_rust::{Notification, Urgency};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long the same message is not shown again.
pub const REPEAT_AFTER: Duration = Duration::from_secs(60);

/// How many notifications are shown within [`REPEAT_AFTER`], whatever they say.
pub const MAX_PER_WINDOW: usize = 5;

/// How many notifications may be on their way at once (see
/// [`notify_error_detached`]). A notification daemon that does not answer
/// keeps each of them waiting on a thread.
const MAX_WAITING: usize = 4;

/// What a [`Throttle`] says about a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Show it.
    Show,
    /// The same message was shown less than the window ago.
    Repeated,
    /// Too many notifications were shown within the window.
    TooMany,
}

/// Decides which notifications are shown: each distinct message once per
/// `window`, and at most `limit` messages per `window` in all. It keeps no
/// clock: the caller says what time it is, so the rule is tested without
/// waiting.
#[derive(Debug)]
pub struct Throttle {
    window: Duration,
    limit: usize,
    /// The notifications shown within the last `window`, oldest first. Never
    /// more than `limit`.
    shown: Vec<(String, Instant)>,
}

impl Throttle {
    pub const fn new(window: Duration, limit: usize) -> Self {
        Self {
            window,
            limit,
            shown: Vec::new(),
        }
    }

    /// Whether `message` may be shown at `now`. A `Show` is remembered; a
    /// notification that is held back does not extend the time its message
    /// stays quiet.
    pub fn admit(&mut self, message: &str, now: Instant) -> Verdict {
        let window = self.window;
        self.shown
            .retain(|(_, at)| now.saturating_duration_since(*at) < window);

        if self.shown.iter().any(|(shown, _)| shown == message) {
            return Verdict::Repeated;
        }
        if self.shown.len() >= self.limit {
            return Verdict::TooMany;
        }
        self.shown.push((message.to_owned(), now));
        Verdict::Show
    }
}

/// The throttle every notification of this process passes.
static THROTTLE: Mutex<Throttle> = Mutex::new(Throttle::new(REPEAT_AFTER, MAX_PER_WINDOW));

/// Notifications that are on their way on a thread of their own.
static WAITING: AtomicUsize = AtomicUsize::new(0);

/// Asks the throttle, and says in the log why a notification is not shown.
fn admitted(summary: &str, body: &str) -> bool {
    let verdict = THROTTLE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .admit(&format!("{summary}\n{body}"), Instant::now());
    match verdict {
        Verdict::Show => true,
        Verdict::Repeated => {
            tracing::debug!("not notifying again within a minute: {summary}: {body}");
            false
        }
        Verdict::TooMany => {
            tracing::debug!(
                "too many notifications within a minute, not showing: {summary}: {body}"
            );
            false
        }
    }
}

fn show(summary: &str, body: &str) {
    if let Err(e) = Notification::new()
        .appname("Veil")
        .summary(summary)
        .body(body)
        .urgency(Urgency::Critical)
        .show()
    {
        tracing::warn!("Failed to dispatch desktop notification: {e}");
    }
}

/// Dispatches a critical desktop notification for an error, unless the
/// throttle holds it back. This waits for the notification daemon, for as long
/// as that takes to answer: use it only where the process is about to exit
/// anyway. Everything else uses [`notify_error_detached`].
pub fn notify_error(summary: &str, body: &str) {
    if admitted(summary, body) {
        show(summary, body);
    }
}

/// Like [`notify_error`], for a caller that must not wait (the event loop): the
/// notification is sent from a thread of its own, which ends once it is
/// delivered. If several are still waiting for the notification daemon, this
/// one is dropped: a daemon that does not answer must not collect threads.
pub fn notify_error_detached(summary: &str, body: &str) {
    if !admitted(summary, body) {
        return;
    }
    if WAITING.fetch_add(1, Ordering::AcqRel) >= MAX_WAITING {
        WAITING.fetch_sub(1, Ordering::AcqRel);
        tracing::warn!(
            "the notification daemon does not answer ({MAX_WAITING} notifications are waiting); \
             not showing: {summary}: {body}"
        );
        return;
    }

    let (summary, body) = (summary.to_owned(), body.to_owned());
    let spawned = std::thread::Builder::new()
        .name("veil-notify".into())
        .spawn(move || {
            show(&summary, &body);
            WAITING.fetch_sub(1, Ordering::AcqRel);
        });
    if let Err(e) = spawned {
        WAITING.fetch_sub(1, Ordering::AcqRel);
        tracing::warn!("Failed to start a thread for a desktop notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    fn seconds(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn the_same_message_is_shown_once_per_window() {
        let mut throttle = Throttle::new(MINUTE, 5);
        let start = Instant::now();

        assert_eq!(throttle.admit("the video stopped", start), Verdict::Show);
        for second in [0, 1, 30, 59] {
            assert_eq!(
                throttle.admit("the video stopped", start + seconds(second)),
                Verdict::Repeated,
                "after {second} s"
            );
        }
        // A minute after it was shown, not a minute after it was last asked for.
        assert_eq!(
            throttle.admit("the video stopped", start + seconds(60)),
            Verdict::Show
        );
        assert_eq!(
            throttle.admit("the video stopped", start + seconds(61)),
            Verdict::Repeated
        );
    }

    #[test]
    fn different_messages_do_not_hold_each_other_back() {
        let mut throttle = Throttle::new(MINUTE, 5);
        let start = Instant::now();
        assert_eq!(throttle.admit("a", start), Verdict::Show);
        assert_eq!(throttle.admit("b", start), Verdict::Show);
        assert_eq!(throttle.admit("a", start + seconds(1)), Verdict::Repeated);
        assert_eq!(throttle.admit("b", start + seconds(1)), Verdict::Repeated);
    }

    #[test]
    fn no_more_than_the_limit_is_shown_per_window_whatever_the_messages() {
        let mut throttle = Throttle::new(MINUTE, 3);
        let start = Instant::now();

        // Messages that differ every time (a position, a counter) get past the
        // first rule, but not past this one.
        for n in 0..3 {
            let message = format!("error at frame {n}");
            assert_eq!(throttle.admit(&message, start + seconds(n)), Verdict::Show);
        }
        for n in 3..50 {
            let message = format!("error at frame {n}");
            assert_eq!(
                throttle.admit(&message, start + seconds(10)),
                Verdict::TooMany
            );
        }
        // What was held back is not remembered.
        assert_eq!(throttle.shown.len(), 3);

        // Room is made as the shown ones leave the window, one by one.
        assert_eq!(
            throttle.admit("another", start + seconds(59)),
            Verdict::TooMany
        );
        assert_eq!(
            throttle.admit("another", start + seconds(60)),
            Verdict::Show
        );
        assert_eq!(
            throttle.admit("one more", start + seconds(60)),
            Verdict::TooMany
        );
        assert_eq!(
            throttle.admit("one more", start + seconds(61)),
            Verdict::Show
        );
    }

    #[test]
    fn a_message_that_was_held_back_is_shown_as_soon_as_there_is_room() {
        let mut throttle = Throttle::new(MINUTE, 1);
        let start = Instant::now();
        assert_eq!(throttle.admit("first", start), Verdict::Show);
        // Asking again and again does not push the moment back.
        for second in 1..60 {
            assert_eq!(
                throttle.admit("second", start + seconds(second)),
                Verdict::TooMany
            );
        }
        assert_eq!(throttle.admit("second", start + seconds(60)), Verdict::Show);
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_panic_or_show_more() {
        let mut throttle = Throttle::new(MINUTE, 5);
        let start = Instant::now() + seconds(100);
        assert_eq!(throttle.admit("a", start), Verdict::Show);
        assert_eq!(throttle.admit("a", start - seconds(50)), Verdict::Repeated);
    }
}
