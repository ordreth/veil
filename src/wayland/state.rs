use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::protocols::wp::{
        fractional_scale::v1::client::{
            wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
            wp_fractional_scale_v1::{self, WpFractionalScaleV1},
        },
        viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        WaylandSurface,
    },
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    delegate_noop,
    protocol::{wl_output, wl_surface},
    Connection, Dispatch, Proxy, QueueHandle,
};

use super::answers::{Answers, Ticket};
use super::binding::{OutputBinding, OutputKey};
use super::control::{EngineEvent, SetRequests};
use super::render::{Engine, RenderState, Switched};
use super::retire::Retirements;
use super::scale::ScaleInputs;
use super::screen::{OutputSurface, Screen};
use super::size::{self, OutputSize};
use super::surface::{self, ScaleGlobals};
use super::target::{PinnedDisplay, SurfaceTarget};
use super::unseen::StallWatch;
use crate::engine::image::ImageEngine;
use crate::engine::{RenderTarget, ReportSink, Source};
use crate::error::VeilError;
use crate::ipc::server::Reply;
use crate::notify;
use crate::persist::{Store, Wallpaper, Wallpapers};
use crate::policy::Visibility;
use crate::scaling::ScaleOptions;
use crate::service::{self, NotifyState, Progress, Readiness};

pub struct AppState {
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub compositor_state: CompositorState,
    pub shm: Shm,
    pub layer_shell: LayerShell,
    pub scale_globals: ScaleGlobals,
    pub conn: Connection,
    /// Where the `wl_display*` for a video sink comes from. It keeps the
    /// connection open until the process ends, whatever is dropped when.
    pub(super) display: PinnedDisplay,
    /// One per output the daemon draws on, with its wallpaper, and its surface
    /// while the output is there. A named output keeps its screen while it is
    /// unplugged.
    pub screens: Vec<Screen>,
    /// Which outputs are wanted, and which `wl_output` hosts the surface of
    /// which screen right now. `binding.output(key)` is `Some` exactly when
    /// that screen's `surface` is.
    pub binding: OutputBinding<wl_output::WlOutput>,
    pub exit: bool,
    /// `--mode`/`--bg-color` of the daemon, used by `veil set` requests that omit them.
    pub defaults: ScaleOptions,
    /// What an output that is new to the daemon shows: its own wallpaper (the
    /// latest `veil set` shown on it), else the default (the daemon's
    /// `--input`, then the latest `veil set` for every output), else the
    /// solid `--bg-color` background. At start-up, what was saved.
    pub wallpapers: Wallpapers,
    /// The state file: the wallpapers saved for the next daemon.
    pub store: Store,
    /// Until the event loop runs: what the outputs found at start-up are given.
    pub startup: Option<Startup>,
    pub qh: QueueHandle<Self>,
    /// The `veil set` requests that are being loaded or wait for the loader.
    pub set_requests: SetRequests,
    /// How many frame callbacks have been handled, on all surfaces. Only a
    /// diagnostic: the log says how many arrived while a wallpaper was loaded,
    /// which shows that the event loop kept running.
    pub frames_handled: u64,
    /// Engines that were replaced and are being shut down on threads of their
    /// own, by the screen they were shown on, with the parts of requests that
    /// wait for that.
    pub retirements: Retirements<OutputKey, Ticket>,
    /// The clients whose `veil set` is carried out on several outputs, until
    /// the last of them is done.
    pub answers: Answers<Reply>,
    /// Where the threads of the engines report to the event loop.
    pub engine_events: calloop::channel::Sender<EngineEvent>,
    /// What the compositor shows on each output, and which wallpapers are
    /// paused because of it (`wayland/visibility.rs`).
    pub visibility: Visibility,
    /// Whether the timer that pauses hidden wallpapers is armed.
    pub pause_timer_armed: bool,
    /// How long a playing video's surface may go undrawn before the video is
    /// released (`--release-unseen-after`); `None`: never.
    pub release_unseen_after: Option<std::time::Duration>,
    /// Whether the timer that looks for videos nobody sees is armed.
    pub unseen_timer_armed: bool,
    /// The event loop, once it runs: for timers that are set from a handler.
    pub loop_handle: Option<calloop::LoopHandle<'static, AppState>>,
    /// The outputs found at start-up whose first frame the service manager
    /// waits for (`READY=1`, `service::Readiness`). Set when the event loop
    /// starts, and `None` again once the daemon is ready.
    pub readiness: Option<Readiness<OutputKey>>,
    /// When the daemon started (`veil status`).
    pub started: std::time::Instant,
}

