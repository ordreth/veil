//! What the compositor shows over the wallpapers, and what is done with
//! videos that cannot be seen.
//!
//! A wallpaper is paused while `veil pause` holds it (`Screen::held`) or the
//! compositor hides it; `update_pause` combines the two.
//!
//! Two sources say that a video is not seen. The compositor backend reports to
//! `impl CompositorEvents for AppState` what each output shows;
//! `policy::Visibility` (pure) decides which output's wallpaper plays and when,
//! and this applies its verdicts to the screens, through each screen's
//! `RenderState::set_paused`, with a `calloop` timer for the pauses that are
//! due later. And a compositor stops taking the frames of a video it does not
//! show (a locked session, a monitor that is off, on any compositor;
//! `unseen::StallWatch`): a second timer, which runs only while a video plays,
//! reads the video's frame counts, releases a video nobody sees
//! (`RenderState::release_unseen`), and leaves a frame callback outstanding on
//! its surface. The frame handler shows the video again when the compositor
//! answers it (`RenderState::seen`).

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};

use smithay_client_toolkit::shell::WaylandSurface;

use super::binding::OutputKey;
use super::state::{request_frame, AppState};
use super::unseen::StallWatch;
use crate::compositor::{CompositorEvent, CompositorEvents};
use crate::policy::Playback;

impl CompositorEvents for AppState {
    fn compositor_event(&mut self, event: CompositorEvent) {
        log_event(&event);
        let resumed = self.visibility.event(&event, Instant::now());
        self.apply_playback(resumed);
        self.schedule_pauses();
    }
}

impl AppState {
    /// Gives every screen in `changes` its verdict. A name the daemon does not
    /// draw on is skipped; its screen, if one is made later, is given the
    /// verdict then (`apply_current_playback`).
    fn apply_playback(&mut self, changes: Vec<(String, Playback)>) {
        for (output, _) in changes {
            self.update_pause(&OutputKey::Named(output));
        }
    }

    /// Gives the screen `key` the verdict that holds for its output now: a
    /// screen that is new, or an output that is back.
    pub(super) fn apply_current_playback(&mut self, key: &OutputKey) {
        self.update_pause(key);
    }

    /// Whether the compositor hides the wallpaper of `key` (the verdict of
    /// `visibility`). An output without a name is never reported.
    pub(super) fn is_hidden(&self, key: &OutputKey) -> bool {
        key.name()
            .is_some_and(|name| self.visibility.playback(name) == Playback::Pause)
    }

    /// Pauses the wallpaper of `key` if `veil pause` holds it or the
    /// compositor hides it, and lets it play otherwise.
    pub(super) fn update_pause(&mut self, key: &OutputKey) {
        let hidden = self.is_hidden(key);
        let Some(screen) = self.screen(key) else {
            return;
        };
        let paused = screen.held || hidden;
        if screen.render.is_paused() == paused {
            return;
        }
        let what = match (paused, screen.held) {
            (true, true) => "is paused by `veil pause`",
            (true, false) => "cannot be seen; pausing it",
            (false, _) => "can be seen and is not held; playing it",
        };
        if screen.render.is_dynamic() {
            tracing::info!("the wallpaper of {key} {what}");
        } else {
            tracing::debug!("the wallpaper of {key} {what}");
        }
        // Through the target: a video that plays again needs its
        // frame-callback loop again, and `with_target` starts it.
        self.with_target(key, |render, _| render.set_paused(paused));
    }

