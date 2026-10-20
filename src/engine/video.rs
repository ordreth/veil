//! Video wallpapers: a GStreamer `playbin` that renders through `waylandsink` into
//! a subsurface of the layer surface.
//!
//! # Limits of `waylandsink` that shape this module
//!
//! Both were checked against GStreamer 1.28.7 and upstream `main` on 2026-10-10.
//! `docs/known-issues.md` has the details, how to reproduce them, and the options
//! that were considered.
//!
//! 1. **One sink, one surface.** `waylandsink` cannot change its window handle and
//!    keeps its window until it is finalized. [`Player`] therefore lives as long as
//!    one surface: [`VideoEngine::detach`] drops it and the next `attach` builds a
//!    new one.
//! 2. **Output changes crash the process (upstream bug, not fixed here).**
//!    `waylandsink` binds every `wl_output` on the shared display and handles
//!    their events on its own thread. Its `output_done` handler takes the display
//!    pointer with `g_object_steal_data`, so the *second* `done` event of any
//!    output dereferences `NULL` and the process dies with SIGSEGV. A compositor
//!    sends `done` after every change of an output (scale, mode, transform,
//!    position), and sway also re-sends it for the remaining outputs when another
//!    one is added, removed, enabled or disabled. Nothing in veil runs in between,
//!    so it cannot be caught or avoided from here. What veil does: it keeps a sink
//!    alive only while a video is actually on screen (`detach` when the surface is
//!    lost), so nothing can crash while the wallpaper's output is away. Measured
//!    in a headless sway: with a single output, disabling and enabling it works
//!    (the pipeline is rebuilt); with two outputs, every change of either one
//!    killed a playing video. The decision on 2026-10-10 was to accept and
//!    document this, and to reconsider replacing `waylandsink` later.

use super::{Attached, EngineSignal, FrameCounts, RenderContext, StreamInfo, WallpaperEngine};
use crate::error::VeilError;
use crate::notify::{self, Throttle, Verdict};
use crate::scaling::{self, Rect, ScaleMode, ScaleOptions, SurfaceScale};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::*;
use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Sink chain. `videocrop` sits right before `waylandsink`, which accepts
/// `GstVideoCropMeta`: the crop is then only attached as metadata and applied by the
/// compositor through the video subsurface's viewport, so no frame is copied.
///
/// `videoconvert` is what a video costs when the decoder's frames are in system
/// memory in a format the compositor does not take (NV12 from NVDEC, which no
/// compositor takes in `wl_shm`): it converts every frame to RGB on the CPU, and
/// passes frames through untouched when the sink takes them as they are. Two
/// threads, because one cannot convert 4K at 60 fps (measured: 45 to 51 frames
/// a second with one, 60 with two, for about 1.5 points more of a core at 1080p).
const SINK_BIN: &str = "videoconvert n-threads=2 ! videocrop name=vcrop ! waylandsink name=wsink";

/// `playbin` flags: video only, and `native-video`, so that `playsink` puts no
/// `videoconvert`, `videoscale` and `videobalance` of its own in front of the sink
/// bin. The sink bin's `videoconvert` still converts what the sink cannot take,
/// and the compositor scales (`docs/measurements.md`).
const PLAYBIN_FLAGS: &str = "video+native-video";

/// Size of the decoded video frames, as negotiated in the caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
    /// Pixel aspect ratio as `(numerator, denominator)`; `(1, 1)` for square pixels.
    pub par: (u32, u32),
}

impl FrameSize {
    /// Reads the frame size from negotiated raw video caps.
    pub fn from_caps(caps: &gst::CapsRef) -> Option<Self> {
        let info = gst_video::VideoInfo::from_caps(caps).ok()?;
        let par = info.par();
        let par = match (u32::try_from(par.numer()), u32::try_from(par.denom())) {
            (Ok(n), Ok(d)) if n > 0 && d > 0 => (n, d),
            _ => (1, 1),
        };
        (info.width() > 0 && info.height() > 0).then_some(Self {
            width: info.width(),
            height: info.height(),
            par,
        })
    }

    /// Frames per second of negotiated raw video caps, as a fraction, if the
    /// stream has a fixed rate (`0/1` in the caps means a variable one).
    pub fn frame_rate(caps: &gst::CapsRef) -> Option<(u32, u32)> {
        let fps = gst_video::VideoInfo::from_caps(caps).ok()?.fps();
        match (u32::try_from(fps.numer()), u32::try_from(fps.denom())) {
            (Ok(n), Ok(d)) if n > 0 && d > 0 => Some((n, d)),
            _ => None,
        }
    }

    /// Whether this is a size a frame can have: no dimension and neither part
    /// of the pixel aspect ratio is zero. `from_caps` returns nothing else, but
    /// the fields are public, and the layout divides by them.
    fn is_valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.par.0 > 0 && self.par.1 > 0
    }

    /// Width the frame is meant to be displayed at (non-square pixels applied).
    fn display_width(&self) -> u32 {
        let (n, d) = self.par;
        let width = (u64::from(self.width) * u64::from(n) + u64::from(d) / 2) / u64::from(d);
        width.clamp(1, u64::from(u32::MAX)) as u32
    }
}

/// Pixels `videocrop` removes from each edge of a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Crop {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

/// How a video is presented: the frame is cropped by `crop`, and what remains is
/// scaled (without keeping its aspect ratio; `crop` and `render` already do) to the
/// `render` rectangle of the surface, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoLayout {
    pub crop: Crop,
    pub render: Rect,
}

/// Computes the video layout for `mode` on a surface of `surface` logical pixels
/// shown at `scale`, reusing the shared [`scaling::place`] geometry. Until the
/// frame size is known (`None`, or a size with a zero in it) the video covers
/// the whole surface. `Tile` is not meaningful for video and acts as `Center`.
/// Returns `None` for an empty surface.
///
/// The frame is placed on the surface's physical pixels, so `Center` maps one
/// frame pixel to one device pixel (as it does for images). The render rectangle
/// is then converted back to the logical pixels `waylandsink` positions its
/// subsurface in. The compositor scales the frame straight to the physical size
/// of that rectangle, so video needs no larger buffers on HiDPI outputs.
pub fn video_layout(
    mode: ScaleMode,
    frame: Option<FrameSize>,
    surface: (u32, u32),
    scale: SurfaceScale,
) -> Option<VideoLayout> {
    let full = Rect::new(0, 0, surface.0, surface.1);
    if surface.0 == 0 || surface.1 == 0 {
        return None;
    }
    let Some(frame) = frame.filter(FrameSize::is_valid) else {
        return Some(VideoLayout {
            crop: Crop::default(),
            render: full,
        });
    };

    let mode = match mode {
        ScaleMode::Tile => ScaleMode::Center,
        mode => mode,
    };
    let display = (frame.display_width(), frame.height);
    let placement = scaling::place(mode, display, scale.physical_size(surface))?;

    // `place` works in display pixels; the crop is in frame pixels, which differ
    // horizontally when the pixel aspect ratio is not 1:1.
    let to_frame_x = |x: u32| {
        ((u64::from(x) * u64::from(frame.width) + u64::from(display.0) / 2) / u64::from(display.0))
            as u32
    };
    let src = placement.src;
    let left = to_frame_x(src.x).min(frame.width - 1);
    let width = to_frame_x(src.width).clamp(1, frame.width - left);

    Some(VideoLayout {
        crop: Crop {
            left,
            right: frame.width - left - width,
            top: src.y,
            bottom: frame.height - src.y - src.height,
        },
        render: scale.rect_to_logical(placement.dst, surface),
    })
}

/// Inputs of the video layout. Written by the event-loop thread (`attach`, `resize`) and
/// by the streaming thread once the caps are negotiated (the frame size).
#[derive(Debug, Default)]
struct LayoutState {
    mode: ScaleMode,
    /// Surface size in logical pixels.
    surface: Option<(u32, u32)>,
    scale: SurfaceScale,
    frame: Option<FrameSize>,
    /// Frames per second, from the same caps as `frame`. Only reported
    /// (`veil status`); the layout does not use it.
    frame_rate: Option<(u32, u32)>,
    applied: Option<VideoLayout>,
    /// Who is told when the streaming thread moves the render rectangle: the
    /// signal of the surface the pipeline is attached to (`start_on`).
    signal: Option<EngineSignal>,
}

/// Applies the layout to `videocrop` and `waylandsink` whenever an input changes.
/// It only holds weak references to the elements, so the caps handler that owns
/// it does not keep the pipeline alive.
struct Geometry {
    state: Mutex<LayoutState>,
    videocrop: gst::glib::WeakRef<gst::Element>,
    waylandsink: gst::glib::WeakRef<gst::Element>,
}

impl Geometry {
    /// Updates the inputs with `change` and pushes the resulting layout to the
    /// elements if it differs from the last one applied. `force` re-applies anyway
    /// (after `waylandsink` gets its window handle). Returns whether the render
    /// rectangle was set anew.
    fn update(&self, force: bool, change: impl FnOnce(&mut LayoutState)) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut state);
        if force {
            state.applied = None;
        }

        let Some(surface) = state.surface else {
            return false;
        };
        let Some(layout) = video_layout(state.mode, state.frame, surface, state.scale) else {
            return false;
        };
        if state.applied == Some(layout) {
            return false;
        }

        if state.applied.map(|applied| applied.crop) != Some(layout.crop) {
            if let Some(videocrop) = self.videocrop.upgrade() {
                let crop = layout.crop;
                for (edge, value) in [
                    ("left", crop.left),
                    ("right", crop.right),
                    ("top", crop.top),
                    ("bottom", crop.bottom),
                ] {
                    videocrop.set_property(edge, i32::try_from(value).unwrap_or(i32::MAX));
                }
            }
        }

        let moved = state.applied.map(|applied| applied.render) != Some(layout.render);
        if let Some(overlay) = self
            .waylandsink
            .upgrade()
            .and_then(|sink| sink.dynamic_cast::<gst_video::VideoOverlay>().ok())
        {
            let r = layout.render;
            let to_i32 = |v: u32| i32::try_from(v).unwrap_or(i32::MAX);
            if let Err(e) = overlay.set_render_rectangle(
                to_i32(r.x),
                to_i32(r.y),
                to_i32(r.width),
                to_i32(r.height),
            ) {
                tracing::warn!("failed to set the video render rectangle: {e}");
            }
        }

        tracing::debug!(
            "video layout: frame {:?}, surface {surface:?}, scale {}, mode {:?} -> {layout:?}",
            state.frame,
            state.scale,
            state.mode
        );
        state.applied = Some(layout);
        moved
    }

    /// Tells the surface the pipeline is attached to that the render rectangle
    /// was moved from the streaming thread. `waylandsink` places its subsurface
    /// with `wl_subsurface.set_position`, which takes effect only when that
    /// surface is committed, and nothing else commits it while a video plays.
    fn report_move(&self) {
        let signal = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .signal
            .clone();
        if let Some(signal) = signal {
            signal.moved();
        }
    }
}

/// Re-applies the layout when the caps reaching `videocrop` (the native frame size)
/// are negotiated, which happens on the streaming thread before the first frame
/// is rendered, and reports a render rectangle that moved (`Geometry::report_move`).
fn watch_frame_size(videocrop: &gst::Element, geometry: &Arc<Geometry>) -> Result<(), VeilError> {
    let crop_sink = videocrop
        .static_pad("sink")
        .ok_or_else(|| VeilError::Gstreamer("videocrop has no sink pad".into()))?;
    let geometry = Arc::clone(geometry);
    crop_sink.connect_notify(Some("caps"), move |pad, _| {
        let Some(caps) = pad.current_caps() else {
            return;
        };
        let frame = FrameSize::from_caps(&caps);
        if frame.is_some() {
            let frame_rate = FrameSize::frame_rate(&caps);
            let moved = geometry.update(false, |state| {
                state.frame = frame;
                state.frame_rate = frame_rate;
            });
            if moved {
                geometry.report_move();
            }
        }
    });
    Ok(())
}

/// Logs the video decoder `playbin` plugs, when it plugs it: which one GStreamer
/// picked (by rank) decides most of what a video costs, and whether it runs on
/// the GPU. The handler holds no reference to the pipeline.
fn log_video_decoder(pipeline: &gst::Element) {
    let Some(bin) = pipeline.downcast_ref::<gst::Bin>() else {
        return;
    };
    bin.connect_deep_element_added(|_, _, element| {
        let Some(factory) = element.factory() else {
            return;
        };
        if !factory
            .has_type(gst::ElementFactoryType::DECODER | gst::ElementFactoryType::MEDIA_VIDEO)
        {
            return;
        }
        let hardware = factory
            .metadata(gst::ELEMENT_METADATA_KLASS)
            .is_some_and(|klass| klass.contains("Hardware"));
        tracing::info!(
            "video decoder: {} ({})",
            factory.name(),
            if hardware { "hardware" } else { "software" }
        );
    });
}