/// The wallpapers of the outputs found at start-up: the `--input` for every
/// output, or for each what the configuration file gives it, else what was
/// saved. They get their engines at once, on this thread, before the event
/// loop runs. Each file is opened (and an image decoded) once, however many
/// outputs show it.
pub struct Startup {
    /// The files opened so far, each with its source, or `None` if it cannot
    /// be shown. The `--input` is opened before the daemon connects to the
    /// compositor, the others when an output needs them.
    pub sources: Vec<(Wallpaper, Option<Source>)>,
    /// Whether the wallpapers are the `--input`, which the daemon cannot start
    /// without. A wallpaper of the configuration file or a saved one that
    /// cannot be shown is only logged: its outputs show its background colour
    /// (`veil config check` finds the first kind before a start).
    pub from_input: bool,
    /// The first error building an engine from the `--input`. Start-up fails
    /// with it.
    pub error: Option<VeilError>,
}

impl Startup {
    /// An engine for `key` showing `wallpaper`, or `None` if there is none.
    fn engine(&mut self, key: &OutputKey, wallpaper: &Wallpaper) -> Option<Engine> {
        let Some(input) = &wallpaper.input else {
            // Nothing to open: only a colour (`veil clear`).
            return Some(Box::new(ImageEngine::solid(wallpaper.options.bg_color)));
        };
        let index = match self.sources.iter().position(|(w, _)| w == wallpaper) {
            Some(index) => index,
            None => {
                let opened = Source::open(input, wallpaper.options)
                    .inspect_err(|e| {
                        tracing::warn!(
                            "the wallpaper of {key} cannot be shown, showing its background \
                             colour instead: {e}"
                        )
                    })
                    .ok();
                self.sources.push((wallpaper.clone(), opened));
                self.sources.len() - 1
            }
        };
        let source = self.sources.get(index)?.1.as_ref()?;
        match source.engine() {
            Ok(engine) => Some(engine),
            Err(e) if self.from_input => {
                self.error.get_or_insert(e);
                None
            }
            Err(e) => {
                tracing::warn!(
                    "the wallpaper of {key} cannot be shown, showing its background colour \
                     instead: {e}"
                );
                None
            }
        }
    }
}

/// How far the screen `key` is with its first wallpaper, for `READY=1`: shown
/// once its engine is attached and has its first frame (an image when it is
/// drawn, a video when it has taken over), gone without a surface.
fn startup_progress(screens: &[Screen], key: &OutputKey) -> Progress {
    match screens.iter().find(|screen| &screen.key == key) {
        Some(Screen {
            surface: Some(_),
            render,
            ..
        }) if render.is_attached() && !render.is_starting() => Progress::Shown,
        Some(Screen {
            surface: Some(_), ..
        }) => Progress::Waiting,
        _ => Progress::Gone,
    }
}

/// The way out, whichever it is: a signal, an error from the event loop (the
/// compositor is gone), an error before the loop ran. The engines go first,
/// then the surfaces, as on every other release of one (`release_surface`): a
/// video sink has subsurfaces of its surface, and one that is being shut down
/// on a thread of its own is waited for. The fields of the state are dropped
/// only after that, in an order that then no longer matters.
impl Drop for AppState {
    fn drop(&mut self) {
        service::notify(&[NotifyState::Stopping, NotifyState::Status("Stopping")]);
        self.abandon_starting_switches();
        for key in self.screen_keys() {
            self.release_surface(&key);
        }
        self.wait_for_retired_engines();
        // The connection is not closed by veil (`PinnedDisplay`), so what is
        // still buffered has to be sent here. It fails if the compositor is
        // gone, and then nobody is left to read it.
        let _ = self.conn.flush();
    }
}