    /// Arms the timer for the next pause that `visibility` has waiting, if it
    /// is not armed. One timer serves them all: each pause is due a fixed delay
    /// after its event, so a later event never has an earlier deadline than the
    /// armed one, and the timer arms itself again for the next.
    fn schedule_pauses(&mut self) {
        if self.pause_timer_armed {
            return;
        }
        let (Some(deadline), Some(handle)) = (self.visibility.next_due(), &self.loop_handle) else {
            return;
        };
        let timer = Timer::from_deadline(deadline);
        let inserted = handle.insert_source(timer, |_, _, state| {
            let paused = state.visibility.due(Instant::now());
            state.apply_playback(paused);
            match state.visibility.next_due() {
                Some(next) => TimeoutAction::ToInstant(next),
                None => {
                    state.pause_timer_armed = false;
                    TimeoutAction::Drop
                }
            }
        });
        match inserted {
            Ok(_) => self.pause_timer_armed = true,
            // Without the timer nothing is paused, which is what the daemon does
            // without a compositor backend; the next event tries again.
            Err(e) => tracing::warn!("cannot set the timer that pauses hidden wallpapers: {e}"),
        }
    }
}

/// How often the daemon looks for videos whose surface is not drawn, while a
/// video plays.
const UNSEEN_TICK: Duration = Duration::from_secs(1);

impl AppState {
    /// Arms the timer that releases videos whose surface the compositor does
    /// not draw, unless it is armed or turned off. It ends by itself once no
    /// video plays; `with_target` arms it again when one does.
    pub(super) fn watch_unseen(&mut self) {
        if self.unseen_timer_armed || self.release_unseen_after.is_none() {
            return;
        }
        let Some(handle) = &self.loop_handle else {
            return;
        };
        let inserted = handle.insert_source(Timer::from_duration(UNSEEN_TICK), |_, _, state| {
            if state.release_unseen_videos(Instant::now()) {
                TimeoutAction::ToDuration(UNSEEN_TICK)
            } else {
                state.unseen_timer_armed = false;
                TimeoutAction::Drop
            }
        });
        match inserted {
            Ok(_) => self.unseen_timer_armed = true,
            Err(e) => tracing::warn!("cannot set the timer that looks for unseen videos: {e}"),
        }
    }

    /// Observes the frame counts of every playing video, and releases those
    /// whose frames the compositor has refused for `release_unseen_after` at
    /// `now`. Returns whether a video still plays somewhere, and needs
    /// watching.
    fn release_unseen_videos(&mut self, now: Instant) -> bool {
        let Some(after) = self.release_unseen_after else {
            return false;
        };
        let qh = &self.qh;
        let mut playing = false;
        for screen in &mut self.screens {
            let Some(surface) = &mut screen.surface else {
                continue;
            };
            if !screen.render.plays_video() {
                continue;
            }
            if let Some(counts) = screen.render.frame_counts() {
                tracing::trace!("frames of {}: {counts:?}", screen.key);
                surface.stall.observe(counts, now);
            }
            if !surface.stall.unseen(now, after) || !screen.render.release_unseen() {
                playing = true;
                continue;
            }
            surface.stall = StallWatch::default();
            // The answer to a frame callback is the sign that the surface is
            // drawn again (`frame`); the video's loop may have left one.
            let wl_surface = surface.layer.wl_surface().clone();
            request_frame(qh, surface, &wl_surface);
            tracing::info!(
                "the video of {} has not been shown for {} s (a locked screen, a monitor that \
                 is off?); it is stopped until its output shows it again",
                screen.key,
                after.as_secs()
            );
        }
        playing
    }
}

fn log_event(event: &CompositorEvent) {
    match event {
        CompositorEvent::WorkspaceChanged { output, workspace } => {
            tracing::debug!("compositor: {output} shows workspace {workspace}");
        }
        CompositorEvent::FullscreenChanged { output, fullscreen } => {
            tracing::debug!(
                "compositor: {output} {} a fullscreen window",
                if *fullscreen {
                    "shows"
                } else {
                    "no longer shows"
                }
            );
        }
        CompositorEvent::CoverageChanged { output, covered } => {
            tracing::debug!(
                "compositor: the windows on {output} {} cover it",
                if *covered { "now" } else { "no longer" }
            );
        }
        CompositorEvent::OutputGone { output } => {
            tracing::debug!("compositor: {output} is no longer reported");
        }
        CompositorEvent::Disconnected { reason } => {
            tracing::debug!("compositor: nothing is known about the outputs any more ({reason})");
        }
    }
}
