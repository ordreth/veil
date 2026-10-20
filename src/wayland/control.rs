//! What the daemon does with a request from its control socket. The socket
//! itself (connections, limits, who may connect) is `ipc::server`; requests
//! arrive here parsed and validated.
//!
//! A `set` is not carried out here. The file is loaded on a thread of its own
//! (`engine::loader`), once for all the outputs it is for, and its client waits
//! in the queue (`queue::SetQueue`). When the loader delivers, each output's
//! engine is switched on the event loop (`load_finished`), and the client is
//! answered when the last of them is shown (`answers::Answers`).
//!
//! The daemon uses the same way for an output that appears while it runs: it
//! loads the default wallpaper for it, with nobody to answer. A `clear` goes
//! the same way too, with a wallpaper that has no file, so the latest request
//! wins on each output whichever of them it is.
//!
//! The other commands are answered at once: `pause`, `resume` and `toggle`
//! change `Screen::held` (`AppState::update_pause` applies it), `status` and
//! `outputs` report (`wayland/report.rs`), `quit` ends the event loop.

use std::time::Instant;

use calloop::channel::Sender;

use super::answers::{Part, Ticket, Verdict};
use super::binding::OutputKey;
use super::queue::{Finished, SetQueue, Submitted};
use super::render::{RenderState, Started, Switched};
use super::screen::{Screen, Starting};
use super::state::AppState;
use crate::engine::loader::{self, LoadRequest, Loaded};
use crate::engine::{Engine, EngineReport};
use crate::error::VeilError;
use crate::ipc::protocol::{ErrorCode, Request, Response};
use crate::ipc::report::Toggled;
use crate::ipc::server::{Reply, RequestHandler};
use crate::notify;
use crate::persist::{Wallpaper, Wallpapers};
use crate::scaling::ScaleOptions;

/// What the daemon remembers of a wallpaper once it is shown on an output
/// (`AppState::wallpapers`, and the state file in `AppState::store`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remember {
    /// Nothing: a load of the daemon's own shows what is remembered already.
    Nothing,
    /// Until the daemon stops (`veil set --no-persist`).
    ThisRun,
    /// Also in the state file, for the next daemon.
    Saved,
}

/// A wallpaper that is to become the wallpaper of an output, and what is
/// remembered of it when it does.
#[derive(Debug, Clone)]
pub struct Record {
    pub wallpaper: Wallpaper,
    pub remember: Remember,
    /// The number of the `set` for every output it comes from, while that
    /// waits to be shown before it becomes the default
    /// (`SetRequests::pending_default`).
    pub default: Option<u64>,
}

/// A video for every output that has loaded and is not shown anywhere yet.
struct PendingDefault {
    number: u64,
    wallpaper: Wallpaper,
    remember: Remember,
}

/// The `set` requests the daemon is working on.
pub struct SetRequests {
    queue: SetQueue<OutputKey, SetJob, Requester>,
    /// Where a loader thread delivers its result. The event loop reads the other
    /// end and calls `AppState::load_finished`.
    results: Sender<Loaded>,
    /// The load that is running.
    running: Option<Running>,
    /// A video for every output becomes the default once one output shows it
    /// (`AppState::remember_shown`), and not at all if it cannot start
    /// anywhere: whether it plays is only known then. A newer `set` for every
    /// output takes its place.
    pending_default: Option<PendingDefault>,
    /// How many videos for every output have waited to become the default.
    defaults_pending: u64,
}

/// What a request asks to load.
pub struct SetJob {
    /// `None` for a load of the daemon's own: the default wallpaper as it is
    /// when the load starts (a `set` for every output that was loaded before
    /// it may have changed it).
    wallpaper: Option<Wallpaper>,
    /// A `set` for every output: once it is loaded it is also the default
    /// wallpaper of outputs that appear later.
    for_every_output: bool,
    /// What is remembered of it.
    remember: Remember,
}

/// Who waits for a load.
pub enum Requester {
    Client(Reply),
    /// The daemon itself, for an output that appeared. Nobody is answered; what
    /// fails is logged and notified, like any failure of the daemon's own
    /// wallpaper.
    Daemon,
}

