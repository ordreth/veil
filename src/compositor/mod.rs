//! What the compositor shows on each output, beyond veil's own surfaces.
//!
//! Whether a video wallpaper can be seen at all depends on the windows in
//! front of it: a fullscreen window hides it, and so do windows that cover the
//! whole output. Wayland itself does not tell a background surface that, so it
//! is read from the compositor's own IPC. Each compositor speaks its own
//! protocol; a backend turns it into the [`CompositorEvent`]s of this module,
//! which are the same for every compositor, and hands them to the
//! [`CompositorEvents`] the event loop runs with.
//!
//! Nothing here blocks the event loop and nothing starts a thread: a backend's
//! sockets are non-blocking `calloop` sources. Nothing here knows about Wayland
//! objects either, so a backend is tested against a fake compositor socket
//! (`tests/compositor.rs`).
//!
//! Hyprland ([`hyprland`]) and sway ([`sway`]) are supported; both are followed
//! the same way ([`ipc`]). Elsewhere [`detect`] finds no backend, and nothing is
//! reported: a wallpaper is taken to be visible.

pub mod coverage;
pub mod hyprland;
pub mod ipc;
pub mod sway;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use calloop::LoopHandle;

/// A change of what the compositor shows on an output, named as the
/// compositor names the output (the `wl_output` name, e.g. `DP-3`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositorEvent {
    /// The output shows another workspace (or reports one for the first time).
    WorkspaceChanged { output: String, workspace: String },
    /// A window is fullscreen on one of the output's visible workspaces, or no
    /// longer is.
    FullscreenChanged { output: String, fullscreen: bool },
    /// The visible windows on the output cover all of it, or no longer do.
    /// Only their rectangles count: a translucent window is taken to cover
    /// what is behind it.
    CoverageChanged { output: String, covered: bool },
    /// The compositor no longer reports the output (it was unplugged or
    /// disabled).
    OutputGone { output: String },
    /// The connection to the compositor's IPC is lost, so nothing is known any
    /// more about any output until the backend has reconnected; then every
    /// output is reported again, as at the start.
    Disconnected { reason: String },
}

/// Receives what a backend reports. Implemented by the data the event loop is
/// dispatched with; called on the event loop.
pub trait CompositorEvents {
    fn compositor_event(&mut self, event: CompositorEvent);
}

/// What an output shows, as far as a wallpaper cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputView {
    /// Name of the workspace that is shown on the output.
    pub workspace: String,
    /// Whether a window is fullscreen on a visible workspace of the output.
    pub fullscreen: bool,
    /// Whether the visible windows cover the whole output.
    pub covered: bool,
}

/// What every output shows, by output name.
pub type Snapshot = BTreeMap<String, OutputView>;

/// The events that lead from `old` to `new`, output by output in name order:
/// for every field that changed, and for every output that is new, all of its
/// fields. An output that is only in `old` is gone.
pub fn diff(old: &Snapshot, new: &Snapshot) -> Vec<CompositorEvent> {
    let mut events = Vec::new();
    for (output, view) in new {
        let before = old.get(output);
        if before.map(|b| &b.workspace) != Some(&view.workspace) {
            events.push(CompositorEvent::WorkspaceChanged {
                output: output.clone(),
                workspace: view.workspace.clone(),
            });
        }
        if before.map(|b| b.fullscreen) != Some(view.fullscreen) {
            events.push(CompositorEvent::FullscreenChanged {
                output: output.clone(),
                fullscreen: view.fullscreen,
            });
        }
        if before.map(|b| b.covered) != Some(view.covered) {
            events.push(CompositorEvent::CoverageChanged {
                output: output.clone(),
                covered: view.covered,
            });
        }
    }
    for output in old.keys().filter(|output| !new.contains_key(*output)) {
        events.push(CompositorEvent::OutputGone {
            output: output.clone(),
        });
    }
    events
}

/// The compositor IPC the daemon talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    Hyprland(hyprland::Sockets),
    /// sway, listening on this socket (``).
    Sway(PathBuf),
    /// None: nothing is reported, and every wallpaper counts as visible. The
    /// reason is logged.
    None(String),
}

impl Backend {
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Hyprland(_) => "Hyprland",
            Backend::Sway(_) => "sway",
            Backend::None(_) => "none",
        }
    }
}

