//! When a video wallpaper is paused because nobody can see it.
//!
//! The compositor backend (`crate::compositor`) reports what each output shows
//! (an [`OutputView`]). [`decide`] turns that into a verdict per output, and
//! [`Visibility`] keeps the latest view of every output and decides when a
//! verdict is applied: a wallpaper that becomes visible plays again at once,
//! one that is hidden is paused only after it has stayed hidden for a while, so
//! that switching workspaces back and forth does not stop and start the video
//! every time.
//!
//! Everything here is pure: the caller passes the time, and applies what comes
//! out (`AppState` in `wayland/state.rs`, through each screen's `RenderState`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::compositor::{CompositorEvent, OutputView, Snapshot};

/// What hides a wallpaper enough to pause it (`veil daemon --pause-on`). Each
/// level includes the ones before it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum PauseOn {
    /// Nothing: a video always plays.
    Never,
    /// A fullscreen window on the output (Hyprland's maximised windows are not
    /// fullscreen).
    #[default]
    Fullscreen,
    /// A fullscreen window, or windows that cover the whole output. Only their
    /// rectangles count, so a video is paused behind a translucent window too.
    Covered,
}

impl PauseOn {
    /// The name, as `--pause-on` and the configuration file take it.
    pub const fn name(self) -> &'static str {
        match self {
            PauseOn::Never => "never",
            PauseOn::Fullscreen => "fullscreen",
            PauseOn::Covered => "covered",
        }
    }
}

/// When the daemon stops a video nobody can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// What a compositor that veil follows has to show for a video to be
    /// paused (`--pause-on`).
    pub pause_on: PauseOn,
    /// How long a video's surface may go without being drawn before the video
    /// is released (`--release-unseen-after`, `wayland::unseen`). `None`:
    /// never.
    pub release_unseen_after: Option<Duration>,
}

/// What a wallpaper does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playback {
    Play,
    Pause,
}

/// The verdict for an output that shows `view`. An output the compositor
/// reports nothing about (`None`: never reported, gone, or the compositor's
/// IPC is lost) plays: a wallpaper is paused only when it is known to be
/// hidden.
pub fn decide(pause_on: PauseOn, view: Option<&OutputView>) -> Playback {
    let Some(view) = view else {
        return Playback::Play;
    };
    let hidden = match pause_on {
        PauseOn::Never => false,
        PauseOn::Fullscreen => view.fullscreen,
        PauseOn::Covered => view.fullscreen || view.covered,
    };
    if hidden {
        Playback::Pause
    } else {
        Playback::Play
    }
}

/// How long a wallpaper must stay hidden before it is paused.
pub const PAUSE_DELAY: Duration = Duration::from_secs(1);

/// An output whose wallpaper is paused, or is going to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hidden {
    /// Hidden since a moment that is not long enough ago; paused at this
    /// instant if it stays hidden.
    Since {
        pause_at: Instant,
    },
    Paused,
}

/// The latest view of every output, and which wallpapers are paused.
#[derive(Debug)]
pub struct Visibility {
    pause_on: PauseOn,
    delay: Duration,
    views: Snapshot,
    /// The outputs whose verdict is `Pause`, by name. An output that is not
    /// here plays.
    hidden: BTreeMap<String, Hidden>,
}

impl Visibility {
    pub fn new(pause_on: PauseOn, delay: Duration) -> Self {
        Self {
            pause_on,
            delay,
            views: Snapshot::new(),
            hidden: BTreeMap::new(),
        }
    }

    /// What hides a wallpaper enough to pause it.
    pub fn pause_on(&self) -> PauseOn {
        self.pause_on
    }

    /// Whether the wallpaper of `output` is paused now.
    pub fn playback(&self, output: &str) -> Playback {
        match self.hidden.get(output) {
            Some(Hidden::Paused) => Playback::Pause,
            Some(Hidden::Since { .. }) | None => Playback::Play,
        }
    }

