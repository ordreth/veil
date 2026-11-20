//! veil's video sink: shows the frames of a pipeline in a [`VideoPlane`].
//!
//! It takes the place of `waylandsink` (see the module docs of
//! `engine::video`), and of the `videoconvert ! videocrop` in front of it.
//! Every frame is converted to XRGB8888 by GStreamer's own video converter
//! (the code `videoconvert` runs), straight into the memory of a `wl_shm`
//! buffer, and cropped in the same pass: what `fill` and `center` crop away is
//! never converted. The compositor scales the rest to its place on the screen,
//! through the plane's viewport, on the GPU.
//!
//! The converter does not scale. It can, but scaling takes it off its fast
//! path: 600 frames of 4K H.264 converted to 1080p took 13.1 s more than
//! decoding them alone with its bilinear scaler (16.4 s with lanczos), and
//! 8.5 s converted at their own size (measured with `videoconvertscale`,
//! which runs the same code, 2026-11-09). Only nearest-neighbour was faster
//! (6.3 s), and it shows.
//!
//! The conversion runs in `prepare`, before the sink waits for the frame's
//! time, and the frame is shown in `show_frame`, at its time. A frame that
//! comes while the compositor has not taken the one before (it answers the
//! frame callback when it draws the surface) is dropped, as `waylandsink`
//! does: the sink's `stats` then count it as dropped, which is how the daemon
//! sees that nobody is shown the video (`wayland::unseen`).

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use gst::glib;
use gst::subclass::prelude::*;
use gst_video::gst_base;
use gst_video::gst_base::prelude::*;
use gst_video::subclass::prelude::*;

use super::{Crop, Geometry, VideoLayout};
use crate::engine::VideoPlane;
use crate::error::VeilError;
use crate::scaling::{Rect, SurfaceScale};
use gstreamer as gst;
use gstreamer_video as gst_video;

/// Buffers per plane at first: one on screen, one to convert the next frame
/// into. A compositor that copies the frame into a texture when it is
/// committed releases the buffer before the next one is converted.
const FIRST_BUFFERS: usize = 2;

/// Buffers per plane at most: a third for a compositor that holds the frame
/// it shows until the next one replaces it (a headless sway, which renders on
/// the CPU). A 4K buffer is 33 MB.
const MAX_BUFFERS: usize = 3;

/// Threads of the converter. One cannot convert 4K at 60 fps (measured with
/// `videoconvert`: 45 to 51 frames a second with one, 60 with two).
const CONVERTER_THREADS: u32 = 2;

glib::wrapper! {
    pub struct PlaneSink(ObjectSubclass<imp::PlaneSink>)
        @extends gst_video::VideoSink, gst_base::BaseSink, gst::Element, gst::Object;
}

impl PlaneSink {
    /// A sink that lays its frames out as `geometry` says, and shows nothing
    /// until it is given a plane.
    pub(super) fn new(geometry: Arc<Geometry>) -> Self {
        let sink: Self = glib::Object::builder().property("name", "vsink").build();
        sink.imp().state().geometry = Some(geometry);
        sink
    }

    /// The plane to show the frames in, from now on.
    pub(super) fn attach(&self, plane: Arc<dyn VideoPlane>) {
        let mut state = self.imp().state();
        state.forget_buffers();
        state.plane = Some(plane);
    }

    /// Takes the frames off the surface: closes the plane and frees its
    /// buffers. The pipeline must not be streaming any more.
    pub(super) fn detach(&self) {
        let mut state = self.imp().state();
        state.forget_buffers();
        if let Some(plane) = state.plane.take() {
            plane.close();
        }
    }

    /// Takes the plane away from the sink, open, with its frame on screen.
    /// The sink shows nothing after this and frees its buffers.
    pub(super) fn take_plane(&self) -> Option<Arc<dyn VideoPlane>> {
        let mut state = self.imp().state();
        state.forget_buffers();
        state.plane.take()
    }