/// Picks the backend from the environment (`lookup` reads a variable, as
/// `std::env::var_os` does): Hyprland when `HYPRLAND_INSTANCE_SIGNATURE` is
/// set (its sockets are in `$XDG_RUNTIME_DIR/hypr/<signature>/`, where
/// Hyprland 0.40 and later keep them), otherwise sway when `SWAYSOCK` is an
/// absolute path.
pub fn detect(lookup: impl Fn(&str) -> Option<OsString>) -> Backend {
    let Some(signature) = lookup("HYPRLAND_INSTANCE_SIGNATURE").filter(|s| !s.is_empty()) else {
        return match lookup("SWAYSOCK")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
        {
            Some(socket) if socket.is_absolute() => Backend::Sway(socket),
            Some(socket) => Backend::None(format!(
                "SWAYSOCK is not an absolute path: {}",
                socket.display()
            )),
            None => Backend::None(
                "no supported compositor found (Hyprland and sway are supported)".into(),
            ),
        };
    };
    // The signature becomes a directory name; anything else would point
    // somewhere else.
    let plain = Path::new(&signature)
        .file_name()
        .is_some_and(|name| name == signature.as_os_str());
    if !plain {
        return Backend::None(format!(
            "HYPRLAND_INSTANCE_SIGNATURE is not a plain name: {signature:?}"
        ));
    }
    let Some(runtime_dir) = lookup("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
    else {
        return Backend::None("XDG_RUNTIME_DIR is not set to an absolute path".into());
    };
    Backend::Hyprland(hyprland::sockets(&runtime_dir.join("hypr").join(signature)))
}

/// Starts `backend` on the event loop of `handle`; it reports to the loop's
/// [`CompositorEvents`]. This does not fail: a compositor whose IPC cannot be
/// reached now is tried again later, and without a backend nothing happens.
pub fn insert<'l, D>(handle: &LoopHandle<'l, D>, backend: Backend)
where
    D: CompositorEvents + 'l,
{
    match backend {
        Backend::Hyprland(sockets) => {
            tracing::info!("following Hyprland through {}", sockets.events.display());
            hyprland::insert(handle, sockets, ipc::Config::default());
        }
        Backend::Sway(socket) => {
            tracing::info!("following sway through {}", socket.display());
            sway::insert(handle, socket, ipc::Config::default());
        }
        Backend::None(reason) => {
            tracing::warn!(
                "{reason}; the daemon cannot tell when a wallpaper is hidden, and treats every \
                 wallpaper as visible"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(workspace: &str, fullscreen: bool, covered: bool) -> OutputView {
        OutputView {
            workspace: workspace.into(),
            fullscreen,
            covered,
        }
    }

    fn snapshot(outputs: &[(&str, OutputView)]) -> Snapshot {
        outputs
            .iter()
            .map(|(name, view)| (name.to_string(), view.clone()))
            .collect()
    }

    #[test]
    fn a_new_output_is_reported_in_full() {
        let new = snapshot(&[("DP-3", view("1", false, true))]);
        assert_eq!(
            diff(&Snapshot::new(), &new),
            [
                CompositorEvent::WorkspaceChanged {
                    output: "DP-3".into(),
                    workspace: "1".into()
                },
                CompositorEvent::FullscreenChanged {
                    output: "DP-3".into(),
                    fullscreen: false
                },
                CompositorEvent::CoverageChanged {
                    output: "DP-3".into(),
                    covered: true
                },
            ]
        );
    }

    #[test]
    fn only_what_changed_is_reported_and_each_output_on_its_own() {
        let old = snapshot(&[
            ("DP-3", view("1", false, false)),
            ("HDMI-A-1", view("2", false, false)),
        ]);
        let new = snapshot(&[
            ("DP-3", view("1", true, true)),
            ("HDMI-A-1", view("5", false, false)),
        ]);
        assert_eq!(
            diff(&old, &new),
            [
                CompositorEvent::FullscreenChanged {
                    output: "DP-3".into(),
                    fullscreen: true
                },
                CompositorEvent::CoverageChanged {
                    output: "DP-3".into(),
                    covered: true
                },
                CompositorEvent::WorkspaceChanged {
                    output: "HDMI-A-1".into(),
                    workspace: "5".into()
                },
            ]
        );
        assert!(diff(&new, &new).is_empty());
    }

    #[test]
    fn an_output_that_is_no_longer_reported_is_gone() {
        let old = snapshot(&[
            ("DP-3", view("1", false, false)),
            ("HDMI-A-1", view("2", false, false)),
        ]);
        let new = snapshot(&[("DP-3", view("1", false, false))]);
        assert_eq!(
            diff(&old, &new),
            [CompositorEvent::OutputGone {
                output: "HDMI-A-1".into()
            }]
        );
    }

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(var, _)| *var == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn hyprland_is_found_by_its_instance_signature() {
        let backend = detect(env(&[
            ("HYPRLAND_INSTANCE_SIGNATURE", "abc_123_456"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]));
        assert_eq!(
            backend,
            Backend::Hyprland(hyprland::Sockets {
                events: "/run/user/1000/hypr/abc_123_456/.socket2.sock".into(),
                requests: "/run/user/1000/hypr/abc_123_456/.socket.sock".into(),
            })
        );
        assert_eq!(backend.name(), "Hyprland");
    }

    #[test]
    fn sway_is_found_by_its_socket() {
        let backend = detect(env(&[
            ("SWAYSOCK", "/run/user/1000/sway-ipc.1000.42.sock"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]));
        assert_eq!(
            backend,
            Backend::Sway("/run/user/1000/sway-ipc.1000.42.sock".into())
        );
        assert_eq!(backend.name(), "sway");
        // Hyprland's signature wins: sway's socket may be left over from a
        // session started inside it.
        let backend = detect(env(&[
            ("HYPRLAND_INSTANCE_SIGNATURE", "abc"),
            ("SWAYSOCK", "/run/user/1000/sway-ipc.1000.42.sock"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]));
        assert_eq!(backend.name(), "Hyprland");
    }

    #[test]
    fn without_a_known_compositor_there_is_no_backend() {
        for vars in [
            &[("XDG_RUNTIME_DIR", "/run/user/1000")][..],
            &[
                ("HYPRLAND_INSTANCE_SIGNATURE", ""),
                ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ],
            // A signature that would lead out of the directory.
            &[
                ("HYPRLAND_INSTANCE_SIGNATURE", "../../etc"),
                ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ],
            &[
                ("HYPRLAND_INSTANCE_SIGNATURE", "a/b"),
                ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ],
            // No place for the sockets.
            &[("HYPRLAND_INSTANCE_SIGNATURE", "abc")],
            &[
                ("HYPRLAND_INSTANCE_SIGNATURE", "abc"),
                ("XDG_RUNTIME_DIR", "run/user"),
            ],
            // A sway socket that is not an absolute path, or empty.
            &[("SWAYSOCK", "sway-ipc.sock")],
            &[("SWAYSOCK", "")],
        ] {
            let backend = detect(env(vars));
            assert!(matches!(backend, Backend::None(_)), "{vars:?}: {backend:?}");
            assert_eq!(backend.name(), "none");
        }
    }
}