struct Running {
    wallpaper: Wallpaper,
    for_every_output: bool,
    remember: Remember,
    since: Instant,
    /// `AppState::frames_handled` when the load started.
    frames: u64,
    /// The outputs the engines are loaded for, in their order, each with the
    /// canvas its engine is prepared for (as in `LoadRequest::canvases`).
    targets: Vec<(OutputKey, Option<(u32, u32)>)>,
}

/// What the threads of the engines tell the event loop.
pub enum EngineEvent {
    /// An engine of the screen `key` reported through its signal with this
    /// serial: how its start went, that it has stopped for good, or that it
    /// has moved its picture.
    Report(OutputKey, u64, EngineReport),
    /// The thread that dropped retired engines is done (`Retirements`).
    Retired(u64),
}

impl SetRequests {
    pub fn new(results: Sender<Loaded>) -> Self {
        Self {
            queue: SetQueue::default(),
            results,
            running: None,
            pending_default: None,
            defaults_pending: 0,
        }
    }

    /// `wallpaper` becomes the default once one output shows it; returns the
    /// number its records carry.
    fn wait_to_become_default(&mut self, wallpaper: Wallpaper, remember: Remember) -> u64 {
        self.defaults_pending += 1;
        self.pending_default = Some(PendingDefault {
            number: self.defaults_pending,
            wallpaper,
            remember,
        });
        self.defaults_pending
    }
}

impl Requester {
    fn answer(self, response: Response) {
        if let Requester::Client(reply) = self {
            reply.send(response);
        }
    }

    /// The whole request failed (its file could not be loaded).
    fn fail(self, error: VeilError) {
        match self {
            Requester::Client(reply) => reply.send(Response::failed(&error)),
            Requester::Daemon => {
                tracing::error!("Failed to load the wallpaper for a new output: {error}");
                notify::notify_error_detached("Veil - Render Error", &error.to_string());
            }
        }
    }
}

/// What `pause`, `resume` and `toggle` do with `Screen::held`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    Pause,
    Resume,
    /// `Resume` if every output it is for is held, `Pause` otherwise.
    Toggle,
}

impl Hold {
    /// Whether the outputs are held after this, given whether each of them is
    /// held now.
    fn held_after(self, held_now: impl IntoIterator<Item = bool>) -> bool {
        match self {
            Hold::Pause => true,
            Hold::Resume => false,
            Hold::Toggle => !held_now.into_iter().all(|held| held),
        }
    }
}

fn remember(persist: bool) -> Remember {
    match persist {
        true => Remember::Saved,
        false => Remember::ThisRun,
    }
}

impl RequestHandler for AppState {
    fn handle_request(&mut self, request: Request, reply: Reply) {
        match request {
            Request::Ping {} => reply.send(Response::ok()),
            Request::Set {
                output,
                input,
                mode,
                bg_color,
                persist,
            } => {
                let options = ScaleOptions {
                    mode: mode.unwrap_or(self.defaults.mode),
                    bg_color: bg_color.unwrap_or(self.defaults.bg_color),
                };
                let wallpaper = Wallpaper::file(input, options);
                self.set_wallpaper(output.as_deref(), wallpaper, remember(persist), reply)
            }
            Request::Clear {
                output,
                bg_color,
                persist,
            } => {
                let wallpaper = Wallpaper::color(ScaleOptions {
                    mode: self.defaults.mode,
                    bg_color: bg_color.unwrap_or(self.defaults.bg_color),
                });
                self.set_wallpaper(output.as_deref(), wallpaper, remember(persist), reply)
            }
            Request::Pause { output } => self.hold(output.as_deref(), Hold::Pause, reply),
            Request::Resume { output } => self.hold(output.as_deref(), Hold::Resume, reply),
            Request::Toggle { output } => self.hold(output.as_deref(), Hold::Toggle, reply),
            Request::Status {} => reply.send(Response::report(&self.status())),
            Request::Outputs {} => reply.send(Response::report(&self.output_list())),
            Request::Quit {} => {
                tracing::info!("a client asked the daemon to quit; shutting down");
                // Answered now: the event loop ends after this batch of
                // events, and the client waits until the daemon is gone.
                reply.send(Response::ok());
                self.exit = true;
            }
        }
    }
}