/// One GStreamer pipeline, bound to at most one Wayland surface for its whole life.
///
/// `waylandsink` cannot be moved to another surface: `set_window_handle` on a sink
/// that already has a window only logs "changing window handle is not supported",
/// and the sink keeps its window and its display connection until it is finalized,
/// even in the `Null` state. So when the surface goes away (its output was
/// unplugged) the whole player is dropped, and a new one is built for the next
/// surface. This is not a loop: loops still seek and never rebuild the pipeline.
struct Player {
    pipeline: gst::Element,
    waylandsink: gst::Element,
    geometry: Arc<Geometry>,
    /// Whether `waylandsink` was given a window handle. Such a player cannot be
    /// attached again.
    attached: bool,
    /// Whether the surface under the video has been filled with the video's
    /// background colour. Until then it is another wallpaper's.
    background_drawn: bool,
    /// The bus thread, from `attach` on. A pipeline that was only built (to
    /// validate a file, or for a surface that never came) has none.
    watch: Option<BusWatch>,
    /// Whether the pipeline is to play or to stay paused, shared with the bus
    /// thread.
    wanted: Arc<Wanted>,
}

/// What the event loop wants of a pipeline once it has its first frame. The bus
/// thread reads it whenever it brings the pipeline back from a failure, and
/// applies it when it is told that it changed; it is the only one that changes
/// the state of a pipeline that has taken over, except `Drop for Player`.
#[derive(Debug, Default)]
struct Wanted {
    /// The engine has taken over its surface (`take_over`): before that a
    /// pipeline shows its first frame and does not play, also after a restart.
    taken_over: AtomicBool,
    /// The wallpaper cannot be seen: the pipeline stays in `Paused`, with the
    /// frame it has on screen.
    paused: AtomicBool,
}

impl Wanted {
    /// The state the pipeline is to be in, once it has its first frame.
    fn state(&self) -> gst::State {
        if self.taken_over.load(Ordering::Acquire) && !self.paused.load(Ordering::Acquire) {
            gst::State::Playing
        } else {
            gst::State::Paused
        }
    }
}

impl Player {
    /// Builds the pipeline for `uri` in the `Null` state. `surface` is the surface
    /// size (logical pixels) and scale to lay the video out for, if already known.
    fn build(
        uri: &str,
        mode: ScaleMode,
        surface: Option<((u32, u32), SurfaceScale)>,
    ) -> Result<Self, VeilError> {
        let sink_bin = gst::parse::bin_from_description(SINK_BIN, true)?;
        let element = |name: &str| {
            sink_bin
                .by_name(name)
                .ok_or_else(|| VeilError::Gstreamer(format!("element '{name}' not found in bin")))
        };
        let waylandsink = element("wsink")?;
        let videocrop = element("vcrop")?;

        // The crop and the render rectangle already keep (or deliberately drop) the
        // aspect ratio, so the sink must scale to exactly the render rectangle.
        // The property came with GStreamer 1.28, which keeps the aspect ratio by
        // default; an older sink scales to the rectangle by itself (checked with
        // 1.26.2: a 4:3 clip in `stretch` filled a 16:9 output).
        if waylandsink.find_property("force-aspect-ratio").is_some() {
            waylandsink.set_property("force-aspect-ratio", false);
        } else {
            tracing::debug!(
                "this waylandsink has no force-aspect-ratio property (GStreamer before 1.28); \
                 it scales to the render rectangle by itself"
            );
        }

        let geometry = Arc::new(Geometry {
            state: Mutex::new(LayoutState {
                mode,
                surface: surface.map(|(size, _)| size),
                scale: surface.map(|(_, scale)| scale).unwrap_or_default(),
                ..LayoutState::default()
            }),
            videocrop: videocrop.downgrade(),
            waylandsink: waylandsink.downgrade(),
        });

        watch_frame_size(&videocrop, &geometry)?;

        let pipeline = gst::ElementFactory::make("playbin")
            .property("uri", uri)
            .property("video-sink", &sink_bin)
            .build()?;

        pipeline.set_property_from_str("flags", PLAYBIN_FLAGS);
        log_video_decoder(&pipeline);

        Ok(Self {
            pipeline,
            waylandsink,
            geometry,
            attached: false,
            background_drawn: false,
            watch: None,
            wanted: Arc::default(),
        })
    }

    /// The engine takes over the surface: the video plays from now on, unless
    /// its wallpaper is paused.
    fn play(&self) -> Result<(), VeilError> {
        self.wanted.taken_over.store(true, Ordering::Release);
        self.pipeline.set_state(self.wanted.state())?;
        Ok(())
    }

    /// Asks for the pipeline to be paused or to play. The bus thread does it, as
    /// soon as the pipeline has its first frame: a pipeline that is starting or
    /// being restarted must not be changed under its feet.
    fn set_paused(&self, paused: bool) {
        if self.wanted.paused.swap(paused, Ordering::AcqRel) == paused {
            return;
        }
        if let Some(watch) = &self.watch {
            watch.wanted_changed();
        }
    }
}

impl Drop for Player {
    /// Leaves nothing of the pipeline behind: when this returns, its bus thread
    /// has ended, its streaming threads are gone and no handler of it can run.
    /// GStreamer may still hold the pipeline on one of its own pool threads for
    /// a moment, so the last reference is not always the one dropped here.
    fn drop(&mut self) {
        // The bus thread must not take the shutdown for a failure to loop.
        if let Some(watch) = &self.watch {
            watch.stopping();
        }
        // `Null` first: it also releases a seek the bus thread may be inside.
        if let Err(e) = self.pipeline.set_state(gst::State::Null) {
            tracing::warn!("failed to shut the video pipeline down: {e}");
        }
        // Ends the bus thread and waits for it.
        self.watch = None;
        // Once more: the bus thread restarts a pipeline that failed, and may
        // have been in the middle of that when it was told to stop.
        let _ = self.pipeline.set_state(gst::State::Null);
        if let Some(bus) = self.pipeline.bus() {
            // The sync handler carries the display pointer of the surface that
            // is being given up.
            bus.unset_sync_handler();
            // A flushing bus keeps no message. One that stays queued holds a
            // reference to the element that posted it, and the pipeline would
            // keep itself alive through its own bus. `Null` leaves the bus
            // flushing only if the pipeline was started, and waking the bus
            // thread turns flushing off again.
            bus.set_flushing(true);
        }
    }
}

/// Name of the bus thread, as `/proc/<pid>/task/*/comm` shows it.
const BUS_THREAD: &str = "veil-bus";

/// The message that wakes the bus thread to end it.
const STOP_MESSAGE: &str = "veil-bus-stop";

/// The message that tells the bus thread that `Wanted::paused` changed.
const WANTED_MESSAGE: &str = "veil-bus-wanted";

/// The thread that reads the bus of a playing pipeline: it starts the video
/// again when it has ended, and reports playback errors.
///
/// The thread lives exactly as long as this value. It sleeps in the bus until a
/// message arrives (no timeout, so an idle video costs no wake-ups), and `Drop`
/// wakes it with a message of its own and waits until it has ended. A pipeline
/// that is replaced or rebuilt therefore leaves no thread and no bus behind.
struct BusWatch {
    bus: gst::Bus,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl BusWatch {
    /// Starts the thread for `pipeline`. It holds the bus and only a weak
    /// reference to the pipeline.
    ///
    /// The thread inherits the signal mask of the caller, which must have
    /// SIGINT and SIGTERM blocked (the event loop thread has).
    fn start(
        pipeline: &gst::Element,
        sink: Option<&gst::Element>,
        wanted: Arc<Wanted>,
        signal: EngineSignal,
        policy: RestartPolicy,
    ) -> Result<Self, VeilError> {
        let bus = pipeline
            .bus()
            .ok_or_else(|| VeilError::Gstreamer("Pipeline has no bus".into()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name(BUS_THREAD.into())
            .spawn({
                let bus = bus.clone();
                let stop = Arc::clone(&stop);
                let pipeline = pipeline.downgrade();
                let sink = sink.map(|sink| sink.downgrade());
                move || {
                    watch_bus(
                        &bus,
                        &pipeline,
                        sink.as_ref(),
                        &stop,
                        &wanted,
                        signal,
                        policy,
                    )
                }
            })
            .map_err(|e| VeilError::Gstreamer(format!("cannot start the bus thread: {e}")))?;
        Ok(Self {
            bus,
            stop,
            thread: Some(thread),
        })
    }

    /// Tells the thread that the pipeline is being shut down, so it handles no
    /// further message. `Drop` is what ends it.
    fn stopping(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// Wakes the thread to apply what is wanted of the pipeline now.
    fn wanted_changed(&self) {
        let message = gst::message::Application::new(gst::Structure::new_empty(WANTED_MESSAGE));
        if let Err(e) = self.bus.post(message) {
            tracing::warn!("cannot tell the bus thread of a video to pause or resume it: {e}");
        }
    }
}

impl Drop for BusWatch {
    fn drop(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        self.stopping();
        // The thread sleeps in the bus; only a message wakes it. A pipeline that
        // was shut down has set its bus flushing, which drops whatever is posted.
        self.bus.set_flushing(false);
        let wake_up = gst::message::Application::new(gst::Structure::new_empty(STOP_MESSAGE));
        if let Err(e) = self.bus.post(wake_up) {
            // Waiting for a thread that was not woken would never end.
            tracing::error!("cannot wake the bus thread of a video; it is left behind: {e}");
            return;
        }
        if thread.join().is_err() {
            tracing::error!("the bus thread of a video panicked");
        }
    }
}

/// How a failure of a video that was playing is answered: the pipeline is
/// restarted, after a delay that doubles every time, and given up after a
/// number of restarts in a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    /// The delay before the first restart. Each further one waits twice as long.
    pub first_delay: Duration,
    /// How many restarts are tried in a row before the video is given up.
    pub max_restarts: u32,
    /// A video that has played this long since its last restart is taken to be
    /// in order again: its next failure starts a new row of restarts.
    pub healthy_after: Duration,
    /// How long a pipeline may take to show its first frame, when it is
    /// started and after each restart.
    pub start_timeout: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            first_delay: Duration::from_millis(500),
            max_restarts: 4,
            healthy_after: Duration::from_secs(60),
            start_timeout: Duration::from_secs(10),
        }
    }
}

/// The restarts of one pipeline, counted according to a [`RestartPolicy`]. It
/// keeps no clock: the bus thread says what time it is.
#[derive(Debug)]
struct Restarts {
    policy: RestartPolicy,
    /// Restarts since the video was last in order.
    row: u32,
    /// Since when the video has been playing without a failure.
    playing_since: Option<Instant>,
}

impl Restarts {
    fn new(policy: RestartPolicy) -> Self {
        Self {
            policy,
            row: 0,
            playing_since: None,
        }
    }

    /// The video plays: it has its first frame, at the start or after a restart.
    fn playing(&mut self, now: Instant) {
        self.playing_since = Some(now);
    }