impl AppState {
    pub(super) fn screen(&self, key: &OutputKey) -> Option<&Screen> {
        self.screens.iter().find(|screen| &screen.key == key)
    }

    pub(super) fn screen_mut(&mut self, key: &OutputKey) -> Option<&mut Screen> {
        self.screens.iter_mut().find(|screen| &screen.key == key)
    }

    pub(super) fn screen_keys(&self) -> Vec<OutputKey> {
        self.screens
            .iter()
            .map(|screen| screen.key.clone())
            .collect()
    }

    /// The screen whose layer surface is `surface`.
    fn key_of_surface(&self, surface: &wl_surface::WlSurface) -> Option<OutputKey> {
        let screen = self
            .screens
            .iter()
            .find(|screen| screen.shows_on(surface))?;
        Some(screen.key.clone())
    }

    fn check_and_bind_output(&mut self, qh: &QueueHandle<Self>, output: &wl_output::WlOutput) {
        let Some(info) = self.output_state.info(output) else {
            return;
        };
        let Some(key) = self.binding.offer(output, info.name.as_deref()) else {
            return;
        };

        let returning = self.screen(&key).is_some();
        tracing::info!(
            "binding to output {key} ({})",
            if returning {
                "it is back; its wallpaper is shown again"
            } else if self.binding.requested().is_empty() {
                "a new output; every output gets a wallpaper"
            } else {
                "one of the outputs it was given"
            }
        );
        if !returning {
            self.add_screen(key.clone());
        }

        let (layer, scaling) = surface::create_layer_surface(
            &self.compositor_state,
            &self.layer_shell,
            &self.scale_globals,
            qh,
            output,
        );
        let surface = OutputSurface {
            scaling,
            layer,
            pool: None,
            scale_inputs: ScaleInputs {
                output_hint: Some(info.scale_factor),
                ..ScaleInputs::default()
            },
            pending_configure: None,
            frame_callback_pending: false,
            stall: StallWatch::default(),
        };
        let scale = surface.scale_inputs.effective(surface.has_viewport());
        if let Some(screen) = self.screen_mut(&key) {
            screen.surface = Some(surface);
            screen.render.set_scale(scale);
        }
    }

    /// A screen for an output the daemon has not drawn on before. While the
    /// daemon starts up it gets its wallpaper at once. Later it shows the
    /// background colour first, and its wallpaper once that has been loaded
    /// off the event loop.
    fn add_screen(&mut self, key: OutputKey) {
        let wallpaper = self.wallpapers.for_output(key.name()).cloned();
        let options = wallpaper.as_ref().map_or(self.defaults, |w| w.options);
        let background = || -> Engine { Box::new(ImageEngine::solid(options.bg_color)) };
        let color = Wallpaper::color(options);
        let (engine, shows, load_later) = match (&mut self.startup, wallpaper) {
            (Some(startup), Some(wallpaper)) => match startup.engine(&key, &wallpaper) {
                Some(engine) => (engine, wallpaper, false),
                None => (background(), color, false),
            },
            (Some(_), None) => (background(), color, false),
            // Only a colour: there is nothing to load.
            (None, Some(wallpaper)) if wallpaper.input.is_none() => {
                (background(), wallpaper, false)
            }
            (None, _) => (background(), color, true),
        };
        let render = RenderState::new(engine, self.report_sink(&key));
        self.screens.push(Screen {
            key: key.clone(),
            render,
            surface: None,
            starting: None,
            shows,
            held: false,
        });
        self.apply_current_playback(&key);
        if load_later {
            self.load_wallpaper_of(key);
        }
    }