impl AppState {
    /// Replaces the wallpaper of `output`, or of every output, with
    /// `wallpaper` (a file for `set`, a colour for `clear`). The client is
    /// answered once the wallpaper is shown everywhere, or cannot be.
    fn set_wallpaper(
        &mut self,
        output: Option<&str>,
        wallpaper: Wallpaper,
        remember: Remember,
        reply: Reply,
    ) {
        let targets = match output {
            Some(name) if self.draws_on(name) => vec![OutputKey::Named(name.to_string())],
            Some(name) => return reply.send(self.not_driven(name)),
            // Every output the daemon draws on, also those that are unplugged.
            None => self.screen_keys(),
        };
        if targets.is_empty() {
            return reply.send(Response::error(
                ErrorCode::Failed,
                "there is no output to show the wallpaper on",
            ));
        }

        let job = SetJob {
            wallpaper: Some(wallpaper),
            for_every_output: output.is_none(),
            remember,
        };
        self.submit(targets, job, Requester::Client(reply));
    }

    /// `pause`, `resume` or `toggle` for `output`, or for every output the
    /// daemon has a wallpaper for. Answered at once: what a video does about
    /// it follows from the screen's render state.
    fn hold(&mut self, output: Option<&str>, hold: Hold, reply: Reply) {
        let keys = match output {
            Some(name) => {
                let key = OutputKey::Named(name.to_string());
                if self.screen(&key).is_none() {
                    return reply.send(match self.draws_on(name) {
                        true => Response::error(
                            ErrorCode::UnknownOutput,
                            format!("output '{name}' has not been connected yet"),
                        ),
                        false => self.not_driven(name),
                    });
                }
                vec![key]
            }
            None => self.screen_keys(),
        };
        if keys.is_empty() {
            return reply.send(Response::error(
                ErrorCode::Failed,
                "there is no output the daemon draws on",
            ));
        }

        let held = hold.held_after(
            keys.iter()
                .map(|key| self.screen(key).is_some_and(|screen| screen.held)),
        );
        tracing::info!(
            "{} the wallpaper of {} (veil {})",
            if held { "holding" } else { "releasing" },
            describe_keys(keys.iter()),
            match hold {
                Hold::Pause => "pause",
                Hold::Resume => "resume",
                Hold::Toggle => "toggle",
            }
        );
        for key in &keys {
            if let Some(screen) = self.screen_mut(key) {
                screen.held = held;
            }
            self.update_pause(key);
        }
        reply.send(match hold {
            Hold::Toggle => Response::report(&Toggled { paused: held }),
            Hold::Pause | Hold::Resume => Response::ok(),
        });
    }

    /// The answer to a command for an output the daemon does not draw on.
    fn not_driven(&self, name: &str) -> Response {
        Response::error(
            ErrorCode::UnknownOutput,
            format!(
                "output '{name}' is not driven by this daemon; {}",
                self.describe_outputs()
            ),
        )
    }

    /// Whether `veil set --output name` is accepted: the output is one the
    /// daemon draws on, now or when it is back. With `--output`, the daemon
    /// also takes a wallpaper for a requested output it has not seen yet.
    fn draws_on(&self, name: &str) -> bool {
        let requested = self.binding.requested();
        if !requested.is_empty() {
            return requested.iter().any(|r| r == name);
        }
        self.screens.iter().any(|s| s.key.name() == Some(name))
    }

    fn describe_outputs(&self) -> String {
        let requested = self.binding.requested();
        let names: Vec<&str> = if requested.is_empty() {
            self.screens.iter().filter_map(|s| s.key.name()).collect()
        } else {
            requested.iter().map(String::as_str).collect()
        };
        if names.is_empty() {
            "it draws on no named output".into()
        } else {
            format!("it draws on {}", names.join(", "))
        }
    }