    /// Takes in what the compositor reported at `now`. Returns the outputs
    /// whose wallpaper plays again because of it, which is at once. A
    /// wallpaper that is hidden by it is paused later, by [`Visibility::due`],
    /// if it is still hidden then.
    pub fn event(&mut self, event: &CompositorEvent, now: Instant) -> Vec<(String, Playback)> {
        let changed: Vec<String> = match event {
            CompositorEvent::WorkspaceChanged { output, workspace } => {
                self.view_mut(output).workspace = workspace.clone();
                vec![output.clone()]
            }
            CompositorEvent::FullscreenChanged { output, fullscreen } => {
                self.view_mut(output).fullscreen = *fullscreen;
                vec![output.clone()]
            }
            CompositorEvent::CoverageChanged { output, covered } => {
                self.view_mut(output).covered = *covered;
                vec![output.clone()]
            }
            CompositorEvent::OutputGone { output } => {
                self.views.remove(output);
                vec![output.clone()]
            }
            CompositorEvent::Disconnected { .. } => {
                self.views.clear();
                self.hidden.keys().cloned().collect()
            }
        };

        let mut resumed = Vec::new();
        for output in changed {
            match decide(self.pause_on, self.views.get(&output)) {
                Playback::Play => {
                    if self.hidden.remove(&output) == Some(Hidden::Paused) {
                        resumed.push((output, Playback::Play));
                    }
                }
                Playback::Pause => {
                    // Hidden since earlier: the delay runs from then.
                    self.hidden.entry(output).or_insert(Hidden::Since {
                        pause_at: now + self.delay,
                    });
                }
            }
        }
        resumed
    }

    /// The outputs whose wallpaper has been hidden long enough at `now`; they
    /// are paused from now on.
    pub fn due(&mut self, now: Instant) -> Vec<(String, Playback)> {
        let mut paused = Vec::new();
        for (output, hidden) in &mut self.hidden {
            if matches!(*hidden, Hidden::Since { pause_at } if pause_at <= now) {
                *hidden = Hidden::Paused;
                paused.push((output.clone(), Playback::Pause));
            }
        }
        paused
    }

    /// When [`Visibility::due`] has something to do next, if ever.
    pub fn next_due(&self) -> Option<Instant> {
        self.hidden
            .values()
            .filter_map(|hidden| match *hidden {
                Hidden::Since { pause_at } => Some(pause_at),
                Hidden::Paused => None,
            })
            .min()
    }