    /// Playback has failed. Returns how long to wait before the pipeline is
    /// restarted, or `None` if it has been restarted often enough.
    fn failed(&mut self, now: Instant) -> Option<Duration> {
        let healthy = self
            .playing_since
            .take()
            .is_some_and(|since| now.saturating_duration_since(since) >= self.policy.healthy_after);
        if healthy {
            self.row = 0;
        }
        if self.row >= self.policy.max_restarts {
            return None;
        }
        let delay = self
            .policy
            .first_delay
            .saturating_mul(2u32.saturating_pow(self.row));
        self.row += 1;
        Some(delay)
    }
}

/// What the bus thread is waiting for.
#[derive(Debug, Clone, Copy)]
enum Phase {
    /// The first frame of a pipeline that was attached, until `deadline`.
    Starting { deadline: Instant },
    /// Nothing: the video has its first frame, and is in the state that is
    /// wanted of it (`Wanted`): playing, paused on purpose, or still waiting
    /// for its engine to take over. Messages come when they come.
    Playing,
    /// The moment to restart a pipeline that failed. It is in `Null` meanwhile.
    Waiting { until: Instant },
    /// The first frame of a pipeline that was restarted, until `deadline`.
    Restarting { deadline: Instant },
    /// Nothing any more: the start failed, or the video was given up. Whoever
    /// was told so drops the pipeline.
    Over,
}

impl Phase {
    /// When this phase ends by itself, if it does.
    fn deadline(&self) -> Option<Instant> {
        match *self {
            Phase::Starting { deadline } | Phase::Restarting { deadline } => Some(deadline),
            Phase::Waiting { until } => Some(until),
            Phase::Playing | Phase::Over => None,
        }
    }
}

/// What `sink` did with the frames since the video last started from its
/// beginning: a sink counts what it rendered and what it dropped, and starts
/// again from zero with every flushing seek (GStreamer 1.28). `waylandsink`
/// counts as dropped a frame it replaced before the compositor took the one
/// before. `None` if the sink keeps no such statistics.
fn sink_counts(sink: &gst::Element) -> Option<FrameCounts> {
    sink.find_property("stats")?;
    let stats = sink.property::<gst::Structure>("stats");
    Some(FrameCounts {
        rendered: stats.get::<u64>("rendered").ok()?,
        dropped: stats.get::<u64>("dropped").ok()?,
    })
}

/// How many frames have reached `sink` since the video last started from its
/// beginning, rendered or dropped.
fn frames_in_this_run(sink: &gst::Element) -> Option<u64> {
    let counts = sink_counts(sink)?;
    Some(counts.rendered.saturating_add(counts.dropped))
}

/// Body of the bus thread: handles the messages of `bus` until `stop` is set.
///
/// It tells `signal` how the start went, and nobody else: whoever attached the
/// engine decides what a failed start means, and tells the user. A video that
/// fails later is restarted here, on the same pipeline, as `policy` says. Only
/// when that is given up does the thread say so, with `EngineSignal::lost`.
///
/// The thread sleeps in the bus without a timeout while the video plays or is
/// paused. It has a deadline only while it waits for a first frame or for a
/// restart. Once the pipeline has its first frame, the thread puts it into the
/// state `wanted` asks for, after a restart and whenever it is told that this
/// changed; a paused pipeline is therefore never taken for one that waits for
/// its first frame, and a restart does not undo a pause.
fn watch_bus(
    bus: &gst::Bus,
    pipeline: &gst::glib::WeakRef<gst::Element>,
    sink: Option<&gst::glib::WeakRef<gst::Element>>,
    stop: &AtomicBool,
    wanted: &Wanted,
    signal: EngineSignal,
    policy: RestartPolicy,
) {
    use gst::MessageView;

    let mut phase = Phase::Starting {
        deadline: Instant::now() + policy.start_timeout,
    };
    let mut restarts = Restarts::new(policy);
    // A pipeline can repeat a warning for every frame; the log does not.
    let mut warnings = Throttle::new(notify::REPEAT_AFTER, notify::MAX_PER_WINDOW);

    loop {
        let timeout = phase.deadline().map(|deadline| {
            let left = deadline.saturating_duration_since(Instant::now());
            gst::ClockTime::from_nseconds(u64::try_from(left.as_nanos()).unwrap_or(u64::MAX))
        });
        let msg = bus.timed_pop(timeout);
        if stop.load(Ordering::Acquire) {
            break;
        }
        // Held for this round only; `Drop for Player` waits for this thread
        // before it lets go of the pipeline.
        let Some(pipeline) = pipeline.upgrade() else {
            break;
        };
        let now = Instant::now();

        // What this round found out about the video: nothing, or why it does
        // not play.
        let failure = match msg {
            None if phase.deadline().is_some_and(|deadline| now < deadline) => None,
            None => match phase {
                Phase::Starting { .. } => {
                    phase = Phase::Over;
                    signal.started(Err(VeilError::Gstreamer(format!(
                        "the video did not show a frame within {:?}",
                        policy.start_timeout
                    ))));
                    None
                }
                Phase::Waiting { .. } => {
                    tracing::debug!("restarting the video pipeline");
                    match pipeline.set_state(gst::State::Paused) {
                        Ok(_) => {
                            phase = Phase::Restarting {
                                deadline: now + policy.start_timeout,
                            };
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    }
                }
                Phase::Restarting { .. } => Some(format!(
                    "it did not show a frame within {:?}",
                    policy.start_timeout
                )),
                Phase::Playing | Phase::Over => None,
            },
            Some(msg) => match msg.view() {
                // The pipeline has reached `Paused`: its first frame is in the
                // sink. (Every seek ends with this message too.)
                MessageView::AsyncDone(_) => match phase {
                    Phase::Starting { .. } => {
                        tracing::debug!("the video has its first frame");
                        restarts.playing(now);
                        phase = Phase::Playing;
                        signal.started(Ok(()));
                        None
                    }
                    // Paused, it has its first frame again: it plays on if it
                    // is to play. (A paused video counts as playing towards
                    // `RestartPolicy::healthy_after`.)
                    Phase::Restarting { .. } => match pipeline.set_state(wanted.state()) {
                        Ok(_) => {
                            tracing::info!(
                                "the video {}",
                                match wanted.state() {
                                    gst::State::Playing => "plays again",
                                    _ => "is back, and stays paused",
                                }
                            );
                            restarts.playing(now);
                            phase = Phase::Playing;
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    },
                    Phase::Playing | Phase::Waiting { .. } | Phase::Over => None,
                },
                // The loop: the same pipeline plays again, it is never rebuilt.
                MessageView::Eos(_) if matches!(phase, Phase::Playing) => {
                    // A file that has become unreadable gives no error: the
                    // pipeline finds nothing to decode, and "ends" on time with
                    // the last good frame still on screen. A run that showed
                    // no frame at all is therefore a failure, not a loop.
                    let frames = sink
                        .and_then(|sink| sink.upgrade())
                        .as_ref()
                        .and_then(frames_in_this_run);
                    if frames == Some(0) {
                        Some("it ended without showing a single frame".to_string())
                    } else if frames == Some(1) {
                        // One frame is a picture: there is nothing to play
                        // again. Seeking would show the same frame, end at
                        // once and seek again, thousands of times a second
                        // (measured: more than two CPU cores for a PNG that
                        // GStreamer was given as a video). The sink keeps the
                        // frame on screen at the end of the stream.
                        tracing::info!(
                            "the video has a single frame; it stays on screen and is not looped"
                        );
                        None
                    } else {
                        match pipeline.seek_simple(
                            gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                            gst::ClockTime::ZERO,
                        ) {
                            Ok(()) => {
                                tracing::debug!(
                                    "the video ended after {} frames; playing it again from the start",
                                    frames.map_or("an unknown number of".to_string(), |n| n.to_string())
                                );
                                None
                            }
                            // Not while shutting down: a seek cut short by `Null`
                            // fails.
                            Err(_) if stop.load(Ordering::Acquire) => break,
                            Err(e) => {
                                Some(format!("it could not be started again at its end: {e}"))
                            }
                        }
                    }
                }
                MessageView::Error(err) => match phase {
                    Phase::Starting { .. } => {
                        tracing::debug!(
                            "the video cannot start: {} ({:?})",
                            err.error(),
                            err.debug()
                        );
                        phase = Phase::Over;
                        signal.started(Err(VeilError::Gstreamer(format!(
                            "the video cannot be played: {}",
                            err.error()
                        ))));
                        None
                    }
                    Phase::Playing | Phase::Restarting { .. } => {
                        tracing::debug!("GStreamer error: {} ({:?})", err.error(), err.debug());
                        Some(err.error().to_string())
                    }
                    // One failure gives several errors; the first one was
                    // handled.
                    Phase::Waiting { .. } | Phase::Over => {
                        tracing::debug!("GStreamer error: {}", err.error());
                        None
                    }
                },
                MessageView::Warning(warning) => {
                    let text = warning.error().to_string();
                    match warnings.admit(&text, now) {
                        Verdict::Show => {
                            tracing::warn!("GStreamer warning: {text} ({:?})", warning.debug())
                        }
                        Verdict::Repeated | Verdict::TooMany => {
                            tracing::debug!("GStreamer warning: {text}")
                        }
                    }
                    None
                }
                // Not expected: a local file is not buffered (the pipeline has
                // only the `video` flag). If it ever comes, it is at least seen.
                // The event loop wants the video paused, or playing again.
                // Only a pipeline that has its first frame is changed: one
                // that is starting or being restarted reads `wanted` when it
                // has one.
                MessageView::Application(app)
                    if app.structure().is_some_and(|s| s.name() == WANTED_MESSAGE) =>
                {
                    match phase {
                        Phase::Playing => {
                            let state = wanted.state();
                            tracing::debug!("video pipeline: {state:?} is wanted");
                            pipeline.set_state(state).err().map(|e| e.to_string())
                        }
                        Phase::Starting { .. }
                        | Phase::Waiting { .. }
                        | Phase::Restarting { .. }
                        | Phase::Over => None,
                    }
                }
                MessageView::Buffering(buffering) => {
                    tracing::debug!("the video is buffering: {} %", buffering.percent());
                    None
                }
                MessageView::StateChanged(change)
                    if msg.src() == Some(pipeline.upcast_ref::<gst::Object>()) =>
                {
                    tracing::debug!(
                        "video pipeline: {:?} -> {:?}",
                        change.old(),
                        change.current()
                    );
                    None
                }
                _ => None,
            },
        };

        let Some(failure) = failure else {
            continue;
        };
        if stop.load(Ordering::Acquire) {
            break;
        }
        // The pipeline is of no use as it is. `Null` frees what it holds, and
        // empties the bus of the other errors of the same failure. The sink
        // shows nothing in `Null`, so the background colour is on screen.
        if let Err(e) = pipeline.set_state(gst::State::Null) {
            tracing::warn!("failed to stop the video pipeline after an error: {e}");
        }
        match restarts.failed(now) {
            Some(delay) => {
                tracing::warn!(
                    "video playback failed: {failure}; restarting it in {delay:?} \
                     (restart {} of {})",
                    restarts.row,
                    policy.max_restarts
                );
                phase = Phase::Waiting { until: now + delay };
            }
            None => {
                phase = Phase::Over;
                signal.lost(VeilError::Gstreamer(format!(
                    "the video stopped and could not be restarted: {failure}"
                )));
            }
        }
    }
}

/// The `file://` URI of the absolute path `path`, percent-encoded. A path is not
/// a URI as it is: `playbin` takes a `#` or a `?` in it for the start of a
/// fragment or a query and a `%` for an escape, and then cannot open the file
/// (`veil set` of `wall#2.webm` used to fail with "Resource not found").
fn file_uri(path: &std::path::Path) -> Result<String, VeilError> {
    Ok(gst::glib::filename_to_uri(path, None)?.into())
}

pub struct VideoEngine {
    /// `file://` URI of the video, kept to rebuild the pipeline after `detach`.
    uri: String,
    options: ScaleOptions,
    /// The pipeline. `None` after `detach`, until the next `attach` builds one.
    player: Option<Player>,
    /// Whether the wallpaper is paused (`pause`/`resume`). Every pipeline of
    /// this engine is told, also one that is built later.
    paused: bool,
}

impl VideoEngine {
    /// Builds the pipeline up front, so a missing file or GStreamer plugin is
    /// reported before the current wallpaper is replaced.
    pub fn new(input_path: &str, options: ScaleOptions) -> Result<Self, VeilError> {
        gst::init()?;

        let abs_path =
            std::fs::canonicalize(input_path).map_err(|e| VeilError::PathResolution {
                path: input_path.to_string(),
                source: e,
            })?;
        let uri = file_uri(&abs_path)?;

        if options.mode == ScaleMode::Tile {
            tracing::warn!("--mode tile is not supported for video; using center");
        }
        let player = Player::build(&uri, options.mode, None)?;

        Ok(Self {
            uri,
            options,
            player: Some(player),
            paused: false,
        })
    }

    /// The pipeline to attach to a surface of `size` logical pixels at `scale`.
    /// A pipeline serves one surface only (see `Player`): the one built by `new`
    /// is used if it has no window yet; after `detach`, or if the current one
    /// already has a window, a new one is built for this surface.
    fn fresh_player(
        &mut self,
        size: (u32, u32),
        scale: SurfaceScale,
    ) -> Result<&mut Player, VeilError> {
        let player = match self.player.take() {
            Some(player) if !player.attached => {
                player.geometry.update(false, |state| {
                    state.surface = Some(size);
                    state.scale = scale;
                });
                player
            }
            _ => Player::build(&self.uri, self.options.mode, Some((size, scale)))?,
        };
        player.set_paused(self.paused);
        Ok(self.player.insert(player))
    }
}

/// Type of the GStreamer context through which `waylandsink` is given the
/// display of the application, in the field `handle`. GStreamer 1.28 reads
/// `display` first and `handle` for backward compatibility (`gstwlcontext.c`);
/// a field it cannot read makes the sink open a window of its own.
const DISPLAY_CONTEXT: &str = "GstWaylandDisplayHandleContextType";

/// The `wl_display*` of the surface a video is shown on, on its way to
/// `waylandsink`. veil never dereferences it: it is only put into the context
/// below, as the address it is.
#[derive(Clone, Copy)]
struct WlDisplayPtr(*mut c_void);

// SAFETY: this type has no operation that reads through the pointer, so moving
// it to another thread moves an address and nothing else. Who reads through it
// is `waylandsink`, on GStreamer's threads, with libwayland's `wl_display`
// functions, which may be called from any thread. The display it points to is
// never freed while the process runs (see `WindowHandle::display`), so there is
// no thread on which the address could outlive it.
unsafe impl Send for WlDisplayPtr {}
// SAFETY: as above. A shared reference gives the same address and still no way
// to read through it. The bus sync handler needs this: GStreamer calls it from
// whichever thread posts a message.
unsafe impl Sync for WlDisplayPtr {}

impl From<WlDisplayPtr> for gst::glib::Value {
    fn from(ptr: WlDisplayPtr) -> Self {
        Self::from(ptr.0)
    }
}

/// The context that tells `waylandsink` which display its window is on.
fn display_context(display: WlDisplayPtr) -> gst::Context {
    let mut context = gst::Context::new(DISPLAY_CONTEXT, true);
    // `make_mut` and not `get_mut`: a context that was just created has one
    // owner and is written in place, and nothing here can fail.
    context.make_mut().structure_mut().set("handle", display);
    context
}

impl VideoEngine {
    /// Puts the video's subsurface on the context's surface and lets the
    /// pipeline produce its first frame there. The video does not play yet, and
    /// the surface keeps the wallpaper it shows: the pipeline's bus thread
    /// reports through `ctx.signal` when the frame is there, and `take_over`
    /// does the rest. The surface itself gets `--bg-color`, because a
    /// subsurface is only visible while its parent has a buffer, and the colour
    /// shows through letterbox bars.
    fn start_on(&mut self, ctx: &mut RenderContext<'_>) -> Result<Attached, VeilError> {
        let window = ctx.target.window();
        if window.display.is_null() || window.surface.is_null() {
            return Err(VeilError::Gstreamer(
                "the render target has no Wayland surface for waylandsink".into(),
            ));
        }
        let display = WlDisplayPtr(window.display);
        let bg_color = self.options.bg_color;

        let player = self.fresh_player(ctx.size, ctx.scale)?;

        player.pipeline.set_context(&display_context(display));

        let bus = player
            .pipeline
            .bus()
            .ok_or_else(|| VeilError::Gstreamer("Pipeline has no bus".into()))?;

        // The sink asks for the display when it reaches `Ready`, if the context
        // set on the pipeline did not get to it. The handler runs on the thread
        // that posts the message.
        bus.set_sync_handler(move |_, msg| {
            use gst::MessageView;
            if let MessageView::NeedContext(ctx) = msg.view() {
                if ctx.context_type() == DISPLAY_CONTEXT {
                    if let Some(src) = msg.src().and_then(|s| s.downcast_ref::<gst::Element>()) {
                        src.set_context(&display_context(display));
                    }
                }
            }
            gst::BusSyncReply::Pass
        });

        player.pipeline.set_state(gst::State::Ready)?;

        let overlay = player
            .waylandsink
            .dynamic_cast_ref::<gst_video::VideoOverlay>()
            .ok_or_else(|| {
                VeilError::Gstreamer("waylandsink does not implement VideoOverlay".into())
            })?;

        // SAFETY: the handle must be a live `wl_surface*` on the display the sink
        // was given, and the sink must not have a window yet. `window.surface` is
        // the surface this engine is being attached to and was checked for null
        // above; it belongs to `window.display`, the display of the context set
        // on the pipeline, and it is alive until `detach` returns
        // (`WindowHandle`), so for the whole of this call. `waylandsink` reads
        // through the pointer only inside the call, where it makes its own
        // subsurface under the surface and commits it (GStreamer 1.28.7,
        // `gst_wl_window_new_in_surface`); it does not keep the pointer. This
        // player has had no window before: `fresh_player` returns only one whose
        // `attached` is false, and that is set right below.
        unsafe {
            overlay.set_window_handle(window.surface as usize);
        }
        player.attached = true;

        // Geometry must be (re-)applied after the window handle is set in `Ready`.
        // What the streaming thread changes later is reported to this surface.
        let signal = ctx.signal.clone();
        player
            .geometry
            .update(true, |state| state.signal = Some(signal));

        // Started here and not with the pipeline, so only a pipeline that plays
        // has a thread, and the thread is started from the event loop thread. It
        // reports the first frame, or the error that prevents it.
        player.watch = Some(BusWatch::start(
            &player.pipeline,
            Some(&player.waylandsink),
            Arc::clone(&player.wanted),
            ctx.signal.clone(),
            RestartPolicy::default(),
        )?);

        // The surface must have a buffer for the video to be visible on it. A
        // surface that shows nothing gets the background colour now. One that
        // shows another wallpaper keeps it until this video has its first
        // frame (`take_over`), and for good if it never has one.
        if !ctx.keep_previous {
            ctx.target.fill(bg_color)?;
            player.background_drawn = true;
        }

        // `Paused`, not `Playing`: the pipeline opens the file, decodes one frame
        // and shows it in its subsurface, above the previous wallpaper, and then
        // waits. The answer comes on the bus, also when the state change is
        // refused at once (a file that cannot be opened): the element that
        // refuses says why in an error message, and the bus thread passes that
        // on as the failed start. If no message comes, its deadline does.
        if let Err(e) = player.pipeline.set_state(gst::State::Paused) {
            tracing::debug!("the video pipeline does not start ({e}); the bus will say why");
        }
        Ok(Attached::Starting)
    }
}

impl WallpaperEngine for VideoEngine {
    /// Starts the video on the context's surface (see `start_on`). An attach
    /// that fails keeps nothing: a pipeline that already has its window on the
    /// surface is dropped, with its sink and its bus thread.
    fn attach(&mut self, ctx: &mut RenderContext<'_>) -> Result<Attached, VeilError> {
        let attached = self.start_on(ctx);
        if attached.is_err() && self.player.as_ref().is_some_and(|player| player.attached) {
            self.player = None;
        }
        attached
    }

    /// The first frame is on screen: play (unless the wallpaper is paused), and
    /// put the background colour under the video, in place of the previous
    /// wallpaper. Playing comes first, so that a pipeline that cannot play
    /// leaves that wallpaper's buffer alone.
    fn take_over(&mut self, ctx: &mut RenderContext<'_>) -> Result<(), VeilError> {
        let Some(player) = &mut self.player else {
            return Err(VeilError::Gstreamer(
                "the video has no pipeline to start".into(),
            ));
        };
        player.play()?;
        if !player.background_drawn {
            ctx.target.fill(self.options.bg_color)?;
            player.background_drawn = true;
        }
        Ok(())
    }

    /// Lays the video out for the new size or scale. The buffer under the video
    /// has to follow the surface as well, once it is this video's.
    fn resize(&mut self, ctx: &mut RenderContext<'_>) -> Result<(), VeilError> {
        let Some(player) = &self.player else {
            return Ok(());
        };
        player.geometry.update(false, |state| {
            state.surface = Some(ctx.size);
            state.scale = ctx.scale;
        });
        if player.background_drawn {
            ctx.target.fill(self.options.bg_color)?;
        }
        Ok(())
    }

    /// Drops the pipeline. This must happen before the surface is destroyed:
    /// `waylandsink` holds subsurfaces of it and a raw pointer to it. Dropping the
    /// sink also stops its Wayland event thread, so while there is no surface the
    /// `output_done` crash described in the module docs cannot occur.
    fn detach(&mut self) {
        self.player = None;
    }

    /// The pipeline stops in `Paused`, with its frame on screen, as soon as it
    /// has one; its bus thread does that (`Wanted`). A failure is handled like
    /// any other failure of a playing video: restarted, and reported as lost
    /// when that does not help.
    fn pause(&mut self) {
        self.paused = true;
        if let Some(player) = &self.player {
            player.set_paused(true);
        }
    }

    fn resume(&mut self) {
        self.paused = false;
        if let Some(player) = &self.player {
            player.set_paused(false);
        }
    }

    /// The sink's statistics, once the video has taken over its surface.
    fn frame_counts(&self) -> Option<FrameCounts> {
        let player = self.player.as_ref()?;
        if !player.wanted.taken_over.load(Ordering::Acquire) {
            return None;
        }
        sink_counts(&player.waylandsink)
    }

    /// The frame size and rate, once the caps are negotiated.
    fn stream(&self) -> Option<StreamInfo> {
        let player = self.player.as_ref()?;
        let state = player
            .geometry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Some(StreamInfo {
            size: state.frame.map(|frame| (frame.width, frame.height)),
            frame_rate: state.frame_rate,
        })
    }

    fn is_dynamic(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testing::FakeTarget;
    use crate::engine::EngineReport;
    use crate::scaling::Color;
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const SQUARE: (u32, u32) = (1, 1);

    fn frame(width: u32, height: u32, par: (u32, u32)) -> Option<FrameSize> {
        Some(FrameSize { width, height, par })
    }

    fn layout(mode: ScaleMode, frame: Option<FrameSize>, surface: (u32, u32)) -> VideoLayout {
        layout_at(SurfaceScale::ONE, mode, frame, surface)
    }

    fn layout_at(
        scale: SurfaceScale,
        mode: ScaleMode,
        frame: Option<FrameSize>,
        surface: (u32, u32),
    ) -> VideoLayout {
        video_layout(mode, frame, surface, scale).expect("non-empty surface")
    }

    fn crop(left: u32, right: u32, top: u32, bottom: u32) -> Crop {
        Crop {
            left,
            right,
            top,
            bottom,
        }
    }

    #[test]
    fn unknown_frame_size_covers_the_surface() {
        for mode in [ScaleMode::Fill, ScaleMode::Fit, ScaleMode::Center] {
            assert_eq!(
                layout(mode, None, (2560, 1080)),
                VideoLayout {
                    crop: Crop::default(),
                    render: Rect::new(0, 0, 2560, 1080),
                }
            );
        }
    }

    #[test]
    fn fit_letterboxes_1080p_on_ultrawide() {
        // 16:9 video on a 21:9 output.
        assert_eq!(
            layout(ScaleMode::Fit, frame(1920, 1080, SQUARE), (2560, 1080)),
            VideoLayout {
                crop: Crop::default(),
                render: Rect::new(320, 0, 1920, 1080),
            }
        );
    }

    #[test]
    fn fill_crops_1080p_on_ultrawide() {
        assert_eq!(
            layout(ScaleMode::Fill, frame(1920, 1080, SQUARE), (2560, 1080)),
            VideoLayout {
                crop: crop(0, 0, 135, 135),
                render: Rect::new(0, 0, 2560, 1080),
            }
        );
    }

    #[test]
    fn stretch_uses_the_whole_frame_and_surface() {
        assert_eq!(
            layout(ScaleMode::Stretch, frame(1920, 1080, SQUARE), (2560, 1080)),
            VideoLayout {
                crop: Crop::default(),
                render: Rect::new(0, 0, 2560, 1080),
            }
        );
    }

    #[test]
    fn center_crops_a_larger_frame_at_native_size() {
        assert_eq!(
            layout(ScaleMode::Center, frame(1920, 1080, SQUARE), (1280, 720)),
            VideoLayout {
                crop: crop(320, 320, 180, 180),
                render: Rect::new(0, 0, 1280, 720),
            }
        );
        // A smaller frame is not cropped and sits in the middle.
        assert_eq!(
            layout(ScaleMode::Center, frame(640, 360, SQUARE), (1920, 1080)),
            VideoLayout {
                crop: Crop::default(),
                render: Rect::new(640, 360, 640, 360),
            }
        );
    }

    #[test]
    fn tile_falls_back_to_center() {
        let f = frame(1920, 1080, SQUARE);
        assert_eq!(
            layout(ScaleMode::Tile, f, (1280, 720)),
            layout(ScaleMode::Center, f, (1280, 720))
        );
    }

    #[test]
    fn non_square_pixels_are_displayed_at_their_aspect_ratio() {
        // 1440x1080 with 4:3 pixels is displayed as 1920x1080.
        let anamorphic = frame(1440, 1080, (4, 3));
        assert_eq!(
            layout(ScaleMode::Fit, anamorphic, (1920, 1080)).render,
            Rect::new(0, 0, 1920, 1080)
        );
        assert_eq!(
            layout(ScaleMode::Fill, anamorphic, (2560, 1080)).crop,
            crop(0, 0, 135, 135)
        );
        // The horizontal crop is converted back to frame pixels: 960 display
        // pixels out of 1920 are 720 frame pixels out of 1440.
        assert_eq!(
            layout(ScaleMode::Center, anamorphic, (960, 1080)),
            VideoLayout {
                crop: crop(360, 360, 0, 0),
                render: Rect::new(0, 0, 960, 1080),
            }
        );
    }

    #[test]
    fn crops_always_leave_a_visible_region_inside_the_frame() {
        let frames = [
            (1, 1, SQUARE),
            (1920, 1080, SQUARE),
            (1440, 1080, (4, 3)),
            (720, 576, (16, 11)),
            (3, 4000, SQUARE),
        ];
        let surfaces = [(1, 1), (1920, 1080), (2560, 1080), (1080, 1920), (7, 3)];
        let scales = [96, 120, 150, 180, 210, 240].map(SurfaceScale::from_120ths);
        for mode in [
            ScaleMode::Fill,
            ScaleMode::Fit,
            ScaleMode::Stretch,
            ScaleMode::Center,
        ] {
            for (w, h, par) in frames {
                for surface in surfaces {
                    for scale in scales {
                        let l = layout_at(scale, mode, frame(w, h, par), surface);
                        let ctx =
                            format!("{mode:?} {w}x{h} {par:?} on {surface:?} at {scale}: {l:?}");
                        assert!(l.crop.left + l.crop.right < w, "{ctx}");
                        assert!(l.crop.top + l.crop.bottom < h, "{ctx}");
                        assert!(l.render.width > 0 && l.render.height > 0, "{ctx}");
                        assert!(l.render.x + l.render.width <= surface.0, "{ctx}");
                        assert!(l.render.y + l.render.height <= surface.1, "{ctx}");
                    }
                }
            }
        }
    }

    #[test]
    fn scale_does_not_change_aspect_preserving_layouts() {
        // On a 2x output the surface is 1280x540 logical and 2560x1080 physical.
        // The crop is the same as at 1x on a 2560x1080 surface, and the render
        // rectangle is the same shape in logical pixels.
        let two = SurfaceScale::from_integer(2);
        let f = frame(1920, 1080, SQUARE);
        assert_eq!(
            layout_at(two, ScaleMode::Fill, f, (1280, 540)),
            VideoLayout {
                crop: crop(0, 0, 135, 135),
                render: Rect::new(0, 0, 1280, 540),
            }
        );
        assert_eq!(
            layout_at(two, ScaleMode::Fit, f, (1280, 540)),
            VideoLayout {
                crop: Crop::default(),
                render: Rect::new(160, 0, 960, 540),
            }
        );
        assert_eq!(
            layout_at(two, ScaleMode::Stretch, f, (1280, 540)),
            VideoLayout {
                crop: Crop::default(),
                render: Rect::new(0, 0, 1280, 540),
            }
        );
    }

    #[test]
    fn center_maps_frame_pixels_to_physical_pixels() {
        // 2x: a 640x360 frame covers 320x180 logical pixels, centred.
        let two = SurfaceScale::from_integer(2);
        assert_eq!(
            layout_at(
                two,
                ScaleMode::Center,
                frame(640, 360, SQUARE),
                (1920, 1080)
            ),
            VideoLayout {
                crop: Crop::default(),
                render: Rect::new(800, 450, 320, 180),
            }
        );
        // 2x on a 960x540 surface (1920x1080 physical): a 1080p frame fits exactly
        // and is not cropped, although it is larger than the logical size.
        assert_eq!(
            layout_at(
                two,
                ScaleMode::Center,
                frame(1920, 1080, SQUARE),
                (960, 540)
            ),
            VideoLayout {
                crop: Crop::default(),
                render: Rect::new(0, 0, 960, 540),
            }
        );
        // 1.5x on 1280x720 (1920x1080 physical): a 4K frame is cropped to its
        // middle 1920x1080 pixels.
        let one_and_a_half = SurfaceScale::from_120ths(180);
        assert_eq!(
            layout_at(
                one_and_a_half,
                ScaleMode::Center,
                frame(3840, 2160, SQUARE),
                (1280, 720)
            ),
            VideoLayout {
                crop: crop(960, 960, 540, 540),
                render: Rect::new(0, 0, 1280, 720),
            }
        );
        // 1.25x on 1536x864 (1920x1080 physical): a 960x540 frame is centred at
        // (480, 270) physical, which is (384, 216) logical, 768x432 in size.
        assert_eq!(
            layout_at(
                SurfaceScale::from_120ths(150),
                ScaleMode::Center,
                frame(960, 540, SQUARE),
                (1536, 864)
            ),
            VideoLayout {
                crop: Crop::default(),
                render: Rect::new(384, 216, 768, 432),
            }
        );
    }

    #[test]
    fn unknown_frame_size_covers_the_surface_at_any_scale() {
        assert_eq!(
            layout_at(
                SurfaceScale::from_120ths(210),
                ScaleMode::Center,
                None,
                (2194, 1234)
            ),
            VideoLayout {
                crop: Crop::default(),
                render: Rect::new(0, 0, 2194, 1234),
            }
        );
    }

    #[test]
    fn empty_surface_has_no_layout() {
        assert_eq!(
            video_layout(
                ScaleMode::Fill,
                frame(1920, 1080, SQUARE),
                (0, 1080),
                SurfaceScale::ONE
            ),
            None
        );
    }

    #[test]
    fn a_frame_size_with_a_zero_in_it_is_laid_out_like_an_unknown_one() {
        let unknown = video_layout(ScaleMode::Fill, None, (1920, 1080), SurfaceScale::ONE);
        for (width, height, par) in [
            (0, 1080, SQUARE),
            (1920, 0, SQUARE),
            (1920, 1080, (0, 1)),
            (1920, 1080, (1, 0)),
            (0, 0, (0, 0)),
        ] {
            for mode in [
                ScaleMode::Fill,
                ScaleMode::Fit,
                ScaleMode::Stretch,
                ScaleMode::Center,
                ScaleMode::Tile,
            ] {
                assert_eq!(
                    video_layout(
                        mode,
                        frame(width, height, par),
                        (1920, 1080),
                        SurfaceScale::ONE
                    ),
                    unknown,
                    "{width}x{height}, pixel aspect ratio {par:?}, {mode:?}"
                );
            }
        }
    }

    #[test]
    fn frame_size_is_read_from_raw_video_caps() {
        gst::init().expect("gstreamer init");
        let caps = gst_video::VideoInfo::builder(gst_video::VideoFormat::I420, 1440, 1080)
            .par(gst::Fraction::new(4, 3))
            .build()
            .expect("video info")
            .to_caps()
            .expect("caps");
        assert_eq!(FrameSize::from_caps(&caps), frame(1440, 1080, (4, 3)));

        let audio = gst::Caps::builder("audio/x-raw").build();
        assert_eq!(FrameSize::from_caps(&audio), None);
    }

    #[test]
    fn frame_rate_is_read_from_raw_video_caps_if_it_is_fixed() {
        gst::init().expect("gstreamer init");
        let caps = |fps: gst::Fraction| {
            gst_video::VideoInfo::builder(gst_video::VideoFormat::I420, 64, 48)
                .fps(fps)
                .build()
                .expect("video info")
                .to_caps()
                .expect("caps")
        };
        assert_eq!(
            FrameSize::frame_rate(&caps(gst::Fraction::new(30000, 1001))),
            Some((30000, 1001))
        );
        // A variable rate.
        assert_eq!(FrameSize::frame_rate(&caps(gst::Fraction::new(0, 1))), None);
        let audio = gst::Caps::builder("audio/x-raw").build();
        assert_eq!(FrameSize::frame_rate(&audio), None);
    }

    /// Builds `description` with `gst::parse::launch`, or returns `None` (and the
    /// test is skipped) when a needed plugin is not installed.
    fn launch_or_skip(description: &str) -> Option<gst::Pipeline> {
        gst::init().expect("gstreamer init");
        for factory in ["videotestsrc", "capsfilter", "videocrop", "fakesink"] {
            if gst::ElementFactory::find(factory).is_none() {
                eprintln!("skipping: GStreamer element '{factory}' is not installed");
                return None;
            }
        }
        let pipeline = gst::parse::launch(description).expect("valid pipeline");
        Some(pipeline.downcast().expect("a pipeline"))
    }

    #[test]
    fn negotiated_caps_drive_the_crop() {
        // No compositor: a test source feeds videocrop, the same caps watch as
        // `VideoEngine` computes the layout, and fakesink receives the cropped frames.
        let Some(pipeline) = launch_or_skip(
            "videotestsrc num-buffers=3 ! video/x-raw,width=1920,height=1080 \
             ! videocrop name=vcrop ! fakesink name=sink",
        ) else {
            return;
        };
        let videocrop = pipeline.by_name("vcrop").expect("videocrop");
        let sink = pipeline.by_name("sink").expect("fakesink");

        let geometry = Arc::new(Geometry {
            state: Mutex::new(LayoutState {
                mode: ScaleMode::Fill,
                surface: Some((2560, 1080)),
                ..LayoutState::default()
            }),
            videocrop: videocrop.downgrade(),
            // fakesink is no VideoOverlay; the render rectangle is simply skipped.
            waylandsink: sink.downgrade(),
        });
        watch_frame_size(&videocrop, &geometry).expect("caps watch");

        pipeline.set_state(gst::State::Playing).expect("play");
        let bus = pipeline.bus().expect("bus");
        let msg = bus
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(10),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .expect("pipeline finishes");
        assert!(
            matches!(msg.view(), gst::MessageView::Eos(_)),
            "pipeline error: {msg:?}"
        );

        let caps = sink
            .static_pad("sink")
            .and_then(|pad| pad.current_caps())
            .expect("negotiated caps");
        let info = gst_video::VideoInfo::from_caps(&caps).expect("video caps");
        pipeline.set_state(gst::State::Null).expect("stop");

        // `fill` of 16:9 on 21:9 keeps the full width and 810 of the 1080 rows.
        assert_eq!((info.width(), info.height()), (1920, 810));
        let state = geometry.state.lock().expect("state");
        assert_eq!(state.frame, frame(1920, 1080, SQUARE));
        assert_eq!(state.applied.map(|l| l.crop), Some(crop(0, 0, 135, 135)));
    }

    /// The moves the caps watch reports for a 1920x1080 stream on a 2560x1080
    /// surface in `mode`, after the layout was applied without a frame size, as
    /// `VideoEngine::attach` does.
    fn moves_reported(mode: ScaleMode) -> Option<Vec<String>> {
        let pipeline = launch_or_skip(
            "videotestsrc num-buffers=3 ! video/x-raw,width=1920,height=1080 \
             ! videocrop name=vcrop ! fakesink name=sink",
        )?;
        let videocrop = pipeline.by_name("vcrop").expect("videocrop");
        let sink = pipeline.by_name("sink").expect("fakesink");
        let geometry = Arc::new(Geometry {
            state: Mutex::new(LayoutState {
                mode,
                surface: Some((2560, 1080)),
                ..LayoutState::default()
            }),
            videocrop: videocrop.downgrade(),
            waylandsink: sink.downgrade(),
        });
        let (signal, reports) = signal();
        // Applied on the event loop thread: not reported.
        assert!(geometry.update(true, |state| state.signal = Some(signal)));
        watch_frame_size(&videocrop, &geometry).expect("caps watch");

        pipeline.set_state(gst::State::Playing).expect("play");
        let bus = pipeline.bus().expect("bus");
        let msg = bus
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(10),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .expect("pipeline finishes");
        pipeline.set_state(gst::State::Null).expect("stop");
        assert!(
            matches!(msg.view(), gst::MessageView::Eos(_)),
            "pipeline error: {msg:?}"
        );
        Some(reports.try_iter().map(|r| format!("{r:?}")).collect())
    }

    #[test]
    fn a_frame_size_that_moves_the_picture_is_reported() {
        // `fit` puts the 16:9 frame in the middle of the 21:9 surface: the
        // subsurface moves, and the surface has to be committed for that.
        let Some(moves) = moves_reported(ScaleMode::Fit) else {
            return;
        };
        assert_eq!(moves, ["Moved"]);
        // `fill` covers the surface whatever the frame size: nothing moves.
        let moves = moves_reported(ScaleMode::Fill).expect("the same elements");
        assert!(moves.is_empty(), "{moves:?}");
    }

    /// Tests that start bus threads hold this: they count the threads of that
    /// name in the whole test process, and `cargo test` runs tests in parallel.
    static BUS_THREADS: Mutex<()> = Mutex::new(());

    /// How many threads of this process are bus threads right now.
    fn bus_threads() -> usize {
        std::fs::read_dir("/proc/self/task")
            .expect("list the threads of this process")
            .filter_map(Result::ok)
            .filter(|task| {
                std::fs::read_to_string(task.path().join("comm"))
                    .is_ok_and(|name| name.trim_end() == BUS_THREAD)
            })
            .count()
    }

    /// Waits until no bus thread is left, and fails the test if one stays. The
    /// kernel still lists a thread for a moment after it has been joined, so
    /// this cannot be asserted at once.
    fn wait_for_no_bus_thread() {
        wait_until("the bus threads to be gone", || bus_threads() == 0);
    }

    /// Waits until `reached` holds, and fails the test if it takes too long.
    fn wait_until(what: &str, reached: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !reached() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Drops `value` on another thread, and fails the test if the drop does not
    /// return: a bus thread that is not woken would hang the test for ever.
    fn drop_in_time<T: Send + 'static>(value: T) {
        let (done, dropped) = mpsc::channel();
        std::thread::spawn(move || {
            drop(value);
            let _ = done.send(());
        });
        dropped
            .recv_timeout(Duration::from_secs(20))
            .expect("the drop returns");
    }

    /// Frames in the clip of `temp_clip`.
    const CLIP_FRAMES: usize = 4;

    /// Writes a clip of four small raw frames to a unique temp path; the caller
    /// removes it. It plays without a decoder. Returns `None` (and the test is
    /// skipped) when a plugin for writing or playing it is not installed.
    fn temp_clip(name: &str) -> Option<PathBuf> {
        temp_clip_of(name, CLIP_FRAMES)
    }

    /// Like `temp_clip`, with `frames` frames.
    fn temp_clip_of(name: &str, frames: usize) -> Option<PathBuf> {
        temp_clip_as(name, frames, &RAW)
    }

    /// How a test clip is stored.
    struct ClipFormat {
        /// What it is, for the messages of a test.
        name: &'static str,
        /// The elements between the test source and the file.
        encoder: &'static str,
        extension: &'static str,
        /// The elements writing it and playing it need, besides those of
        /// every clip.
        elements: &'static [&'static str],
        /// The caps of its encoded frames, which an installed decoder must
        /// take; `None` for raw frames.
        decoded_from: Option<&'static str>,
    }

    /// Raw frames in Matroska: no encoder, no decoder.
    const RAW: ClipFormat = ClipFormat {
        name: "raw frames in Matroska",
        encoder: "matroskamux",
        extension: "mkv",
        elements: &["matroskamux", "matroskademux"],
        decoded_from: None,
    };

    /// The kinds of file a user is most likely to have.
    const ENCODED: [ClipFormat; 2] = [
        ClipFormat {
            name: "H.264 in MP4",
            encoder: "openh264enc ! h264parse ! mp4mux",
            extension: "mp4",
            elements: &["openh264enc", "h264parse", "mp4mux", "qtdemux"],
            decoded_from: Some("video/x-h264"),
        },
        ClipFormat {
            name: "VP8 in WebM",
            encoder: "vp8enc ! webmmux",
            extension: "webm",
            elements: &["vp8enc", "webmmux", "matroskademux"],
            decoded_from: Some("video/x-vp8"),
        },
    ];

    /// Like `temp_clip`, with `frames` frames stored as `format`. Returns
    /// `None` (and the test is skipped) also when no installed decoder takes
    /// its frames.
    fn temp_clip_as(name: &str, frames: usize, format: &ClipFormat) -> Option<PathBuf> {
        gst::init().expect("gstreamer init");
        let common = [
            "videotestsrc",
            "filesink",
            "playbin",
            "videoconvert",
            "videoscale",
            "fakesink",
        ];
        for factory in common.iter().chain(format.elements) {
            if gst::ElementFactory::find(factory).is_none() {
                eprintln!(
                    "skipping {}: GStreamer element '{factory}' is not installed",
                    format.name
                );
                return None;
            }
        }
        if let Some(caps) = format.decoded_from {
            let caps = gst::Caps::new_empty_simple(caps);
            let decodable = gst::ElementFactory::factories_with_type(
                gst::ElementFactoryType::DECODER,
                gst::Rank::MARGINAL,
            )
            .iter()
            .any(|factory| factory.can_sink_any_caps(&caps));
            if !decodable {
                eprintln!("skipping {}: no decoder is installed", format.name);
                return None;
            }
        }

        let path = std::env::temp_dir().join(format!(
            "veil-test-{}-{name}.{}",
            std::process::id(),
            format.extension
        ));
        let writer = gst::parse::launch(&format!(
            "videotestsrc num-buffers={frames} \
             ! video/x-raw,format=I420,width=32,height=32,framerate=30/1 \
             ! {} ! filesink name=out",
            format.encoder
        ))
        .expect("valid pipeline")
        .downcast::<gst::Pipeline>()
        .expect("a pipeline");
        writer
            .by_name("out")
            .expect("filesink")
            .set_property("location", path.to_str().expect("UTF-8 temp path"));
        writer.set_state(gst::State::Playing).expect("write");
        let msg = writer
            .bus()
            .expect("bus")
            .timed_pop_filtered(
                gst::ClockTime::from_seconds(10),
                &[gst::MessageType::Eos, gst::MessageType::Error],
            )
            .expect("the clip is written");
        writer.set_state(gst::State::Null).expect("stop");
        assert!(
            matches!(msg.view(), gst::MessageView::Eos(_)),
            "writing the clip failed: {msg:?}"
        );
        Some(path)
    }

    /// What a pipeline reported through its signal, in the order of the reports.
    type Reports = mpsc::Receiver<EngineReport>;

    /// A signal whose reports can be read from the returned end.
    fn signal() -> (EngineSignal, Reports) {
        let (reports, reported) = mpsc::channel();
        let reports = Mutex::new(reports);
        let signal = EngineSignal::new(
            1,
            Arc::new(move |_, report| {
                let reports = reports.lock().unwrap_or_else(PoisonError::into_inner);
                let _ = reports.send(report);
            }),
        );
        (signal, reported)
    }

    /// Restarts that follow each other quickly, and a start timeout no test
    /// machine runs into.
    fn quick_restarts() -> RestartPolicy {
        RestartPolicy {
            first_delay: Duration::from_millis(10),
            max_restarts: 3,
            healthy_after: Duration::from_secs(3600),
            start_timeout: Duration::from_secs(20),
        }
    }

    /// A `Player` for the file at `path`, as `VideoEngine::attach` leaves it:
    /// its bus thread runs and the pipeline is on its way to `Paused`. There is
    /// no compositor: a `fakesink` takes the place of the sink bin. Also returns
    /// the count of frames that reached the sink, and the pipeline's reports.
    fn starting(path: &Path) -> (Player, Arc<AtomicUsize>, Reports) {
        starting_with(path, quick_restarts())
    }

    fn starting_with(path: &Path, policy: RestartPolicy) -> (Player, Arc<AtomicUsize>, Reports) {
        let sink = gst::ElementFactory::make("fakesink")
            .property("sync", true)
            .build()
            .expect("fakesink");
        let frames = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&frames);
        sink.static_pad("sink")
            .expect("sink pad")
            .add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                counter.fetch_add(1, Ordering::Relaxed);
                gst::PadProbeReturn::Ok
            })
            .expect("frame counter");

        let pipeline = gst::ElementFactory::make("playbin")
            .property("uri", file_uri(path).expect("an absolute path"))
            .property("video-sink", &sink)
            .build()
            .expect("playbin");
        pipeline.set_property_from_str("flags", PLAYBIN_FLAGS);

        let geometry = Arc::new(Geometry {
            state: Mutex::new(LayoutState::default()),
            videocrop: gst::glib::WeakRef::new(),
            waylandsink: sink.downgrade(),
        });
        // In the order of `VideoEngine::attach`: the bus thread, then `Paused`.
        let (signal, reports) = signal();
        let wanted = Arc::new(Wanted::default());
        let watch = BusWatch::start(&pipeline, Some(&sink), Arc::clone(&wanted), signal, policy)
            .expect("bus thread");
        // Like `attach`, which leaves a refusal to the bus thread to report.
        let _ = pipeline.set_state(gst::State::Paused);
        let player = Player {
            pipeline,
            waylandsink: sink,
            geometry,
            attached: true,
            background_drawn: false,
            watch: Some(watch),
            wanted,
        };
        (player, frames, reports)
    }