    /// Where the engines of the screen `key` report to.
    pub(super) fn report_sink(&self, key: &OutputKey) -> ReportSink {
        let events = self.engine_events.clone();
        let key = key.clone();
        std::sync::Arc::new(move |serial, report| {
            // The receiving end is gone when the daemon is shutting down.
            let _ = events.send(EngineEvent::Report(key.clone(), serial, report));
        })
    }

    /// Gives up the layer surface of `key`, because its output is gone or the
    /// compositor closed it. The daemon keeps running, and so do the other
    /// outputs. A named output keeps its screen: `check_and_bind_output`
    /// creates a new surface when it is announced again, which starts from
    /// scratch (first configure, first frame). An output without a name cannot
    /// be recognised again, so its screen goes too.
    pub(super) fn release_surface(&mut self, key: &OutputKey) {
        let Some(screen) = self.screen_mut(key) else {
            return;
        };
        if screen.surface.is_none() {
            return;
        }
        // The engines go first: a video sink renders into subsurfaces of this
        // surface and holds a raw pointer to it. This also forgets the size, so
        // nothing is drawn until the next surface is configured.
        let promoted = screen.render.release();
        // So does every engine that is still being shut down on a thread of its
        // own after it was shown here: this waits for them.
        self.surface_released(key, promoted);
        // Then the viewport and fractional-scale objects, the surface itself
        // and its buffers (`OutputSurface` drops them in that order). The
        // callback dies with the surface, so the next video requests one.
        if let Some(screen) = self.screen_mut(key) {
            screen.surface = None;
        }
        if matches!(key, OutputKey::Unnamed(_)) {
            // Every engine of it is detached, so it holds nothing.
            self.screens.retain(|screen| &screen.key != key);
        }
    }

    /// Size in physical pixels of the buffers of the surface of `key` (its
    /// logical size times its scale), or `None` while it has no configured
    /// surface.
    pub(super) fn buffer_size(&self, key: &OutputKey) -> Option<(u32, u32)> {
        self.screen(key)?.buffer_size()
    }

    /// Sizing information of the output that hosts the surface of `key`, if
    /// the compositor has sent it.
    fn output_size(&self, key: &OutputKey) -> Option<OutputSize> {
        let output = self.binding.output(key)?;
        Some(OutputSize::from(&self.output_state.info(output)?))
    }

    /// Re-evaluates the scale of `key` after its `scale_inputs` changed. If
    /// the surface is already showing something, `redraw_if_needed` draws it at
    /// the new scale.
    fn update_scale(&mut self, key: &OutputKey) {
        let Some(screen) = self.screen_mut(key) else {
            return;
        };
        let Some(surface) = &screen.surface else {
            return;
        };
        let scale = surface.scale_inputs.effective(surface.has_viewport());
        let previous = screen.render.scale();
        if scale == previous {
            return;
        }
        if screen.render.is_attached() {
            tracing::info!("scale of output {key} changed from {previous} to {scale}");
        } else {
            tracing::debug!("scale of output {key} is {scale}");
        }
        screen.render.set_scale(scale);
    }

    /// Draws every wallpaper that is not shown yet, or whose size or scale
    /// changed since it was last drawn. Called once per batch of events, never
    /// from the event handlers.
    pub fn redraw_if_needed(&mut self) {
        for key in self.screen_keys() {
            self.redraw(&key);
        }
    }

    fn redraw(&mut self, key: &OutputKey) {
        let Some(screen) = self.screen(key) else {
            return;
        };
        if !screen.render.needs_draw() {
            return;
        }
        let first = !screen.render.is_attached();
        if let Some((width, height)) = screen.render.size() {
            tracing::debug!(
                "{} {key} at {width}x{height}, scale {}",
                if first { "drawing" } else { "redrawing" },
                screen.render.scale()
            );
        }
        let drawn = self.with_target(key, |render, target| render.draw(target));
        match drawn {
            Some(Ok(())) | None => {}
            Some(Err(e)) if first => {
                tracing::error!("Failed to show the wallpaper on {key}: {e}");
                self.not_shown_at_startup(key, &e);
                // Not from the event loop: a notification daemon may take its time.
                notify::notify_error_detached("Veil - Render Error", &e.to_string());
            }
            Some(Err(e)) => tracing::error!("Failed to redraw the wallpaper on {key}: {e}"),
        }
    }

