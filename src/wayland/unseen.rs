//! Whether the compositor still shows the video of a surface.
//!
//! `waylandsink` hands a frame to the compositor only once the compositor has
//! answered the frame callback of the frame before. A compositor answers when
//! it draws the surface, and does not while it does not: while the session is
//! locked, while the output is powered off, and (sway) while a window covers
//! the video completely. The sink then replaces every new frame with the next
//! and counts it as dropped (GStreamer 1.28.7, `gst_wl_window_render_hdr`
//! returns false, and `waylandsink` reports `GST_BASE_SINK_FLOW_DROPPED`). So
//! frames that are dropped while hardly any is rendered mean that nobody sees
//! the video; frames that are neither (a video of one frame, a slide show)
//! mean nothing.
//!
//! veil's own layer surface cannot say it: a video that covers all of it hides
//! it from the compositor, which then never answers its frame callbacks while
//! the video is perfectly visible (measured in a headless sway 1.12).
//!
//! Pure: the caller passes the counts and the time.

use std::time::{Duration, Instant};

use crate::engine::FrameCounts;

/// The frame counts of one playing video, observed from time to time.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StallWatch {
    last: Option<FrameCounts>,
    /// Since when every observation has found frames dropped and (almost) none
    /// rendered.
    refused_since: Option<Instant>,
}

impl StallWatch {
    /// Takes in the counts at `now`. Counters that went down started again
    /// from zero (a sink resets them at every loop of the video).
    pub fn observe(&mut self, counts: FrameCounts, now: Instant) {
        if let Some(last) = self.last {
            let rendered = since(last.rendered, counts.rendered);
            let dropped = since(last.dropped, counts.dropped);
            // One rendered frame is allowed: the first frame after a loop is
            // taken even by a compositor that shows nothing. Neither rendered
            // nor dropped frames (a still, a slide show, a loop that has just
            // reset the counters) change nothing either way.
            if rendered > 1 {
                self.refused_since = None;
            } else if dropped > 0 {
                self.refused_since.get_or_insert(now);
            }
        }
        self.last = Some(counts);
    }

    /// Whether the compositor has refused the frames for at least `after` at
    /// `now`.
    pub fn unseen(&self, now: Instant, after: Duration) -> bool {
        self.refused_since
            .is_some_and(|since| now.saturating_duration_since(since) >= after)
    }
}

/// How much a counter grew from `before` to `now`; all of `now` if it started
/// again from zero in between.
fn since(before: u64, now: u64) -> u64 {
    now.checked_sub(before).unwrap_or(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFTER: Duration = Duration::from_secs(2);

    fn counts(rendered: u64, dropped: u64) -> FrameCounts {
        FrameCounts { rendered, dropped }
    }

    /// Observes `seen` (rendered, dropped) once a second from `t0`, and says
    /// after which observation the video was first unseen.
    fn first_unseen(seen: &[(u64, u64)]) -> Option<usize> {
        let t0 = Instant::now();
        let mut watch = StallWatch::default();
        seen.iter()
            .enumerate()
            .find_map(|(i, (rendered, dropped))| {
                let now = t0 + Duration::from_secs(i as u64);
                watch.observe(counts(*rendered, *dropped), now);
                watch.unseen(now, AFTER).then_some(i)
            })
    }

    #[test]
    fn a_video_that_is_shown_is_never_unseen() {
        // 30 frames a second, all rendered.
        let shown: Vec<_> = (0..20).map(|s| (30 * s, 0)).collect();
        assert_eq!(first_unseen(&shown), None);
        // Under load: some dropped, most rendered.
        let loaded: Vec<_> = (0..20).map(|s| (20 * s, 10 * s)).collect();
        assert_eq!(first_unseen(&loaded), None);
        // 120 frames a second on a 30 Hz output: a quarter is taken.
        let fast: Vec<_> = (0..20).map(|s| (30 * s, 90 * s)).collect();
        assert_eq!(first_unseen(&fast), None);
    }

    #[test]
    fn a_video_whose_frames_are_all_dropped_is_unseen_after_the_delay() {
        // Shown for 3 s, then the screen is locked.
        let seen = [
            (0, 0),
            (30, 0),
            (60, 0),
            (61, 29),
            (61, 59),
            (61, 89),
            (61, 119),
        ];
        // Refused from the observation at 3 s on; unseen 2 s later.
        assert_eq!(first_unseen(&seen), Some(5));
    }

    #[test]
    fn a_loop_while_unseen_does_not_count_as_shown() {
        // Every loop resets the counters, and the first frame after it is
        // taken; a two-second clip loops at every other observation.
        let seen = [(90, 0), (91, 30), (1, 0), (1, 30), (1, 0), (1, 30), (1, 0)];
        // The resets in between are not progress either.
        assert!(first_unseen(&seen).is_some());
    }

    #[test]
    fn nothing_rendered_and_nothing_dropped_means_nothing() {
        // A video of one frame, or one that waits for a restart.
        let still: Vec<_> = (0..20).map(|_| (1, 0)).collect();
        assert_eq!(first_unseen(&still), None);
        // A slide show of a frame every ten seconds.
        let slow: Vec<_> = (0..30).map(|s| (s / 10, 0)).collect();
        assert_eq!(first_unseen(&slow), None);
    }

    #[test]
    fn frames_taken_again_end_the_refusal() {
        let t0 = Instant::now();
        let mut watch = StallWatch::default();
        watch.observe(counts(10, 0), t0);
        watch.observe(counts(10, 30), t0 + Duration::from_secs(1));
        watch.observe(counts(40, 30), t0 + Duration::from_secs(2));
        assert!(!watch.unseen(t0 + Duration::from_secs(10), AFTER));
    }

    #[test]
    fn a_new_watch_has_seen_nothing() {
        let watch = StallWatch::default();
        assert!(!watch.unseen(Instant::now() + Duration::from_secs(60), AFTER));
    }
}