    /// The next report of a pipeline, which must come in time.
    fn next_report(reports: &Reports) -> EngineReport {
        reports
            .recv_timeout(Duration::from_secs(20))
            .expect("the pipeline reports")
    }

    /// The next report of a pipeline, which must be about its start.
    fn start_report(reports: &Reports) -> Result<(), VeilError> {
        match next_report(reports) {
            EngineReport::Started(started) => started,
            other => panic!("expected a start report, got {other:?}"),
        }
    }

    /// A `Player` that plays `clip`: started as in `starting`, and set to
    /// `Playing` after its first frame, as `VideoEngine::take_over` does.
    fn playing(clip: &Path) -> (Player, Arc<AtomicUsize>, Reports) {
        playing_with(clip, quick_restarts())
    }

    fn playing_with(clip: &Path, policy: RestartPolicy) -> (Player, Arc<AtomicUsize>, Reports) {
        let (player, frames, reports) = starting_with(clip, policy);
        start_report(&reports).expect("the clip starts");
        player.play().expect("play");
        (player, frames, reports)
    }

    /// The state the pipeline of `player` is in right now.
    fn state_of(player: &Player) -> gst::State {
        player.pipeline.state(gst::ClockTime::ZERO).1
    }

    /// Makes the pipeline of `player` believe that one of its elements failed.
    fn inject_error(player: &Player) {
        gst::element_error!(
            player.waylandsink,
            gst::CoreError::Failed,
            ("injected by a test")
        );
    }