    /// Tells the service manager that the daemon is ready (`READY=1`), once
    /// every output found at start-up shows its wallpaper, has failed to, or
    /// is gone. Called once per batch of events, after the redraw; after that
    /// it does nothing.
    pub fn check_ready(&mut self) {
        let Some(readiness) = &mut self.readiness else {
            return;
        };
        let screens = &self.screens;
        let Some(status) = readiness.update(|key| startup_progress(screens, key)) else {
            return;
        };
        self.readiness = None;
        tracing::debug!("ready: {status}");
        service::notify(&[NotifyState::Ready, NotifyState::Status(&status)]);
    }

    /// The wallpaper of `key` could not be shown. If the daemon is still
    /// starting, it does not wait for that output any more.
    pub(super) fn not_shown_at_startup(&mut self, key: &OutputKey, error: &VeilError) {
        if let Some(readiness) = &mut self.readiness {
            readiness.failed(key, error);
        }
    }

    /// Runs `op` on the render state of `key` with a target that draws on its
    /// layer surface at the current size and scale, or with `None` while it has
    /// no configured surface. Afterwards, a video that plays there gets its
    /// frame-callback loop, and the surface of a video that does not play is
    /// committed once. `None` if there is no such screen.
    pub(super) fn with_target<R>(
        &mut self,
        key: &OutputKey,
        op: impl FnOnce(&mut RenderState, Option<&mut dyn RenderTarget>) -> R,
    ) -> Option<R> {
        let (shm, display, qh) = (&self.shm, &self.display, &self.qh);
        let Screen {
            render, surface, ..
        } = self.screens.iter_mut().find(|screen| &screen.key == key)?;
        let (Some(surface), Some(size)) = (surface.as_mut(), render.size()) else {
            return Some(op(render, None));
        };
        let buffer = render.scale().physical_size(size);
        let wl_surface = surface.layer.wl_surface().clone();
        // Pending state: it is applied by the commit that attaches the new buffer.
        sync_buffer_mapping(surface, &wl_surface, render, size);
        let viewport = surface.has_viewport();
        let mut target = SurfaceTarget {
            shm,
            pool: &mut surface.pool,
            surface: &wl_surface,
            display,
            buffer,
            viewport,
        };
        let result = op(render, Some(&mut target));
        let plays_video = render.plays_video();
        if plays_video {
            // The frame-callback loop: it commits at the next frame if it runs.
            request_frame(qh, surface, &wl_surface);
        } else if render.video_attached() {
            // `waylandsink` places its subsurface with `wl_subsurface.set_position`,
            // which is state of this surface and takes effect with a commit of it
            // (a paused video, or one that waits to replace an image).
            wl_surface.commit();
        }
        if !plays_video {
            // What it saw of a video that no longer plays says nothing.
            surface.stall = StallWatch::default();
        }
        if plays_video {
            self.watch_unseen();
        }
        Some(result)
    }

    /// Applies a layer-surface configure of `configured` (where `0` means "client
    /// decides") to the surface of `key`. If the size cannot be resolved yet, it
    /// is kept in `pending_configure` until the output reports its size. The
    /// wallpaper is drawn by `redraw_if_needed`, after the whole batch of events.
    fn apply_configure(&mut self, key: &OutputKey, configured: (u32, u32)) {
        let output = self.output_size(key);
        let Some(screen) = self.screen_mut(key) else {
            return;
        };
        let Some(surface) = &mut screen.surface else {
            return;
        };
        let Some((width, height)) = size::resolve_surface_size(configured, output.as_ref()) else {
            tracing::warn!(
                "compositor sent a {}x{} configure for {key} and the output size is not \
                 known yet; waiting for the next configure or output update",
                configured.0,
                configured.1
            );
            surface.pending_configure = Some(configured);
            return;
        };
        surface.pending_configure = None;
        tracing::debug!(
            "surface size of {key} {width}x{height}, scale {}",
            screen.render.scale()
        );

        if (width, height) != configured {
            tracing::info!(
                "compositor sent a {}x{} configure for {key}; using the output size {width}x{height}",
                configured.0,
                configured.1
            );
        }

        screen.render.set_size((width, height));
    }

