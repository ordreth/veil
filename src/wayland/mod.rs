pub mod answers;
pub mod binding;
mod control;
pub mod queue;
pub mod render;
mod report;
pub mod retire;
pub mod scale;
pub mod screen;
pub mod size;
pub mod state;
pub mod surface;
mod target;
pub mod unseen;
mod visibility;

use calloop::{
    signals::{Signal, Signals},
    timer::{TimeoutAction, Timer},
    EventLoop,
};
use calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::{
    compositor::CompositorState, output::OutputState, registry::RegistryState,
    shell::wlr_layer::LayerShell, shm::Shm,
};
use wayland_client::{globals::registry_queue_init, Connection};

use crate::compositor;
use crate::config_file::Settings;
use crate::engine::Source;
use crate::error::VeilError;
use crate::ipc::{instance, protocol, server};
use crate::persist::{self, Store, Wallpaper, Wallpapers};
use crate::policy::{Visibility, PAUSE_DELAY};
use crate::service::{self, NotifyState, Readiness};
use answers::Answers;
use binding::OutputBinding;
use control::SetRequests;
use retire::Retirements;
use state::{AppState, Startup};
use surface::ScaleGlobals;
use target::PinnedDisplay;

/// Runs the daemon with `settings` (its command line over its configuration
/// file): a wallpaper on each of `settings.outputs` (every output if it is
/// empty), from the file `settings.input` for every output, else from the
/// configuration file and what was saved, else the solid background. A video
/// is paused while its output shows what `policy.pause_on` names, as far as
/// the compositor's IPC tells, and released while the compositor does not
/// draw its surface (`policy.release_unseen_after`).
///
/// The file is opened only after SIGINT/SIGTERM are blocked on this thread,
/// because the threads an engine spawns (GStreamer streaming threads, the bus
/// thread) inherit the signal mask. A thread that does not block them would
/// receive the signal and kill the process with the default action, skipping
/// the clean shutdown.
pub fn run(settings: Settings) -> Result<(), VeilError> {
    let Settings {
        input,
        outputs,
        defaults,
        policy,
        configured,
    } = settings;
    // First of all: without a place for the control socket the daemon does not
    // start, and neither does a second daemon for the same compositor. Both say
    // so before anything is connected or loaded. The socket exists from here
    // on; a client that connects before the event loop runs is served when it
    // does. The guard removes the socket file when `run` returns, with or
    // without an error (it is declared first, so it is dropped last).
    let started = std::time::Instant::now();
    let socket = protocol::socket_path()?;
    let (listener, _socket_guard) = instance::claim(&socket)?;

    // Creating the signalfd source blocks the signals on the calling thread.
    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM])
        .map_err(|e| VeilError::EventLoop(e.to_string()))?;
    // What the last daemon saved. A file that cannot be read is no reason not
    // to start.
    let store = Store::open(persist::state_path());
    let mut wallpapers = store.saved().clone();
    // A file that cannot be a wallpaper is refused here, before the daemon
    // connects to the compositor.
    let (input, sources) = match input {
        Some(input) => {
            let source = Source::open(&input, defaults)?;
            let wallpaper = Wallpaper::file(input, defaults);
            // The `--input` is for every output, as a `veil set` would be.
            wallpapers.set_everywhere(wallpaper.clone(), &[], &outputs);
            (Some(wallpaper.clone()), vec![(wallpaper, Some(source))])
        }
        None => {
            // The configuration file's wallpapers win over the saved ones, as
            // an `--input` would; outputs it says nothing about keep theirs.
            configured.apply(&mut wallpapers, &outputs);
            if !configured.is_empty() {
                tracing::info!(
                    "no --input given; showing the wallpapers of the configuration file{}",
                    match configured.every {
                        Some(_) => "",
                        None => ", and the saved ones on other outputs",
                    }
                );
            } else if wallpapers == Wallpapers::default() {
                tracing::info!(
                    "no --input given and no wallpaper saved; showing a solid background until \
                     `veil set`"
                );
            } else {
                tracing::info!("no --input given; showing the saved wallpapers");
            }
            (None, Vec::new())
        }
    };

    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init::<AppState>(&conn)?;
    let qh = event_queue.handle();

    let compositor_state =
        CompositorState::bind(&globals, &qh).map_err(|e| VeilError::SctkGlobal(e.to_string()))?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| VeilError::SctkGlobal(e.to_string()))?;
    let layer_shell =
        LayerShell::bind(&globals, &qh).map_err(|e| VeilError::SctkGlobal(e.to_string()))?;
    let scale_globals = ScaleGlobals::bind(&globals, &qh);

    // Loader threads deliver the wallpapers they have loaded through this
    // channel; the loop switches to them in `load_finished`.
    let (loaded_sender, loaded) = calloop::channel::channel();

    // The threads of the engines report through this one: a video that has its
    // first frame (or cannot have one), and an engine that has been shut down.
    let (engine_events, engine_event_source) = calloop::channel::channel();

    let mut app_state = AppState {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor_state,
        shm,
        layer_shell,
        scale_globals,
        conn: conn.clone(),
        display: PinnedDisplay::new(&conn),
        screens: Vec::new(),
        binding: OutputBinding::new(outputs.clone()),
        exit: false,
        defaults,
        wallpapers,
        store,
        startup: Some(Startup {
            from_input: input.is_some(),
            sources,
            error: None,
        }),
        qh: qh.clone(),
        set_requests: SetRequests::new(loaded_sender),
        frames_handled: 0,
        retirements: Retirements::default(),
        answers: Answers::default(),
        engine_events,
        visibility: Visibility::new(policy.pause_on, PAUSE_DELAY),
        pause_timer_armed: false,
        release_unseen_after: policy.release_unseen_after,
        unseen_timer_armed: false,
        loop_handle: None,
        readiness: None,
        started,
    };

    event_queue.roundtrip(&mut app_state)?;
    event_queue.roundtrip(&mut app_state)?;

    // A missing output is an error only at startup, where it is most likely a
    // typo in `--output`. Once an output has been bound, the daemon survives its
    // removal and waits for it (see `binding::OutputBinding`).
    if !app_state.binding.ever_bound() {
        return Err(match outputs.is_empty() {
            false => VeilError::OutputNotFound {
                requested: outputs,
                available: app_state.output_names(),
            },
            true => VeilError::NoOutputs,
        });
    }
    let missing: Vec<&str> = outputs
        .iter()
        .filter(|name| !app_state.screens.iter().any(|s| s.key.name() == Some(name)))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        tracing::warn!(
            "output {} not connected; waiting for {} (connected: {})",
            missing.join(", "),
            if missing.len() == 1 { "it" } else { "them" },
            app_state.output_names().join(", ")
        );
    }

    event_queue.roundtrip(&mut app_state)?;

    // From here on an output that appears gets its wallpaper from a loader
    // thread, and the start-up wallpaper's decoded image is no longer held
    // here (only by the engines, until they have drawn).
    if let Some(Startup {
        error: Some(error), ..
    }) = app_state.startup.take()
    {
        return Err(error);
    }
    // The daemon has started with its `--input`, or the wallpapers of its
    // configuration file: that is what the next one shows as well.
    match input {
        Some(wallpaper) => app_state
            .store
            .update(|saved| saved.set_everywhere(wallpaper, &[], &outputs)),
        None if !configured.is_empty() => app_state
            .store
            .update(|saved| configured.apply(saved, &outputs)),
        None => {}
    }

    let mut event_loop: EventLoop<AppState> =
        EventLoop::try_new().map_err(|e| VeilError::EventLoop(e.to_string()))?;
    let loop_handle = event_loop.handle();

    WaylandSource::new(conn.clone(), event_queue)
        .insert(loop_handle.clone())
        .map_err(|e| VeilError::EventLoop(e.to_string()))?;

    loop_handle
        .insert_source(signals, |_event, _, app_state| {
            tracing::info!("received termination signal, shutting down");
            app_state.exit = true;
        })
        .map_err(|e| VeilError::EventLoop(e.to_string()))?;

    loop_handle
        .insert_source(loaded, |event, _, app_state| {
            if let calloop::channel::Event::Msg(loaded) = event {
                app_state.load_finished(loaded);
            }
        })
        .map_err(|e| VeilError::EventLoop(e.to_string()))?;

    loop_handle
        .insert_source(engine_event_source, |event, _, app_state| {
            if let calloop::channel::Event::Msg(event) = event {
                app_state.engine_event(event);
            }
        })
        .map_err(|e| VeilError::EventLoop(e.to_string()))?;

    server::insert(&loop_handle, listener, server::ServerConfig::default())?;

    // What the compositor shows over the wallpapers (workspaces, fullscreen
    // windows), from its own IPC, if it is one veil knows.
    compositor::insert(
        &loop_handle,
        compositor::detect(|name| std::env::var_os(name)),
    );

    // If the unit has a watchdog: the service manager is told that the loop
    // is alive, from the loop itself.
    if let Some(interval) = service::watchdog_interval() {
        insert_watchdog(&loop_handle, interval)?;
    }

    // For the timers handlers set (the pause of a hidden wallpaper). Only while
    // the loop runs: a handle keeps the loop's sources alive, and they go with
    // the loop, before the state, however the loop ends.
    app_state.loop_handle = Some(loop_handle);
    // A video shown before the loop ran is watched from now on.
    app_state.watch_unseen();
    // The service manager is told the daemon is ready when every output found
    // so far shows its wallpaper (`check_ready`, after each redraw).
    let readiness = Readiness::new(
        app_state
            .screens
            .iter()
            .filter(|screen| screen.surface.is_some())
            .map(|screen| screen.key.clone())
            .collect(),
    );
    service::notify(&[NotifyState::Status(&readiness.status())]);
    app_state.readiness = Some(readiness);
    let ended = loop {
        if app_state.exit {
            break Ok(());
        }
        // The first frame and every size or scale change are drawn once per batch
        // of events, after all of them have been handled. The requests are flushed
        // by the Wayland source before the loop sleeps.
        app_state.redraw_if_needed();
        app_state.check_ready();
        if let Err(e) = event_loop.dispatch(None, &mut app_state) {
            break Err(dispatch_error(e));
        }
    };
    app_state.loop_handle = None;
    ended?;

    // `app_state` is dropped here, as on every other way out of this function,
    // and that shuts the engines down before the surfaces go (`Drop for
    // AppState`).
    Ok(())
}