    /// Overwrites the file at `path` in place with bytes no demuxer recognises.
    fn spoil(path: &Path) {
        let noise: Vec<u8> = (0..65_536u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        std::fs::write(path, noise).expect("overwrite the file");
    }

    #[test]
    fn a_video_that_fails_while_it_plays_is_restarted_on_the_same_pipeline() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("restart") else {
            return;
        };
        // Long enough before the restart for the test to see the pipeline stopped.
        let policy = RestartPolicy {
            first_delay: Duration::from_millis(400),
            ..quick_restarts()
        };
        let (player, frames, reports) = playing_with(&clip, policy);
        wait_until("the video to play", || frames.load(Ordering::Relaxed) > 1);

        inject_error(&player);
        wait_until("the pipeline to be stopped", || {
            state_of(&player) == gst::State::Null
        });
        let stopped_at = frames.load(Ordering::Relaxed);

        // It comes back by itself, and plays on past the end of the clip.
        wait_until("the pipeline to play again", || {
            state_of(&player) == gst::State::Playing
        });
        wait_until("the video to loop again", || {
            frames.load(Ordering::Relaxed) > stopped_at + 2 * CLIP_FRAMES
        });
        // Nobody was told: the video is back.
        assert!(reports.try_recv().is_err(), "a report for a restart");
        assert_eq!(bus_threads(), 1);

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    /// Waits until the pipeline of `player` has been in `state` for a moment
    /// and no frame has reached the sink meanwhile, checks that it stays so,
    /// and returns the frame count then. The state alone is not enough: the
    /// flushing seek of a loop leaves a playing pipeline `Paused` until it has
    /// prerolled, which can pass for a pause the bus thread has not applied
    /// yet, and a frame that was on its way may still arrive. (A `nix build`
    /// on a loaded machine once counted 3 frames "while Paused" after a 100 ms
    /// grace; not reproduced.) The clips play at 30 fps, so 200 ms
    /// without a frame means it stands still; a pipeline that goes on showing
    /// frames never does, and the wait times out.
    fn standing_still(player: &Player, frames: &AtomicUsize, state: gst::State) -> usize {
        let last = Cell::new(frames.load(Ordering::Relaxed));
        let since = Cell::new(Instant::now());
        wait_until("the pipeline to stand still", || {
            let now = frames.load(Ordering::Relaxed);
            if now != last.get() || state_of(player) != state {
                last.set(now);
                since.set(Instant::now());
            }
            since.get().elapsed() >= Duration::from_millis(200)
        });
        let before = frames.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(state_of(player), state);
        assert_eq!(
            frames.load(Ordering::Relaxed),
            before,
            "frames while {state:?}"
        );
        before
    }

    #[test]
    fn playbin_puts_no_converter_of_its_own_before_the_sink() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("native") else {
            return;
        };
        let (player, _frames, reports) = starting(&clip);
        start_report(&reports).expect("the clip starts");

        // Without `native-video`, `playsink` adds a `videoconvert`, a
        // `videoscale` and a `videobalance` in front of the sink.
        let bin = player
            .pipeline
            .downcast_ref::<gst::Bin>()
            .expect("playbin is a bin");
        let plugged: Vec<String> = bin
            .iterate_recurse()
            .into_iter()
            .filter_map(Result::ok)
            .filter_map(|element| element.factory().map(|factory| factory.name().to_string()))
            .collect();
        for converter in ["videoconvert", "videoscale", "videobalance"] {
            assert!(
                !plugged.iter().any(|name| name == converter),
                "playbin plugged {converter}: {plugged:?}"
            );
        }

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_playing_sink_counts_the_frames_it_renders() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("counts") else {
            return;
        };
        let (player, frames, _reports) = playing(&clip);
        wait_until("the video to play", || frames.load(Ordering::Relaxed) > 2);
        let counts = sink_counts(&player.waylandsink).expect("a sink with statistics");
        // A sink nothing holds back renders what it gets.
        assert!(counts.rendered > 0, "{counts:?}");
        assert_eq!(counts.dropped, 0, "{counts:?}");

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    #[test]
    fn the_uri_of_a_path_is_escaped() {
        let uri = file_uri(Path::new("/walls/sea #2? 100%.webm")).expect("an absolute path");
        assert_eq!(uri, "file:///walls/sea%20%232%3F%20100%25.webm");
        assert!(file_uri(Path::new("walls/sea.webm")).is_err());
    }

    #[test]
    fn a_video_whose_name_is_not_a_uri_plays() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        // Each of these characters alone kept `playbin` from opening the file
        // while the path was put into the URI as it was.
        let Some(clip) = temp_clip("a#b?c%41 d") else {
            return;
        };
        let (player, frames, _reports) = playing(&clip);
        wait_until("the video to play", || frames.load(Ordering::Relaxed) > 0);

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_paused_video_stands_still_and_plays_on_when_resumed() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("paused") else {
            return;
        };
        // A paused pipeline has no first frame to wait for: a start timeout
        // shorter than the pause must not fire.
        let policy = RestartPolicy {
            start_timeout: Duration::from_millis(200),
            ..quick_restarts()
        };
        let (player, frames, reports) = playing_with(&clip, policy);
        wait_until("the video to play", || frames.load(Ordering::Relaxed) > 1);

        player.set_paused(true);
        let paused_at = standing_still(&player, &frames, gst::State::Paused);
        assert!(reports.try_recv().is_err(), "a report for a pause");

        player.set_paused(false);
        wait_until("the pipeline to play again", || {
            state_of(&player) == gst::State::Playing
        });
        wait_until("the video to loop again", || {
            frames.load(Ordering::Relaxed) > paused_at + 2 * CLIP_FRAMES
        });
        assert!(reports.try_recv().is_err(), "a report for a resume");

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_restart_does_not_undo_a_pause() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("paused-restart") else {
            return;
        };
        let policy = RestartPolicy {
            first_delay: Duration::from_millis(300),
            ..quick_restarts()
        };
        let (player, frames, reports) = playing_with(&clip, policy);
        wait_until("the video to play", || frames.load(Ordering::Relaxed) > 1);
        player.set_paused(true);
        standing_still(&player, &frames, gst::State::Paused);

        // It fails while it is paused, and comes back paused, with a frame.
        let before = frames.load(Ordering::Relaxed);
        inject_error(&player);
        wait_until("the pipeline to be stopped", || {
            state_of(&player) == gst::State::Null
        });
        let restarted = standing_still(&player, &frames, gst::State::Paused);
        assert!(restarted > before, "no frame after the restart");
        assert!(reports.try_recv().is_err(), "a report for a restart");

        player.set_paused(false);
        wait_until("the video to play again", || {
            frames.load(Ordering::Relaxed) > restarted + CLIP_FRAMES
        });

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_video_does_not_play_before_it_has_taken_over() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("not-taken-over") else {
            return;
        };
        let policy = RestartPolicy {
            first_delay: Duration::from_millis(300),
            ..quick_restarts()
        };
        let (player, frames, reports) = starting_with(&clip, policy);
        start_report(&reports).expect("the clip starts");

        // Neither a resume nor a restart lets it play before `take_over`.
        player.set_paused(true);
        player.set_paused(false);
        standing_still(&player, &frames, gst::State::Paused);
        inject_error(&player);
        wait_until("the pipeline to be stopped", || {
            state_of(&player) == gst::State::Null
        });
        let restarted = standing_still(&player, &frames, gst::State::Paused);

        player.play().expect("play");
        wait_until("the video to play", || {
            frames.load(Ordering::Relaxed) > restarted + CLIP_FRAMES
        });

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_video_paused_before_it_has_started_stays_paused_when_it_takes_over() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("paused-start") else {
            return;
        };
        let (player, frames, reports) = starting(&clip);
        player.set_paused(true);
        start_report(&reports).expect("the clip starts");
        player.play().expect("take over");
        let shown = standing_still(&player, &frames, gst::State::Paused);
        assert_eq!(shown, 1, "the first frame, and no other");

        player.set_paused(false);
        wait_until("the video to play", || {
            frames.load(Ordering::Relaxed) > CLIP_FRAMES
        });

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_video_that_cannot_be_restarted_is_given_up_and_reported_once() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("given-up") else {
            return;
        };
        let (player, frames, reports) = playing(&clip);
        wait_until("the video to play", || frames.load(Ordering::Relaxed) > 1);

        // The file is ruined under the playing pipeline, which then fails. Every
        // restart opens the file again and finds nothing to play.
        spoil(&clip);
        inject_error(&player);

        let EngineReport::Lost(error) = next_report(&reports) else {
            panic!("expected the video to be given up");
        };
        let message = error.to_string();
        assert!(
            message.contains("the video stopped and could not be restarted"),
            "{message}"
        );
        // It is left stopped, and says nothing more.
        assert_eq!(state_of(&player), gst::State::Null);
        std::thread::sleep(Duration::from_millis(200));
        assert!(reports.try_recv().is_err(), "a second report");
        assert_eq!(state_of(&player), gst::State::Null);

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_file_that_cannot_be_opened_reports_a_failed_start() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        // Only for the plugin check: this test plays no file at all.
        let Some(clip) = temp_clip("gone-check") else {
            return;
        };
        std::fs::remove_file(&clip).expect("remove the clip");

        // The pipeline refuses the state change at once. The reason is not in
        // that refusal but in the error on the bus, which is what is reported.
        let (player, frames, reports) = starting(&clip);
        let error = start_report(&reports).expect_err("the file is gone");
        let message = error.to_string();
        assert!(message.contains("the video cannot be played"), "{message}");
        assert_eq!(frames.load(Ordering::Relaxed), 0);
        std::thread::sleep(Duration::from_millis(200));
        assert!(reports.try_recv().is_err(), "a second report");

        drop_in_time(player);
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_video_that_ends_without_a_single_frame_is_not_looped_but_restarted() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("no-frames") else {
            return;
        };
        let (player, frames, reports) = playing(&clip);
        wait_until("the video to play", || frames.load(Ordering::Relaxed) > 1);

        // The file is ruined under the playing pipeline. That gives no error:
        // the pipeline finds nothing to decode and "ends" on time, again and
        // again, with the last good frame still shown. The bus thread sees a
        // run without a frame, restarts the pipeline, and gives it up when the
        // restarts find nothing to play either.
        spoil(&clip);
        let EngineReport::Lost(error) = next_report(&reports) else {
            panic!("expected the video to be given up");
        };
        let message = error.to_string();
        assert!(message.contains("could not be restarted"), "{message}");
        assert_eq!(state_of(&player), gst::State::Null);

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_player_that_waits_for_its_restart_can_be_dropped_at_once() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("dropped-waiting") else {
            return;
        };
        // The restart would come in an hour.
        let policy = RestartPolicy {
            first_delay: Duration::from_secs(3600),
            ..quick_restarts()
        };
        let (player, frames, _reports) = playing_with(&clip, policy);
        wait_until("the video to play", || frames.load(Ordering::Relaxed) > 1);
        inject_error(&player);
        wait_until("the pipeline to be stopped", || {
            state_of(&player) == gst::State::Null
        });

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_pipeline_that_never_shows_a_frame_fails_its_start_in_time() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        // The valve lets nothing through, so the sink waits for a frame for ever.
        let Some(pipeline) = launch_or_skip("videotestsrc ! valve drop=true ! fakesink") else {
            return;
        };
        let pipeline: gst::Element = pipeline.upcast();
        let policy = RestartPolicy {
            start_timeout: Duration::from_millis(300),
            ..quick_restarts()
        };
        let (signal, reports) = signal();
        let started = Instant::now();
        let watch =
            BusWatch::start(&pipeline, None, Arc::default(), signal, policy).expect("bus thread");
        // Like `attach`, which leaves a refusal to the bus thread to report.
        let _ = pipeline.set_state(gst::State::Paused);

        let error = start_report(&reports).expect_err("no frame ever comes");
        assert!(started.elapsed() >= Duration::from_millis(300));
        let message = error.to_string();
        assert!(
            message.contains("did not show a frame within 300ms"),
            "{message}"
        );
        // Once, and the thread goes on waiting for whoever drops the pipeline.
        std::thread::sleep(Duration::from_millis(400));
        assert!(reports.try_recv().is_err(), "a second report");

        pipeline.set_state(gst::State::Null).expect("stop");
        drop_in_time(watch);
        wait_for_no_bus_thread();
    }