    /// An output appeared that the daemon has not drawn on before: it shows its
    /// background colour, and gets its wallpaper (`AppState::wallpapers`: its
    /// own, else the default) when that is loaded. Not if a `set` for it is on
    /// its way already.
    pub(super) fn load_wallpaper_of(&mut self, key: OutputKey) {
        if self.wallpapers.for_output(key.name()).is_none()
            || self.set_requests.queue.is_pending_for(&key)
        {
            return;
        }
        let job = SetJob {
            wallpaper: None,
            for_every_output: false,
            remember: Remember::Nothing,
        };
        self.submit(vec![key], job, Requester::Daemon);
    }

    fn submit(&mut self, targets: Vec<OutputKey>, job: SetJob, requester: Requester) {
        let Submitted { start, superseded } =
            self.set_requests.queue.submit(targets, job, requester);
        for earlier in superseded {
            earlier.answer(superseded_response());
        }
        if let Some((targets, job)) = start {
            self.start_load(targets, job);
        }
    }

    /// Hands a load to a loader thread. If no thread can be started (or there
    /// is nothing to load), the request ends here and the next one in the queue
    /// is tried.
    fn start_load(&mut self, mut targets: Vec<OutputKey>, mut job: SetJob) {
        loop {
            // A load of the daemon's own (for one output) takes the output's
            // wallpaper as it is now: a `set` loaded before it may have
            // changed it.
            let wallpaper = job.wallpaper.or_else(|| {
                let key = targets.first()?;
                self.wallpapers.for_output(key.name()).cloned()
            });
            let error = match wallpaper {
                Some(wallpaper) => {
                    match self.spawn_load(targets, wallpaper, job.for_every_output, job.remember) {
                        Ok(()) => return,
                        Err(e) => Some(e),
                    }
                }
                // Only a load of the daemon's own comes without a file, and
                // the output had one before such a load was queued.
                None => None,
            };

            let Finished { wanted, next } = self.set_requests.queue.finished();
            if let Some((requester, _)) = wanted {
                match error {
                    Some(error) => requester.fail(error),
                    None => requester.answer(Response::ok()),
                }
            }
            match next {
                Some(next) => (targets, job) = next,
                None => return,
            }
        }
    }

    fn spawn_load(
        &mut self,
        targets: Vec<OutputKey>,
        wallpaper: Wallpaper,
        for_every_output: bool,
        remember: Remember,
    ) -> Result<(), VeilError> {
        let targets: Vec<_> = targets
            .into_iter()
            .map(|key| {
                // Lets the loader scale an image for each surface already. If a
                // surface changes meanwhile, it is prepared again
                // (`prepared_for_these_surfaces`).
                let canvas = self.buffer_size(&key);
                (key, canvas)
            })
            .collect();
        let request = LoadRequest {
            input: wallpaper.input.clone(),
            options: wallpaper.options,
            canvases: targets.iter().map(|(_, canvas)| *canvas).collect(),
        };
        let results = self.set_requests.results.clone();
        loader::spawn(request, move |loaded| {
            // The receiving end is gone when the daemon is shutting down.
            let _ = results.send(loaded);
        })
        .map_err(|e| VeilError::Load(format!("cannot start a loader thread: {e}")))
        .inspect_err(|e| tracing::error!("{e}"))?;

        tracing::debug!(
            "loading {wallpaper} for {} off the event loop",
            describe_keys(targets.iter().map(|(key, _)| key))
        );
        self.set_requests.running = Some(Running {
            wallpaper,
            for_every_output,
            remember,
            since: Instant::now(),
            frames: self.frames_handled,
            targets,
        });
        Ok(())
    }