    /// Names of all outputs the compositor has announced so far.
    pub fn output_names(&self) -> Vec<String> {
        self.output_state
            .outputs()
            .filter_map(|output| self.output_state.info(&output)?.name)
            .collect()
    }

    /// Replaces the wallpaper of `key`. With a configured surface the new
    /// engine is attached immediately, at the current size and scale. An image
    /// is on screen when this returns; a video takes the place of the previous
    /// wallpaper when it has its first frame (`engine_event`). On failure the
    /// previous engine stays. Without a surface, the engine is kept, holding no
    /// resources, and shown once there is one. `None` if there is no screen
    /// for `key`.
    ///
    /// The engines this replaces are left in the screen's `render` for the
    /// caller to retire.
    pub(super) fn switch_engine(
        &mut self,
        key: &OutputKey,
        engine: Engine,
    ) -> Option<Result<Switched, VeilError>> {
        let screen = self.screen(key)?;
        if screen.surface.is_none() || screen.render.size().is_none() {
            // The output is unplugged, or its surface has not been configured yet.
            tracing::info!(
                "{key} has no surface right now; its new wallpaper is shown once it has"
            );
        }
        self.with_target(key, |render, target| render.switch(engine, target))
    }

    /// A video on the surface of `key` has moved its picture from its own
    /// thread (`EngineReport::Moved`, the frame size became known): the surface
    /// is committed, so the new place of the video's subsurface takes effect. A
    /// report of an engine that is no longer attached there is ignored.
    pub(super) fn video_moved(&mut self, key: &OutputKey, serial: u64) {
        let Some(screen) = self.screen(key) else {
            return;
        };
        let Some(surface) = &screen.surface else {
            return;
        };
        if !screen.render.attached_with(serial) {
            return;
        }
        tracing::debug!("the video of {key} has moved; committing its surface");
        surface.layer.wl_surface().commit();
    }
}

/// Tells the compositor how buffers map to the surface's logical `size`:
/// through the viewport destination, or (without `wp_viewporter`) through the
/// integer buffer scale. This is pending state, applied by the next commit.
fn sync_buffer_mapping(
    surface: &OutputSurface,
    wl_surface: &wl_surface::WlSurface,
    render: &RenderState,
    size: (u32, u32),
) {
    match &surface.scaling.viewport {
        Some(viewport) => {
            if let (Ok(width @ 1..), Ok(height @ 1..)) =
                (i32::try_from(size.0), i32::try_from(size.1))
            {
                viewport.set_destination(width, height);
            }
        }
        // `set_buffer_scale` exists since wl_surface version 3.
        None if wl_surface.version() >= 3 => {
            wl_surface.set_buffer_scale(render.scale().ceil());
        }
        None => {}
    }
}

/// Keeps the frame-callback loop of a video going on one surface: requests a
/// callback and commits, so the compositor presents the video subsurface. At
/// most one callback is outstanding per surface, so switching between videos
/// never starts a second loop, and each output's loop runs at its own refresh
/// rate. While a video is released because nobody sees it, the callback
/// outstanding is how the daemon learns that the surface is drawn again
/// (`RenderState::seen`).
pub(super) fn request_frame(
    qh: &QueueHandle<AppState>,
    surface: &mut OutputSurface,
    wl_surface: &wl_surface::WlSurface,
) {
    if !surface.frame_callback_pending {
        wl_surface.frame(qh, wl_surface.clone());
        surface.frame_callback_pending = true;
        wl_surface.commit();
    }
}