    #[test]
    fn restarts_wait_twice_as_long_each_time_and_end_after_the_last_one() {
        let policy = RestartPolicy {
            first_delay: Duration::from_millis(500),
            max_restarts: 4,
            healthy_after: Duration::from_secs(60),
            start_timeout: Duration::from_secs(10),
        };
        assert_eq!(policy, RestartPolicy::default());
        let mut restarts = Restarts::new(policy);
        let start = Instant::now();
        restarts.playing(start);

        let ms = Duration::from_millis;
        // It fails after ten seconds, and every restart fails as well.
        let delays: Vec<_> = (0..5)
            .map(|n| restarts.failed(start + Duration::from_secs(10 + n)))
            .collect();
        assert_eq!(
            delays,
            [
                Some(ms(500)),
                Some(ms(1000)),
                Some(ms(2000)),
                Some(ms(4000)),
                None
            ]
        );
        // Given up stays given up.
        assert_eq!(restarts.failed(start + Duration::from_secs(1000)), None);
    }

    #[test]
    fn a_restart_that_only_helps_for_a_moment_counts_towards_the_limit() {
        let mut restarts = Restarts::new(RestartPolicy::default());
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let ms = Duration::from_millis;

        // Each restart brings the video back, and it fails again 59 s later.
        restarts.playing(at(0));
        assert_eq!(restarts.failed(at(59)), Some(ms(500)));
        restarts.playing(at(60));
        assert_eq!(restarts.failed(at(119)), Some(ms(1000)));
        restarts.playing(at(120));
        assert_eq!(restarts.failed(at(179)), Some(ms(2000)));
        restarts.playing(at(180));
        assert_eq!(restarts.failed(at(239)), Some(ms(4000)));
        restarts.playing(at(240));
        assert_eq!(restarts.failed(at(299)), None);
    }