    /// A loader thread has delivered. On every output it is still wanted on,
    /// its engine replaces the current one (or the error goes to the client and
    /// the current wallpapers stay); on the others it is dropped. Called on the
    /// event loop, which is the only place an engine is switched.
    pub(super) fn load_finished(&mut self, loaded: Loaded) {
        let Some(loaded) = self.prepared_for_these_surfaces(loaded) else {
            return;
        };

        let running = self.set_requests.running.take();
        let Finished { wanted, next } = self.set_requests.queue.finished();
        if let Some(running) = &running {
            tracing::debug!(
                "{} {} after {} ms off the event loop; {} frame callbacks were handled meanwhile{}",
                running.wallpaper,
                if loaded.is_ok() { "loaded" } else { "failed" },
                running.since.elapsed().as_millis(),
                self.frames_handled - running.frames,
                if wanted.is_some() {
                    ""
                } else {
                    "; dropped, newer requests replaced it"
                }
            );
        }
        // The next load runs while this result is put on screen.
        if let Some((targets, job)) = next {
            self.start_load(targets, job);
        }

        let Some(running) = running else {
            return;
        };
        // A file for every output is what outputs that appear later show. An
        // image (or a colour) is, once it has loaded, even if newer requests
        // have taken all of the current outputs meanwhile: it was decoded. A
        // video only once an output shows it: a file that GStreamer cannot
        // play loads all the same.
        let mut default = None;
        if let (true, Ok(engines)) = (running.for_every_output, &loaded) {
            if engines.iter().any(|engine| engine.is_dynamic()) {
                default = Some(
                    self.set_requests
                        .wait_to_become_default(running.wallpaper.clone(), running.remember),
                );
            } else {
                self.set_requests.pending_default = None;
                self.remember_everywhere(running.remember, &running.wallpaper);
            }
        }
        let Some((requester, wanted)) = wanted else {
            return;
        };
        let engines = match loaded {
            Ok(engines) => engines,
            Err(e) => return requester.fail(e),
        };

        // The engines of outputs a newer request has taken were never attached,
        // and are dropped here.
        let shown: Vec<(OutputKey, Engine)> = running
            .targets
            .into_iter()
            .map(|(key, _)| key)
            .zip(engines)
            .filter(|(key, _)| wanted.contains(key))
            .collect();
        let tickets: Vec<Option<Ticket>> = match requester {
            Requester::Client(reply) => {
                let outputs = shown.iter().map(|(key, _)| key.to_string()).collect();
                match self.answers.open(reply, outputs) {
                    Ok(tickets) => tickets.into_iter().map(Some).collect(),
                    Err(reply) => {
                        return reply.send(Response::error(
                            ErrorCode::Failed,
                            "nothing was loaded for the outputs",
                        ))
                    }
                }
            }
            Requester::Daemon => shown.iter().map(|_| None).collect(),
        };
        let record = Record {
            wallpaper: running.wallpaper,
            remember: running.remember,
            default,
        };
        for ((key, engine), ticket) in shown.into_iter().zip(tickets) {
            self.show(key, engine, record.clone(), ticket);
        }
    }

    /// Makes `engine` the wallpaper of `key`; `record` says which one it is.
    /// `ticket` is the client's part of the request for this output (`None`
    /// for a load of the daemon's own); it is done when the wallpaper is
    /// shown, or cannot be.
    fn show(&mut self, key: OutputKey, mut engine: Engine, record: Record, ticket: Option<Ticket>) {
        // The latest request wins, also over one whose video is attached and
        // still waits for its first frame.
        if let Some(earlier) = self
            .screen_mut(&key)
            .and_then(|s| s.starting.take())
            .and_then(|starting| starting.ticket)
        {
            self.part_done(earlier, Part::Superseded);
        }

        if self.screen(&key).is_none() {
            // A requested output that has not been connected yet: the wallpaper
            // is kept for it. (An output without a name that is gone will not
            // be recognised again, so there is nothing to keep it for.)
            if matches!(key, OutputKey::Named(_)) {
                tracing::info!("{key} is not connected; its wallpaper is shown once it is");
                engine.detach();
                let render = RenderState::new(engine, self.report_sink(&key));
                self.screens.push(Screen {
                    key: key.clone(),
                    render,
                    surface: None,
                    starting: None,
                    shows: record.wallpaper.clone(),
                    held: false,
                });
                self.remember_shown(&key, record);
            }
            return self.part_maybe_done(ticket, Part::Done);
        }

        let switching = Instant::now();
        let Some(switched) = self.switch_engine(&key, engine) else {
            return self.part_maybe_done(ticket, Part::Done);
        };
        tracing::debug!(
            "switching the wallpaper of {key} held the event loop for {} ms",
            switching.elapsed().as_millis()
        );
        match switched {
            // Done when the wallpaper it replaced is gone from the screen.
            Ok(Switched::Shown | Switched::Stored) => {
                self.remember_shown(&key, record);
                self.retire_engines(&key, ticket);
            }
            // Done when its first frame is there (`engine_event`).
            Ok(Switched::Starting) => {
                self.retire_engines(&key, None);
                if let Some(screen) = self.screen_mut(&key) {
                    screen.starting = Some(Starting { ticket, record });
                }
            }
            Err(e) => {
                self.retire_engines(&key, None);
                self.part_failed(&key, ticket, e);
            }
        }
    }