    /// The view of `output`, which is new if it was not reported before. A
    /// backend reports every field of an output it has not reported before, so
    /// the defaults of a new view are replaced by the events that follow.
    fn view_mut(&mut self, output: &str) -> &mut OutputView {
        self.views
            .entry(output.to_string())
            .or_insert_with(|| OutputView {
                workspace: String::new(),
                fullscreen: false,
                covered: false,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(fullscreen: bool, covered: bool) -> OutputView {
        OutputView {
            workspace: "1".into(),
            fullscreen,
            covered,
        }
    }

    #[test]
    fn decide_pauses_only_what_the_level_names() {
        use Playback::{Pause, Play};
        // Every level against every view, and no view at all.
        let views = [
            None,
            Some(view(false, false)),
            Some(view(false, true)),
            Some(view(true, false)),
            Some(view(true, true)),
        ];
        let table = [
            (PauseOn::Never, [Play, Play, Play, Play, Play]),
            (PauseOn::Fullscreen, [Play, Play, Play, Pause, Pause]),
            (PauseOn::Covered, [Play, Play, Pause, Pause, Pause]),
        ];
        for (pause_on, expected) in table {
            for (view, expected) in views.iter().zip(expected) {
                assert_eq!(
                    decide(pause_on, view.as_ref()),
                    expected,
                    "{pause_on:?} with {view:?}"
                );
            }
        }
    }

    #[test]
    fn the_workspace_alone_never_pauses() {
        for pause_on in [PauseOn::Never, PauseOn::Fullscreen, PauseOn::Covered] {
            let empty = OutputView {
                workspace: "special:scratch".into(),
                fullscreen: false,
                covered: false,
            };
            assert_eq!(decide(pause_on, Some(&empty)), Playback::Play);
        }
    }

    #[test]
    fn the_default_level_is_fullscreen() {
        assert_eq!(PauseOn::default(), PauseOn::Fullscreen);
    }

    const DELAY: Duration = Duration::from_millis(1000);

    /// A visibility at the default level, and the instant its test starts at.
    fn visibility() -> (Visibility, Instant) {
        (Visibility::new(PauseOn::Fullscreen, DELAY), Instant::now())
    }

    fn fullscreen(output: &str, on: bool) -> CompositorEvent {
        CompositorEvent::FullscreenChanged {
            output: output.into(),
            fullscreen: on,
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_hidden_wallpaper_is_paused_after_the_delay_and_resumed_at_once() {
        let (mut v, t0) = visibility();
        assert!(v.event(&fullscreen("DP-3", true), t0).is_empty());
        assert_eq!(v.playback("DP-3"), Playback::Play);
        assert_eq!(v.next_due(), Some(t0 + DELAY));

        // Not before the delay is over.
        assert!(v.due(t0 + ms(999)).is_empty());
        assert_eq!(v.due(t0 + DELAY), [("DP-3".into(), Playback::Pause)]);
        assert_eq!(v.playback("DP-3"), Playback::Pause);
        assert_eq!(v.next_due(), None);
        // Once.
        assert!(v.due(t0 + ms(5000)).is_empty());

        // Visible again: at once, without waiting for anything.
        assert_eq!(
            v.event(&fullscreen("DP-3", false), t0 + ms(6000)),
            [("DP-3".into(), Playback::Play)]
        );
        assert_eq!(v.playback("DP-3"), Playback::Play);
        assert_eq!(v.next_due(), None);
    }

    #[test]
    fn switching_back_and_forth_faster_than_the_delay_never_pauses() {
        let (mut v, t0) = visibility();
        // A fullscreen workspace and a normal one, every 300 ms, for 6 s.
        for step in 0..20u64 {
            let now = t0 + ms(300 * step);
            assert!(v.due(now).is_empty(), "paused at step {step}");
            let changes = v.event(&fullscreen("DP-3", step % 2 == 0), now);
            assert!(changes.is_empty(), "{changes:?} at step {step}");
        }
        assert_eq!(v.playback("DP-3"), Playback::Play);
    }

    #[test]
    fn the_delay_runs_from_the_moment_the_wallpaper_was_first_hidden() {
        // Hidden first by a fullscreen window, then also covered: the second
        // event does not start the delay again.
        let mut v = Visibility::new(PauseOn::Covered, DELAY);
        let t0 = Instant::now();
        v.event(&fullscreen("DP-3", true), t0);
        v.event(
            &CompositorEvent::CoverageChanged {
                output: "DP-3".into(),
                covered: true,
            },
            t0 + ms(600),
        );
        assert_eq!(v.due(t0 + DELAY), [("DP-3".into(), Playback::Pause)]);
        // The fullscreen window goes, the covering windows stay.
        assert!(v
            .event(&fullscreen("DP-3", false), t0 + ms(2000))
            .is_empty());
        assert_eq!(v.playback("DP-3"), Playback::Pause);
    }

    #[test]
    fn outputs_are_paused_and_resumed_each_on_its_own() {
        let (mut v, t0) = visibility();
        v.event(&fullscreen("DP-3", true), t0);
        v.event(&fullscreen("HDMI-A-1", true), t0 + ms(500));
        assert_eq!(v.due(t0 + DELAY), [("DP-3".into(), Playback::Pause)]);
        assert_eq!(v.next_due(), Some(t0 + ms(1500)));
        assert_eq!(v.due(t0 + ms(1500)), [("HDMI-A-1".into(), Playback::Pause)]);
        assert_eq!(
            v.event(&fullscreen("HDMI-A-1", false), t0 + ms(2000)),
            [("HDMI-A-1".into(), Playback::Play)]
        );
        assert_eq!(v.playback("DP-3"), Playback::Pause);
    }

    #[test]
    fn a_wallpaper_that_is_not_hidden_any_more_before_the_delay_is_not_paused() {
        let (mut v, t0) = visibility();
        v.event(&fullscreen("DP-3", true), t0);
        assert!(v.event(&fullscreen("DP-3", false), t0 + ms(500)).is_empty());
        assert_eq!(v.next_due(), None);
        assert!(v.due(t0 + ms(5000)).is_empty());
        // Hidden again: a new delay.
        v.event(&fullscreen("DP-3", true), t0 + ms(6000));
        assert!(v.due(t0 + ms(6999)).is_empty());
        assert_eq!(v.due(t0 + ms(7000)), [("DP-3".into(), Playback::Pause)]);
    }

    #[test]
    fn what_is_not_known_plays() {
        let (mut v, t0) = visibility();
        v.event(&fullscreen("DP-3", true), t0);
        v.event(&fullscreen("HDMI-A-1", true), t0);
        v.due(t0 + DELAY);
        // An output that is no longer reported plays again.
        assert_eq!(
            v.event(
                &CompositorEvent::OutputGone {
                    output: "DP-3".into()
                },
                t0 + ms(2000)
            ),
            [("DP-3".into(), Playback::Play)]
        );
        // So does every output when the compositor's IPC is lost.
        v.event(&fullscreen("DP-3", true), t0 + ms(2000));
        assert_eq!(
            v.event(
                &CompositorEvent::Disconnected {
                    reason: "closed".into()
                },
                t0 + ms(2500)
            ),
            [("HDMI-A-1".into(), Playback::Play)]
        );
        // The pause that was on its way for DP-3 is called off.
        assert_eq!(v.next_due(), None);
        assert!(v.due(t0 + ms(9000)).is_empty());
        assert_eq!(v.playback("DP-3"), Playback::Play);
        // An output that was never reported plays.
        assert_eq!(v.playback("eDP-1"), Playback::Play);
    }

    #[test]
    fn workspace_changes_are_taken_in_and_pause_nothing_by_themselves() {
        let (mut v, t0) = visibility();
        let changes = v.event(
            &CompositorEvent::WorkspaceChanged {
                output: "DP-3".into(),
                workspace: "2".into(),
            },
            t0,
        );
        assert!(changes.is_empty());
        assert_eq!(v.next_due(), None);
        // A fullscreen window on it that is left behind by a workspace switch:
        // the backend reports both changes, and only the second one counts.
        v.event(&fullscreen("DP-3", true), t0);
        v.event(
            &CompositorEvent::WorkspaceChanged {
                output: "DP-3".into(),
                workspace: "3".into(),
            },
            t0 + ms(100),
        );
        v.event(&fullscreen("DP-3", false), t0 + ms(100));
        assert!(v.due(t0 + ms(5000)).is_empty());
    }

    #[test]
    fn never_pauses_nothing() {
        let mut v = Visibility::new(PauseOn::Never, DELAY);
        let t0 = Instant::now();
        v.event(&fullscreen("DP-3", true), t0);
        v.event(
            &CompositorEvent::CoverageChanged {
                output: "DP-3".into(),
                covered: true,
            },
            t0,
        );
        assert_eq!(v.next_due(), None);
        assert!(v.due(t0 + ms(5000)).is_empty());
    }

    #[test]
    fn without_a_delay_a_pause_is_due_at_once() {
        let mut v = Visibility::new(PauseOn::Fullscreen, Duration::ZERO);
        let t0 = Instant::now();
        v.event(&fullscreen("DP-3", true), t0);
        assert_eq!(v.next_due(), Some(t0));
        assert_eq!(v.due(t0), [("DP-3".into(), Playback::Pause)]);
    }
}