    #[test]
    fn a_video_that_played_long_enough_starts_with_a_clean_slate() {
        let mut restarts = Restarts::new(RestartPolicy::default());
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let ms = Duration::from_millis;

        restarts.playing(at(0));
        assert_eq!(restarts.failed(at(10)), Some(ms(500)));
        assert_eq!(restarts.failed(at(11)), Some(ms(1000)));
        assert_eq!(restarts.failed(at(12)), Some(ms(2000)));
        // The third restart works, and the video plays for a minute.
        restarts.playing(at(14));
        assert_eq!(restarts.failed(at(74)), Some(ms(500)));
        // A restart that fails is not "playing", however long it took.
        assert_eq!(restarts.failed(at(500)), Some(ms(1000)));
    }

    #[test]
    fn huge_delays_do_not_overflow() {
        let mut restarts = Restarts::new(RestartPolicy {
            first_delay: Duration::from_secs(u64::MAX / 4),
            max_restarts: 100,
            ..RestartPolicy::default()
        });
        let now = Instant::now();
        for _ in 0..100 {
            assert!(restarts.failed(now).is_some());
        }
        assert_eq!(restarts.failed(now), None);
    }

    #[test]
    fn a_video_shows_one_frame_and_reports_it_before_it_plays() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("start") else {
            return;
        };