/// Sends `WATCHDOG=1` every `interval` (half the unit's `WatchdogSec=`). The
/// timer runs on the event loop, so a loop that is stuck stops the pings and
/// systemd kills and restarts the daemon.
fn insert_watchdog(
    handle: &calloop::LoopHandle<'static, AppState>,
    interval: std::time::Duration,
) -> Result<(), VeilError> {
    tracing::debug!(
        "telling the service manager every {} ms that the event loop is alive",
        interval.as_millis()
    );
    handle
        .insert_source(Timer::from_duration(interval), move |_, _, _| {
            service::notify(&[NotifyState::Watchdog]);
            TimeoutAction::ToDuration(interval)
        })
        .map_err(|e| VeilError::EventLoop(e.to_string()))?;
    Ok(())
}

/// Maps a failed event-loop dispatch to an error. A closed or reset socket means
/// the compositor is gone (it exited or crashed). The daemon cannot reconnect,
/// because every Wayland object it holds died with the connection, so it exits
/// with [`VeilError::ConnectionLost`] (exit status 5). A restart would not help
/// either: the session is over, and the next one starts its own daemon, so the
/// systemd unit does not restart on that status.
fn dispatch_error(error: calloop::Error) -> VeilError {
    use std::io::ErrorKind;

    // The loop wraps the Wayland source's I/O error in other errors; look for it
    // along the chain of causes.
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(current) = cause {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            let lost = matches!(
                io.kind(),
                ErrorKind::BrokenPipe
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::NotConnected
                    | ErrorKind::UnexpectedEof
            );
            return if lost {
                VeilError::ConnectionLost(io.to_string())
            } else {
                VeilError::EventLoop(io.to_string())
            };
        }
        cause = current.source();
    }
    VeilError::EventLoop(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    /// The shape the loop reports: the source's error wrapped in `OtherError`.
    fn wrapped(io: Error) -> calloop::Error {
        calloop::Error::OtherError(Box::new(calloop::Error::IoError(io)))
    }

    #[test]
    fn closed_socket_is_reported_as_a_lost_connection() {
        for kind in [
            ErrorKind::BrokenPipe,
            ErrorKind::ConnectionReset,
            ErrorKind::ConnectionAborted,
            ErrorKind::NotConnected,
            ErrorKind::UnexpectedEof,
        ] {
            for error in [
                wrapped(Error::from(kind)),
                calloop::Error::IoError(Error::from(kind)),
            ] {
                let error = dispatch_error(error);
                assert!(
                    matches!(error, VeilError::ConnectionLost(_)),
                    "{kind:?}: {error:?}"
                );
            }
        }
    }

    #[test]
    fn other_dispatch_errors_keep_their_cause() {
        // A Wayland protocol error reaches the loop as EPROTO.
        let error = dispatch_error(wrapped(Error::from_raw_os_error(71)));
        assert!(matches!(error, VeilError::EventLoop(_)), "{error:?}");
        assert!(error.to_string().contains("os error 71"), "{error}");

        let error = dispatch_error(wrapped(Error::from(ErrorKind::PermissionDenied)));
        assert!(matches!(error, VeilError::EventLoop(_)), "{error:?}");

        let error = dispatch_error(calloop::Error::InvalidToken);
        assert!(matches!(error, VeilError::EventLoop(_)), "{error:?}");
    }
}