    /// Shows the frame on screen again, laid out anew. For a layout that
    /// changed (the surface was resized) while no new frame comes: a paused
    /// video. Nothing happens while the compositor has not taken the frame
    /// before, or if there is none.
    pub(super) fn refresh(&self) {
        let Some(buffer) = self.last_sample().and_then(|sample| sample.buffer_owned()) else {
            return;
        };
        let mut state = self.imp().state();
        let Some(plane) = state.plane.clone() else {
            return;
        };
        if !plane.ready() {
            return;
        }
        match state.convert(plane.as_ref(), &buffer) {
            Ok(Some((index, place))) => {
                if plane.show(index, place) {
                    state.report_move();
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("cannot show the video frame again: {e}"),
        }
    }
}

/// Which frame a `GstBuffer` holds: its address and its time. A pool hands
/// the same buffer out again for later frames, with another time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameId {
    address: usize,
    pts: Option<gst::ClockTime>,
}

impl FrameId {
    fn of(buffer: &gst::BufferRef) -> Self {
        Self {
            address: buffer.as_ptr() as usize,
            pts: buffer.pts(),
        }
    }
}

/// A frame converted in `prepare`, to be shown by `show_frame`.
#[derive(Debug, Clone, Copy)]
struct Prepared {
    /// The frame it was converted from: only that one shows it.
    frame: FrameId,
    index: usize,
    place: Rect,
}

/// A converter for one stream, crop and size.
struct Converter {
    input: gst_video::VideoInfo,
    crop: Crop,
    size: (u32, u32),
    output: gst_video::VideoInfo,
    converter: gst_video::VideoConverter,
}

#[derive(Default)]
struct State {
    geometry: Option<Arc<Geometry>>,
    plane: Option<Arc<dyn VideoPlane>>,
    /// The negotiated stream.
    info: Option<gst_video::VideoInfo>,
    /// The plane's buffers, each wrapping the memory the compositor reads, so
    /// the converter writes into it.
    canvases: Vec<gst::Buffer>,
    canvas_size: (u32, u32),
    converter: Option<Converter>,
    prepared: Option<Prepared>,
    /// The frame on screen, until the next one comes. The frame shown while
    /// paused (preroll) is prepared and rendered again when the video plays,
    /// and is neither converted nor shown twice.
    shown: Option<FrameId>,
}

impl State {
    /// Frees the buffers; the next frame allocates new ones.
    fn forget_buffers(&mut self) {
        self.canvases.clear();
        self.canvas_size = (0, 0);
        self.prepared = None;
        self.shown = None;
    }

    fn report_move(&self) {
        if let Some(geometry) = &self.geometry {
            geometry.report_move();
        }
    }

    /// Replaces the buffers by `count` new ones of `size`.
    fn allocate(
        &mut self,
        plane: &dyn VideoPlane,
        size: (u32, u32),
        count: usize,
    ) -> Result<(), VeilError> {
        self.forget_buffers();
        self.canvases = plane
            .allocate(size, count)?
            .into_iter()
            .map(gst::Buffer::from_mut_slice)
            .collect();
        self.canvas_size = size;
        Ok(())
    }