        let (player, frames, reports) = starting(&clip);
        start_report(&reports).expect("the clip starts");
        // The first frame is in the sink, and the pipeline waits there.
        assert_eq!(frames.load(Ordering::Relaxed), 1);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(frames.load(Ordering::Relaxed), 1, "it plays already");

        player.play().expect("play");
        wait_until("the video to play", || frames.load(Ordering::Relaxed) > 1);
        // The start is reported once, although every loop of the video ends
        // with the same message as the start.
        wait_until("the video to start again", || {
            frames.load(Ordering::Relaxed) > CLIP_FRAMES
        });
        assert!(reports.try_recv().is_err(), "a second start report");

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_file_that_is_no_video_reports_a_failed_start_once() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        // Only for the plugin check: this test plays another file.
        let Some(clip) = temp_clip("no-video-check") else {
            return;
        };
        std::fs::remove_file(&clip).expect("remove the clip");

        // Named like a video, but with nothing a demuxer could recognise.
        let path =
            std::env::temp_dir().join(format!("veil-test-{}-no-video.mkv", std::process::id()));
        let noise: Vec<u8> = (0..65_536u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        std::fs::write(&path, noise).expect("write test file");

        let (player, frames, reports) = starting(&path);
        let error = start_report(&reports).expect_err("the file cannot be played");
        assert!(matches!(error, VeilError::Gstreamer(_)), "{error:?}");
        assert_eq!(frames.load(Ordering::Relaxed), 0);
        // GStreamer posts more than one error for such a file. The start is
        // reported once, and the rest is not passed on.
        std::thread::sleep(Duration::from_millis(200));
        assert!(reports.try_recv().is_err(), "a second start report");

        drop_in_time(player);
        std::fs::remove_file(&path).expect("remove test file");
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_video_that_ends_is_played_again_by_the_same_pipeline() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("loop") else {
            return;
        };

        let (player, frames, _reports) = playing(&clip);
        // The frame after the last one of the second run belongs to a third run.
        wait_until("the video to start a third time", || {
            frames.load(Ordering::Relaxed) > 2 * CLIP_FRAMES
        });
        assert_eq!(bus_threads(), 1);

        drop_in_time(player);
        std::fs::remove_file(&clip).expect("remove the clip");
        wait_for_no_bus_thread();
    }

    /// The video decoder `playbin` plugged for `player`, if any.
    fn plugged_decoder(player: &Player) -> Option<String> {
        player
            .pipeline
            .downcast_ref::<gst::Bin>()
            .expect("playbin is a bin")
            .iterate_recurse()
            .into_iter()
            .filter_map(Result::ok)
            .filter_map(|element| element.factory())
            .find(|factory| {
                factory.has_type(
                    gst::ElementFactoryType::DECODER | gst::ElementFactoryType::MEDIA_VIDEO,
                )
            })
            .map(|factory| factory.name().to_string())
    }

    #[test]
    fn an_encoded_clip_starts_loops_pauses_and_leaves_nothing_behind() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        const FRAMES: usize = 8;
        for format in &ENCODED {
            let Some(clip) = temp_clip_as("encoded", FRAMES, format) else {
                continue;
            };
            let name = format.name;

            let (player, frames, reports) = starting(&clip);
            start_report(&reports).expect("the clip starts");
            let decoder = plugged_decoder(&player);
            eprintln!("{name}: decoded by {decoder:?}");
            assert!(decoder.is_some(), "{name}: no decoder");
            assert_eq!(state_of(&player), gst::State::Paused, "{name}");
            assert_eq!(frames.load(Ordering::Relaxed), 1, "{name}");

            player.play().expect("play");
            wait_until("the video to start a third time", || {
                frames.load(Ordering::Relaxed) > 2 * FRAMES
            });
            // Not asserted at once: right after the seek of a loop the
            // pipeline is `Paused` for a moment, until it has prerolled again.
            wait_until("the pipeline to play", || {
                state_of(&player) == gst::State::Playing
            });

            player.set_paused(true);
            let paused_at = standing_still(&player, &frames, gst::State::Paused);
            player.set_paused(false);
            wait_until("the video to loop again", || {
                frames.load(Ordering::Relaxed) > paused_at + FRAMES
            });
            // Neither a restart nor a failure on the way.
            assert!(reports.try_recv().is_err(), "{name}: a report");
            assert_eq!(bus_threads(), 1, "{name}");

            drop_in_time(player);
            std::fs::remove_file(&clip).expect("remove the clip");
            wait_for_no_bus_thread();
        }
    }

    #[test]
    fn a_hundred_players_leave_no_thread_bus_or_pipeline_behind() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip("hundred") else {
            return;
        };

        for round in 0..100 {
            // Every second one is dropped while it starts up, the others while
            // they play.
            let player = if round % 2 == 0 {
                playing(&clip).0
            } else {
                starting(&clip).0
            };
            let pipeline = player.pipeline.downgrade();
            let bus = player.pipeline.bus().expect("bus").downgrade();

            drop_in_time(player);
            // Not at once: GStreamer may hold the pipeline for a moment longer
            // on one of its own threads. What is leaked stays for ever.
            wait_for_no_bus_thread();
            wait_until("the pipeline to be freed", || pipeline.upgrade().is_none());
            wait_until("the bus to be freed", || bus.upgrade().is_none());
        }
        std::fs::remove_file(&clip).expect("remove the clip");
    }

    #[test]
    fn a_video_of_one_frame_is_shown_and_not_looped() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(clip) = temp_clip_of("one-frame", 1) else {
            return;
        };

        let (player, frames, reports) = playing(&clip);
        // Long enough for a loop to show: a run of this clip lasts a thirtieth
        // of a second, so a pipeline that seeks at each end shows the frame
        // again many times in this time.
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(
            frames.load(Ordering::Relaxed),
            1,
            "the frame was shown again"
        );
        // It is not a failure either: the pipeline is where it was, and nobody
        // was told anything.
        assert_eq!(state_of(&player), gst::State::Playing);
        assert!(reports.try_recv().is_err());
        assert_eq!(bus_threads(), 1);

        drop_in_time(player);
        wait_for_no_bus_thread();
        std::fs::remove_file(&clip).expect("remove the clip");
    }

    #[test]
    fn a_bus_thread_ends_whether_or_not_its_pipeline_ever_ran() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        gst::init().expect("gstreamer init");
        let pipeline: gst::Element = gst::Pipeline::new().upcast();

        // Never started: the bus takes the wake-up message as it is.
        let watch = BusWatch::start(
            &pipeline,
            None,
            Arc::default(),
            EngineSignal::unheard(),
            quick_restarts(),
        )
        .expect("bus thread");
        wait_until("the bus thread", || bus_threads() == 1);
        // The thread shares this flag. Once the thread has ended, the test
        // holds the only other reference to it.
        let shared = Arc::clone(&watch.stop);
        drop_in_time(watch);
        assert_eq!(Arc::strong_count(&shared), 1, "the thread was not joined");
        wait_for_no_bus_thread();

        // Started and shut down again: the bus is flushing by then, and would
        // drop the wake-up message.
        let watch = BusWatch::start(
            &pipeline,
            None,
            Arc::default(),
            EngineSignal::unheard(),
            quick_restarts(),
        )
        .expect("bus thread");
        pipeline.set_state(gst::State::Playing).expect("play");
        pipeline.set_state(gst::State::Null).expect("stop");
        watch.stopping();
        let shared = Arc::clone(&watch.stop);
        drop_in_time(watch);
        assert_eq!(Arc::strong_count(&shared), 1, "the thread was not joined");
        wait_for_no_bus_thread();
    }

    #[test]
    fn a_pipeline_that_was_only_built_has_no_thread_and_is_freed() {
        let _counting = BUS_THREADS.lock().unwrap_or_else(PoisonError::into_inner);
        gst::init().expect("gstreamer init");
        for factory in ["playbin", "videoconvert", "videocrop", "waylandsink"] {
            if gst::ElementFactory::find(factory).is_none() {
                eprintln!("skipping: GStreamer element '{factory}' is not installed");
                return;
            }
        }

        // What `VideoEngine::new` builds to validate a file. Nothing is played,
        // so the file does not have to exist.
        let player = Player::build("file:///veil-test-never-played.webm", ScaleMode::Fill, None)
            .expect("build the pipeline");
        assert!(player.watch.is_none());
        assert_eq!(bus_threads(), 0);

        // Nobody reads this bus. A message left on it refers to its sender, here
        // the pipeline itself, which must not keep the pipeline alive.
        let unread = gst::message::Application::builder(gst::Structure::new_empty("veil-test"))
            .src(&player.pipeline)
            .build();
        player
            .pipeline
            .bus()
            .expect("bus")
            .post(unread)
            .expect("post");

        let pipeline = player.pipeline.downgrade();
        let sink = player.waylandsink.downgrade();
        drop(player);
        assert!(pipeline.upgrade().is_none(), "the pipeline is still alive");
        assert!(sink.upgrade().is_none(), "the sink is still alive");
    }

    #[test]
    fn a_pipeline_is_reused_until_it_has_a_window_and_rebuilt_after_detach() {
        gst::init().expect("gstreamer init");
        for factory in ["playbin", "videoconvert", "videocrop", "waylandsink"] {
            if gst::ElementFactory::find(factory).is_none() {
                eprintln!("skipping: GStreamer element '{factory}' is not installed");
                return;
            }
        }

        // Only the path has to exist: the pipeline is built but never started.
        let path =
            std::env::temp_dir().join(format!("veil-test-{}-detach.webm", std::process::id()));
        std::fs::write(&path, b"").expect("write test file");
        let options = ScaleOptions {
            mode: ScaleMode::Fit,
            bg_color: Color::default(),
        };
        let engine = VideoEngine::new(path.to_str().expect("utf-8 temp path"), options);
        std::fs::remove_file(&path).expect("remove test file");
        let mut engine = engine.expect("build the pipeline");
        let validated = engine
            .player
            .as_ref()
            .expect("built by new")
            .pipeline
            .clone();

        // The first surface gets the pipeline that `new` built to validate the
        // file, laid out for that surface.
        let scale = SurfaceScale::from_120ths(180);
        let player = engine.fresh_player((1280, 720), scale).expect("reuse");
        assert_eq!(player.pipeline, validated);
        {
            let state = player.geometry.state.lock().expect("state");
            assert_eq!(state.surface, Some((1280, 720)));
            assert_eq!(state.scale, scale);
        }

        engine.detach();
        assert!(engine.player.is_none());
        // A second detach, and pause/resume without a pipeline, are harmless.
        engine.detach();
        engine.pause();
        engine.resume();

        // The next surface gets a new pipeline, which starts with its inputs.
        let player = engine.fresh_player((1920, 1080), scale).expect("rebuild");
        assert_ne!(player.pipeline, validated);
        assert!(!player.attached);
        {
            let state = player.geometry.state.lock().expect("state");
            assert_eq!(state.mode, ScaleMode::Fit);
            assert_eq!(state.surface, Some((1920, 1080)));
            assert_eq!(state.scale, scale);
            assert_eq!(state.frame, None);
        }

        // A pipeline that already has a window is never handed out again.
        player.attached = true;
        let used = player.pipeline.clone();
        let player = engine.fresh_player((1920, 1080), scale).expect("rebuild");
        assert_ne!(player.pipeline, used);
        assert!(!player.attached);
    }

    #[test]
    fn attach_needs_a_real_surface() {
        gst::init().expect("gstreamer init");
        for factory in ["playbin", "videoconvert", "videocrop", "waylandsink"] {
            if gst::ElementFactory::find(factory).is_none() {
                eprintln!("skipping: GStreamer element '{factory}' is not installed");
                return;
            }
        }

        let path =
            std::env::temp_dir().join(format!("veil-test-{}-no-window.webm", std::process::id()));
        std::fs::write(&path, b"").expect("write test file");
        let engine = VideoEngine::new(
            path.to_str().expect("utf-8 temp path"),
            ScaleOptions::default(),
        );
        std::fs::remove_file(&path).expect("remove test file");
        let mut engine = engine.expect("build the pipeline");

        // A target without Wayland handles is refused before anything is touched:
        // nothing is drawn and the pipeline is still the unused one.
        let mut target = FakeTarget::new((1920, 1080));
        let err = engine
            .attach(&mut RenderContext::new(
                (1920, 1080),
                SurfaceScale::ONE,
                &mut target,
            ))
            .expect_err("no window");
        assert!(matches!(err, VeilError::Gstreamer(_)), "{err:?}");
        assert!(target.shown.is_empty());
        assert!(engine
            .player
            .as_ref()
            .is_some_and(|player| !player.attached));
    }
}