    /// Checks that the loaded engines were prepared for the surfaces as they
    /// are now. If one of them has changed size or scale meanwhile (or has
    /// appeared), the engines go back to a loader thread to prepare for it, and
    /// `None` is returned: `load_finished` is called again when that is done.
    /// Otherwise an image would be decoded and scaled once more inside the
    /// event loop.
    fn prepared_for_these_surfaces(&mut self, loaded: Loaded) -> Option<Loaded> {
        let engines = match loaded {
            Ok(engines) => engines,
            Err(e) => return Some(Err(e)),
        };
        let Some(running) = &self.set_requests.running else {
            return Some(Ok(engines));
        };
        let wanted = self.set_requests.queue.loading_targets();
        let changed: Vec<Option<(u32, u32)>> = running
            .targets
            .iter()
            .map(|(key, canvas)| {
                let now = self.buffer_size(key);
                (wanted.contains(key) && now.is_some() && now != *canvas)
                    .then_some(now)
                    .flatten()
            })
            .collect();
        if changed.iter().all(Option::is_none) || engines.len() != changed.len() {
            return Some(Ok(engines));
        }

        tracing::debug!(
            "a surface changed while the wallpaper was loaded; preparing it again for {:?}",
            changed.iter().flatten().collect::<Vec<_>>()
        );
        let results = self.set_requests.results.clone();
        let spawned = loader::spawn_prepare(engines, changed.clone(), move |loaded| {
            let _ = results.send(loaded);
        });
        match spawned {
            Ok(()) => {
                if let Some(running) = &mut self.set_requests.running {
                    for ((_, canvas), now) in running.targets.iter_mut().zip(changed) {
                        if now.is_some() {
                            *canvas = now;
                        }
                    }
                }
                None
            }
            Err(e) => Some(Err(VeilError::Load(format!(
                "cannot start a loader thread: {e}"
            )))),
        }
    }

    /// A thread of an engine has reported: a video has its first frame or
    /// cannot have one, has stopped for good or has moved its picture, or an
    /// engine that was replaced has been shut down.
    pub(super) fn engine_event(&mut self, event: EngineEvent) {
        let (key, serial, started) = match event {
            EngineEvent::Retired(id) => {
                if let Some(ticket) = self.retirements.finished(id) {
                    self.part_done(ticket, Part::Done);
                }
                return;
            }
            EngineEvent::Report(key, serial, EngineReport::Lost(error)) => {
                return self.wallpaper_lost(&key, serial, error);
            }
            EngineEvent::Report(key, serial, EngineReport::Moved) => {
                return self.video_moved(&key, serial);
            }
            EngineEvent::Report(key, serial, EngineReport::Started(started)) => {
                (key, serial, started)
            }
        };

        let taking_over = Instant::now();
        let Some(outcome) = self.with_target(&key, |render, target| {
            render.started(serial, started, target)
        }) else {
            return;
        };
        if !matches!(outcome, Started::Ignored) {
            tracing::debug!(
                "the start report of an engine of {key} held the event loop for {} ms",
                taking_over.elapsed().as_millis()
            );
        }
        match outcome {
            Started::Switched => {
                tracing::debug!(
                    "the new wallpaper of {key} has its first frame and replaces the previous one"
                );
                let ticket = self.starting_shown(&key);
                self.retire_engines(&key, ticket);
            }
            Started::RolledBack(e) => {
                tracing::warn!(
                    "the new wallpaper of {key} could not be started; the previous one stays: {e}"
                );
                self.retire_engines(&key, None);
                let ticket = self
                    .screen_mut(&key)
                    .and_then(|s| s.starting.take())
                    .and_then(|starting| starting.ticket);
                self.part_failed(&key, ticket, e);
            }
            Started::Shown => {
                tracing::debug!("the wallpaper of {key} has its first frame");
            }
            Started::Failed(e) => {
                tracing::error!("Failed to show the wallpaper on {key}: {e}");
                self.not_shown_at_startup(&key, &e);
                // Not from the event loop: a notification daemon may take its time.
                notify::notify_error_detached("Veil - Render Error", &e.to_string());
            }
            Started::Ignored => {}
        }
    }