    /// Converts `buffer` into a free buffer of `plane`, laid out as the
    /// geometry says now. Returns which buffer, and where to show it; `None`
    /// if there is nothing to show it on yet, or no free buffer.
    fn convert(
        &mut self,
        plane: &dyn VideoPlane,
        buffer: &gst::BufferRef,
    ) -> Result<Option<(usize, Rect)>, VeilError> {
        let failed = |what: &str| VeilError::Gstreamer(format!("video sink: {what}"));
        let Some(info) = self.info.clone() else {
            return Err(failed("a frame came before its caps"));
        };
        let Some((layout, scale)) = self.geometry.as_ref().and_then(|g| g.current()) else {
            return Ok(None);
        };
        let crop = visible(&info, layout.crop);
        let size = buffer_size(layout, scale, plane.scales(), crop);

        if self.canvases.is_empty() || self.canvas_size != size {
            self.allocate(plane, size, FIRST_BUFFERS)?;
        }
        let prepared = self.prepared.map(|prepared| prepared.index);
        let free = |canvases: &[gst::Buffer]| {
            (0..canvases.len()).find(|&index| Some(index) != prepared && plane.is_free(index))
        };
        let index = match free(&self.canvases) {
            Some(index) => index,
            None if self.canvases.len() < MAX_BUFFERS => {
                // The compositor holds every buffer: one more, for good. The
                // frame on screen stays (the compositor has its own mapping).
                let count = self.canvases.len() + 1;
                tracing::debug!("video sink: the compositor holds every buffer; using {count}");
                self.allocate(plane, size, count)?;
                0
            }
            None => return Ok(None),
        };

        let reuse = self.converter.as_ref().is_some_and(|converter| {
            converter.input == info && converter.crop == layout.crop && converter.size == size
        });
        if !reuse {
            self.converter = Some(new_converter(&info, layout.crop, crop, size)?);
        }
        let Some(converter) = &self.converter else {
            return Err(failed("no converter"));
        };

        let source = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
            .map_err(|_| failed("cannot map a frame"))?;
        let canvas = self.canvases[index]
            .get_mut()
            .ok_or_else(|| failed("a buffer of the plane is shared"))?;
        let mut target =
            gst_video::VideoFrameRef::from_buffer_ref_writable(canvas, &converter.output)
                .map_err(|_| failed("cannot map a buffer of the plane"))?;
        converter.converter.frame_ref(&source, &mut target);
        Ok(Some((index, layout.render)))
    }
}

/// The size of the part of a frame of `info` that `crop` leaves, at least one
/// pixel each way.
fn visible(info: &gst_video::VideoInfo, crop: Crop) -> (u32, u32) {
    let width = info.width().saturating_sub(crop.left + crop.right).max(1);
    let height = info.height().saturating_sub(crop.top + crop.bottom).max(1);
    (width, height)
}

/// The size of the buffers a frame is converted into: the visible part of the
/// frame (`crop`), which the compositor scales to its place. A plane that
/// cannot scale needs exactly the physical size of the place, and then the
/// converter scales.
fn buffer_size(
    layout: VideoLayout,
    scale: SurfaceScale,
    scales: bool,
    crop: (u32, u32),
) -> (u32, u32) {
    if scales {
        return crop;
    }
    let place = scale.physical_size((layout.render.width, layout.render.height));
    (place.0.max(1), place.1.max(1))
}

fn new_converter(
    input: &gst_video::VideoInfo,
    crop: Crop,
    visible: (u32, u32),
    size: (u32, u32),
) -> Result<Converter, VeilError> {
    let failed = |e: glib::BoolError| VeilError::Gstreamer(format!("video converter: {e}"));
    let to_i32 = |v: u32| i32::try_from(v).unwrap_or(i32::MAX);
    let output = gst_video::VideoInfo::builder(gst_video::VideoFormat::Bgrx, size.0, size.1)
        .fps(input.fps())
        .interlace_mode(input.interlace_mode())
        .build()
        .map_err(failed)?;
    let mut config = gst_video::VideoConverterConfig::new();
    config.set_src_x(to_i32(crop.left));
    config.set_src_y(to_i32(crop.top));
    config.set_src_width(Some(to_i32(visible.0)));
    config.set_src_height(Some(to_i32(visible.1)));
    config.set_dest_x(0);
    config.set_dest_y(0);
    config.set_dest_width(Some(to_i32(size.0)));
    config.set_dest_height(Some(to_i32(size.1)));
    config.set_threads(CONVERTER_THREADS);
    let converter = gst_video::VideoConverter::new(input, &output, Some(config)).map_err(failed)?;
    tracing::debug!(
        "video converter: {}x{} {:?}, crop {crop:?}, to {}x{} XRGB8888",
        input.width(),
        input.height(),
        input.format(),
        size.0,
        size.1
    );
    Ok(Converter {
        input: input.clone(),
        crop,
        size,
        output,
        converter,
    })
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PlaneSink {
        state: Mutex<State>,
    }

    impl PlaneSink {
        pub(super) fn state(&self) -> MutexGuard<'_, State> {
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// Whether a frame is shown to preroll: the sink is not `Playing`, or
        /// is on its way from there to `Paused` (it still says `Playing` then).
        fn prerolling(&self) -> bool {
            let sink = self.obj();
            sink.current_state() != gst::State::Playing
                || sink.pending_state() == gst::State::Paused
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlaneSink {
        const NAME: &'static str = "VeilPlaneSink";
        type Type = super::PlaneSink;
        type ParentType = gst_video::VideoSink;
    }

    impl ObjectImpl for PlaneSink {}

    impl GstObjectImpl for PlaneSink {}

    impl ElementImpl for PlaneSink {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static METADATA: OnceLock<gst::subclass::ElementMetadata> = OnceLock::new();
            Some(METADATA.get_or_init(|| {
                gst::subclass::ElementMetadata::new(
                    "veil plane sink",
                    "Sink/Video",
                    "Shows video frames in a subsurface of veil's wallpaper surface",
                    "veil",
                )
            }))
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            static TEMPLATES: OnceLock<Vec<gst::PadTemplate>> = OnceLock::new();
            TEMPLATES.get_or_init(|| {
                // Every raw format the converter reads, in system memory.
                let caps = gst_video::VideoCapsBuilder::new().build();
                match gst::PadTemplate::new(
                    "sink",
                    gst::PadDirection::Sink,
                    gst::PadPresence::Always,
                    &caps,
                ) {
                    Ok(template) => vec![template],
                    Err(e) => {
                        tracing::error!("cannot make the pad template of the video sink: {e}");
                        Vec::new()
                    }
                }
            })
        }
    }

    impl BaseSinkImpl for PlaneSink {
        fn set_caps(&self, caps: &gst::Caps) -> Result<(), gst::LoggableError> {
            let info = gst_video::VideoInfo::from_caps(caps)
                .map_err(|_| gst::loggable_error!(gst::CAT_RUST, "not raw video: {caps}"))?;
            let mut state = self.state();
            state.info = Some(info);
            state.converter = None;
            Ok(())
        }

        /// Frames with a `GstVideoMeta` (strides and offsets of their own) are
        /// read as they are: a decoder need not copy them into a plain layout.
        fn propose_allocation(
            &self,
            query: &mut gst::query::Allocation,
        ) -> Result<(), gst::LoggableError> {
            query.add_allocation_meta::<gst_video::VideoMeta>(None);
            Ok(())
        }

        fn prepare(&self, buffer: &gst::Buffer) -> Result<gst::FlowSuccess, gst::FlowError> {
            let frame = FrameId::of(buffer);
            let mut state = self.state();
            state.prepared = None;
            if state.shown == Some(frame) {
                return Ok(gst::FlowSuccess::Ok);
            }
            state.shown = None;
            let Some(plane) = state.plane.clone() else {
                return Ok(gst::FlowSuccess::Ok);
            };
            match state.convert(plane.as_ref(), buffer) {
                Ok(Some((index, place))) => {
                    state.prepared = Some(Prepared {
                        frame,
                        index,
                        place,
                    });
                    Ok(gst::FlowSuccess::Ok)
                }
                Ok(None) => Ok(gst::FlowSuccess::Ok),
                Err(e) => {
                    gst::element_imp_error!(self, gst::StreamError::Failed, ["{e}"]);
                    Err(gst::FlowError::Error)
                }
            }
        }

        fn stop(&self) -> Result<(), gst::ErrorMessage> {
            let mut state = self.state();
            state.prepared = None;
            state.shown = None;
            state.converter = None;
            state.info = None;
            Ok(())
        }
    }

    impl VideoSinkImpl for PlaneSink {
        fn show_frame(&self, buffer: &gst::Buffer) -> Result<gst::FlowSuccess, gst::FlowError> {
            let frame = FrameId::of(buffer);
            let mut state = self.state();
            if state.shown == Some(frame) {
                // The frame shown in `Paused`, rendered again in `Playing`.
                return Ok(gst::FlowSuccess::Ok);
            }
            let prepared = state.prepared.take().filter(|p| p.frame == frame);
            let plane = state.plane.clone();
            match (prepared, plane) {
                (Some(prepared), Some(plane)) if plane.ready() => {
                    if plane.show(prepared.index, prepared.place) {
                        state.report_move();
                    }
                    state.shown = Some(frame);
                    Ok(gst::FlowSuccess::Ok)
                }
                // No surface, no free buffer, or the frame before is not
                // drawn yet: `GST_BASE_SINK_FLOW_DROPPED`. Not for the frame
                // a pipeline prerolls on its way to `Paused`: `basesink` takes
                // anything but `OK` there for a failed preroll, and the state
                // change would never end (a video paused while it is hidden).
                // That frame is rendered again when the video plays.
                _ if self.prerolling() => Ok(gst::FlowSuccess::Ok),
                _ => Ok(gst::FlowSuccess::CustomSuccess),
            }
        }
    }
}