impl CompositorHandler for AppState {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let Some(key) = self.key_of_surface(surface) else {
            return;
        };
        tracing::debug!("compositor prefers buffer scale {new_factor} for {key}");
        if let Some(own) = self.screen_mut(&key).and_then(|s| s.surface.as_mut()) {
            own.scale_inputs.integer = Some(new_factor);
        }
        self.update_scale(&key);
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        // Dispatched at the vblank of the surface's own output (e.g. 144 Hz on
        // a 144 Hz monitor, 60 Hz on the one next to it).
        let qh = &self.qh;
        let Some(screen) = self.screens.iter_mut().find(|s| s.shows_on(surface)) else {
            return;
        };
        let Some(own) = &mut screen.surface else {
            return;
        };
        own.frame_callback_pending = false;
        self.frames_handled += 1;
        if screen.render.seen() {
            // `redraw_if_needed` attaches it, after this batch of events.
            tracing::info!(
                "the wallpaper of {} is drawn again; starting its video",
                screen.key
            );
        }
        if screen.render.plays_video() {
            // Commit the parent surface so the compositor presents the video
            // subsurface. Hyprland needs it: without it the video lagged and
            // showed parts of older frames (seen by the project owner on
            // 2026-10-16, after the loop had been removed).
            request_frame(qh, own, surface);
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for AppState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.check_and_bind_output(qh, &output);
    }

    fn update_output(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.check_and_bind_output(qh, &output);

        let Some(key) = self.binding.key(&output).cloned() else {
            return;
        };
        let scale_factor = self
            .output_state
            .info(&output)
            .map(|info| info.scale_factor);
        let Some(own) = self.screen_mut(&key).and_then(|s| s.surface.as_mut()) else {
            return;
        };
        if let Some(scale_factor) = scale_factor {
            own.scale_inputs.output_hint = Some(scale_factor);
        }
        let pending = own.pending_configure;
        self.update_scale(&key);
        if let Some(configured) = pending {
            self.apply_configure(&key, configured);
        }
    }

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if let Some(key) = self.binding.remove(&output) {
            tracing::info!(
                "output {key} is gone; {}",
                if key.name().is_some() {
                    "waiting for it to come back"
                } else {
                    "it has no name, so it is not waited for"
                }
            );
            self.release_surface(&key);
        }
    }
}

impl LayerShellHandler for AppState {
    /// The compositor closed a surface, normally because its output was removed
    /// or disabled. This is not a reason to exit: the surface is released and
    /// created again when the output is announced (or changes) again. The daemon
    /// exits only on a signal or when the connection to the compositor is lost.
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        let Some(key) = self.key_of_surface(layer.wl_surface()) else {
            return;
        };
        if self.binding.surface_closed(&key) {
            tracing::info!(
                "the compositor closed the wallpaper surface of {key}; waiting for the output"
            );
        }
        self.release_surface(&key);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // sctk has already acked this configure, so a buffer may be attached later
        // (from `update_output`) if the size has to wait for the output.
        if let Some(key) = self.key_of_surface(layer.wl_surface()) {
            self.apply_configure(&key, configure.new_size);
        }
    }
}

impl Dispatch<WpFractionalScaleV1, wl_surface::WlSurface> for AppState {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let wp_fractional_scale_v1::Event::PreferredScale { scale } = event else {
            return;
        };
        let Some(key) = state.key_of_surface(surface) else {
            return;
        };
        tracing::debug!("compositor prefers fractional scale {scale}/120 for {key}");
        if let Some(own) = state.screen_mut(&key).and_then(|s| s.surface.as_mut()) {
            own.scale_inputs.fractional = Some(scale);
        }
        state.update_scale(&key);
    }
}

// These interfaces have no events.
delegate_noop!(AppState: WpViewporter);
delegate_noop!(AppState: WpViewport);
delegate_noop!(AppState: WpFractionalScaleManagerV1);

impl ShmHandler for AppState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for AppState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(AppState);
delegate_output!(AppState);
delegate_shm!(AppState);
delegate_layer!(AppState);
delegate_registry!(AppState);