    /// The engine of `key` with this serial has reported that it stopped
    /// showing its wallpaper for good: a video failed while it was playing, and
    /// restarting its pipeline did not help. If it is still the wallpaper, it
    /// is detached, and the surface shows the video's background colour. The
    /// user is told once; the engine's own restarts were only logged.
    fn wallpaper_lost(&mut self, key: &OutputKey, serial: u64, error: VeilError) {
        let lost = self
            .screen_mut(key)
            .is_some_and(|screen| screen.render.lost(serial));
        if !lost {
            tracing::debug!("an engine that is no longer the wallpaper has stopped: {error}");
            return;
        }
        tracing::error!("The wallpaper of {key} is no longer shown: {error}");
        notify::notify_error_detached("Veil - Video Playback Error", &error.to_string());
    }

    /// Shuts down the engines that the render state of `key` has let go of. A
    /// static engine is dropped here; one that holds a pipeline on a thread of
    /// its own. `ticket` is the part of a request whose wallpaper replaced
    /// them: it is done when they are gone, because until then the old video
    /// may still cover the new wallpaper.
    fn retire_engines(&mut self, key: &OutputKey, ticket: Option<Ticket>) {
        let engines = match self.screen_mut(key) {
            Some(screen) => screen.render.take_retired(),
            None => Vec::new(),
        };
        let events = self.engine_events.clone();
        let done_now = self
            .retirements
            .retire(key.clone(), engines, ticket, move |id| {
                // The receiving end is gone when the daemon is shutting down.
                let _ = events.send(EngineEvent::Retired(id));
            });
        self.part_maybe_done(done_now, Part::Done);
    }

    /// The surface of `key` is being given up, and its render state has
    /// detached its engines. Before the surface is destroyed, every engine that
    /// was shown on it and is still being shut down must be gone as well: a
    /// video sink renders into subsurfaces of the surface and holds a raw
    /// pointer to it.
    ///
    /// `promoted` says that an engine which was waiting for its first frame has
    /// become the wallpaper, to be shown on the next surface. Its client's part
    /// is done like a `set` for an output that is away.
    pub(super) fn surface_released(&mut self, key: &OutputKey, promoted: bool) {
        let ticket = match promoted {
            true => self.starting_shown(key),
            false => None,
        };
        self.retire_engines(key, ticket);
        for ticket in self.retirements.wait_for(key) {
            self.part_done(ticket, Part::Done);
        }
    }

    /// The daemon is shutting down. A client whose video still waits for its
    /// first frame is told that it will not be shown; releasing the surface
    /// would otherwise answer it like a `set` for an output that is away.
    pub(super) fn abandon_starting_switches(&mut self) {
        let tickets: Vec<Ticket> = self
            .screens
            .iter_mut()
            .filter_map(|screen| screen.starting.take()?.ticket)
            .collect();
        for ticket in tickets {
            self.part_done(ticket, Part::Failed("the daemon is shutting down".into()));
        }
    }

    /// Blocks until no engine is being shut down any more, and finishes the
    /// parts of requests that waited for that.
    pub(super) fn wait_for_retired_engines(&mut self) {
        for ticket in self.retirements.wait() {
            self.part_done(ticket, Part::Done);
        }
    }

    /// The wallpaper that was starting on `key` has become its wallpaper: it
    /// is remembered, and the client's part (if a client waits) is returned.
    fn starting_shown(&mut self, key: &OutputKey) -> Option<Ticket> {
        let Starting { ticket, record } = self.screen_mut(key)?.starting.take()?;
        self.remember_shown(key, record);
        ticket
    }

    /// `record` is now the wallpaper of `key`: shown there, or stored to be
    /// shown when the output is back. An output without a name is not
    /// recognised again, so nothing is kept for it. The first output that
    /// shows a video for every output makes it the default.
    fn remember_shown(&mut self, key: &OutputKey, record: Record) {
        if let Some(number) = record.default {
            if let Some(pending) = self
                .set_requests
                .pending_default
                .take_if(|pending| pending.number == number)
            {
                self.remember_everywhere(pending.remember, &pending.wallpaper);
            }
        }
        if let Some(screen) = self.screen_mut(key) {
            screen.shows = record.wallpaper.clone();
        }
        if let Some(name) = key.name() {
            let Record {
                wallpaper,
                remember,
                ..
            } = record;
            self.remember(remember, |wallpapers| {
                wallpapers.shown_on(name, wallpaper.clone())
            });
        }
    }

    /// A `set` for every output has loaded `wallpaper` (an image), or shown it
    /// on its first output (a video): it is the default of
    /// outputs that appear later (see `Wallpapers::set_everywhere`). The
    /// outputs it is shown on follow, each with `remember_shown`.
    fn remember_everywhere(&mut self, remember: Remember, wallpaper: &Wallpaper) {
        let names: Vec<String> = self
            .screens
            .iter()
            .filter_map(|screen| screen.key.name().map(String::from))
            .collect();
        let screens: Vec<&str> = names.iter().map(String::as_str).collect();
        let requested = self.binding.requested().to_vec();
        self.remember(remember, |wallpapers| {
            wallpapers.set_everywhere(wallpaper.clone(), &screens, &requested)
        });
    }

    /// Applies `change` to what the daemon remembers: to what it goes by while
    /// it runs, and, for a wallpaper that is saved, to the state file.
    fn remember(&mut self, remember: Remember, change: impl Fn(&mut Wallpapers)) {
        match remember {
            Remember::Nothing => {}
            Remember::ThisRun => change(&mut self.wallpapers),
            Remember::Saved => {
                change(&mut self.wallpapers);
                self.store.update(change);
            }
        }
    }

    /// One output's part of a client's request has ended. The client is
    /// answered when it was the last part.
    fn part_done(&mut self, ticket: Ticket, part: Part) {
        if let Some((reply, verdict)) = self.answers.done(ticket, part) {
            reply.send(match verdict {
                Verdict::Ok => Response::ok(),
                Verdict::Superseded => superseded_response(),
                Verdict::Failed(message) => Response::error(ErrorCode::Failed, message),
            });
        }
    }

    fn part_maybe_done(&mut self, ticket: Option<Ticket>, part: Part) {
        if let Some(ticket) = ticket {
            self.part_done(ticket, part);
        }
    }

    /// The wallpaper could not be shown on `key`. A client is told through its
    /// ticket and nobody else (the previous wallpaper stays); for a wallpaper
    /// the daemon loaded by itself the error is logged and notified.
    fn part_failed(&mut self, key: &OutputKey, ticket: Option<Ticket>, error: VeilError) {
        match ticket {
            Some(ticket) => self.part_done(ticket, Part::Failed(error.to_string())),
            None => {
                tracing::error!("Failed to show the wallpaper on {key}: {error}");
                notify::notify_error_detached("Veil - Render Error", &error.to_string());
            }
        }
    }
}

/// The answer to a client whose request was replaced by newer ones.
fn superseded_response() -> Response {
    Response::error(
        ErrorCode::Superseded,
        "replaced by a newer `veil set` before it was shown",
    )
}

fn describe_keys<'a>(keys: impl Iterator<Item = &'a OutputKey>) -> String {
    keys.map(OutputKey::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_resumes_only_what_is_all_held() {
        for (held_now, pause, resume, toggle) in [
            (&[][..], true, false, false),
            (&[true], true, false, false),
            (&[false], true, false, true),
            (&[true, true], true, false, false),
            (&[true, false], true, false, true),
            (&[false, false], true, false, true),
        ] {
            let after = |hold: Hold| hold.held_after(held_now.iter().copied());
            assert_eq!(after(Hold::Pause), pause, "{held_now:?}");
            assert_eq!(after(Hold::Resume), resume, "{held_now:?}");
            assert_eq!(after(Hold::Toggle), toggle, "{held_now:?}");
        }
    }
}
