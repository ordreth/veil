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
//!
//! # Frames in dmabufs
//!
//! A decoder on the GPU (VA-API on Intel and AMD) can hand out its frames in
//! dmabufs, in GPU memory. When the compositor takes the frame's format and
//! modifier (the plane says which: [`VideoPlane::dmabuf_formats`]), the sink
//! offers them in its caps (`video/x-raw(memory:DMABuf)`, `DMA_DRM`), and the
//! decoder's frames go to the compositor as they are: no conversion, no copy,
//! and the crop is the viewport's. Nothing is converted on the CPU then. Any
//! other decoder (NVIDIA's give system, CUDA or GL memory) negotiates the
//! system-memory caps, which come second, and its frames are converted as
//! above. A compositor that refuses a dmabuf after all makes the sink offer
//! system memory only; so does a video whose colours the compositor would
//! show wrong, because it cannot be told how they are coded
//! ([`VideoPlane::shows_coding`]). The coding is the caps' `colorimetry`, or,
//! without one, what GStreamer's converter assumes, so a video has the same
//! colours either way. The sink then asks for the pipeline to be started
//! again (`RENEGOTIATE_MESSAGE`): a VA-API decoder does not negotiate again
//! while it plays, whatever the sink asks. Until then it drops the frames.

use std::os::fd::BorrowedFd;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use gst::glib;
use gst::subclass::prelude::*;
use gst_video::gst_base;
use gst_video::gst_base::prelude::*;
use gst_video::subclass::prelude::*;

use super::{Crop, Geometry, VideoLayout, RENEGOTIATE_MESSAGE};
use crate::engine::{
    ColorCoding, ColorMatrix, DmabufFrame, DmabufId, DmabufPlane, DrmFormat, Imported, VideoPlane,
};
use crate::error::VeilError;
use crate::scaling::{Rect, SurfaceScale};
use gstreamer as gst;
use gstreamer_allocators as gst_allocators;
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

/// The caps feature of frames in dmabufs.
const DMABUF: &str = "memory:DMABuf";

/// Frames in dmabufs the sink may hold at once, which the decoder's pool must
/// have on top of its own: one on screen, one the compositor has not released
/// yet, and the one being shown.
const DMABUF_HELD: u32 = 3;

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
        state.dmabuf_refused = false;
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
        match state.prepare(plane.as_ref(), &buffer) {
            Ok(Some(ready)) => {
                if ready.show(plane.as_ref(), &buffer) {
                    state.report_move();
                }
            }
            Ok(None) | Err(Failure::Refused(_)) => {}
            Err(Failure::Error(e)) => tracing::warn!("cannot show the video frame again: {e}"),
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

/// A frame made ready in `prepare`, to be shown by `show_frame`.
#[derive(Debug, Clone, Copy)]
struct Prepared {
    /// The frame it was made from: only that one shows it.
    frame: FrameId,
    ready: Ready,
}

/// A frame the plane can show.
#[derive(Debug, Clone, Copy)]
enum Ready {
    /// Converted into buffer `index` of the plane.
    Converted { index: usize, place: Rect },
    /// A dmabuf the compositor has imported, cropped to `source`.
    Dmabuf {
        id: DmabufId,
        coding: Option<ColorCoding>,
        source: Rect,
        place: Rect,
    },
}

impl Ready {
    /// Shows the frame, which is `buffer`. Returns whether the plane moved.
    fn show(self, plane: &dyn VideoPlane, buffer: &gst::Buffer) -> bool {
        match self {
            Self::Converted { index, place } => plane.show(index, place),
            // The plane holds the decoder's frame while the compositor reads it.
            Self::Dmabuf {
                id,
                coding,
                source,
                place,
            } => plane.show_dmabuf(id, Box::new(buffer.clone()), coding, source, place),
        }
    }
}

/// Why a frame cannot be shown.
#[derive(Debug)]
enum Failure {
    /// The frame is in a dmabuf the compositor does not take: the stream has
    /// to change to system memory.
    Refused(String),
    Error(VeilError),
}

/// The negotiated stream.
#[derive(Debug, Clone)]
enum Stream {
    /// Frames in system memory, which the converter turns into the plane's
    /// buffers.
    Raw(gst_video::VideoInfo),
    /// Frames in dmabufs, which the compositor reads as they are.
    Dmabuf(DmaDrm),
}

/// A stream of frames in dmabufs (`DMA_DRM` caps).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DmaDrm {
    format: DrmFormat,
    size: (u32, u32),
    /// How the colours are coded; `None` for RGB.
    coding: Option<ColorCoding>,
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
    stream: Option<Stream>,
    /// The compositor refused a dmabuf: the sink offers system memory only.
    dmabuf_refused: bool,
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

    /// Makes `buffer` ready to be shown on `plane`, laid out as the geometry
    /// says now: converted into a buffer of the plane, or imported. `None` if
    /// there is nothing to show it on yet, or no free buffer.
    fn prepare(
        &mut self,
        plane: &dyn VideoPlane,
        buffer: &gst::BufferRef,
    ) -> Result<Option<Ready>, Failure> {
        match self.stream.clone() {
            Some(Stream::Raw(info)) => self
                .convert(plane, buffer, &info)
                .map(|converted| converted.map(|(index, place)| Ready::Converted { index, place }))
                .map_err(Failure::Error),
            // Refused already: dropped until the decoder has negotiated again.
            Some(Stream::Dmabuf(_)) if self.dmabuf_refused => Ok(None),
            Some(Stream::Dmabuf(stream)) => self.import(plane, buffer, stream),
            None => Err(Failure::Error(VeilError::Gstreamer(
                "video sink: a frame came before its caps".into(),
            ))),
        }
    }

    /// Has the compositor import the dmabuf of `buffer`, which it shows
    /// cropped as the geometry says now.
    fn import(
        &self,
        plane: &dyn VideoPlane,
        buffer: &gst::BufferRef,
        stream: DmaDrm,
    ) -> Result<Option<Ready>, Failure> {
        let Some((layout, _)) = self.geometry.as_ref().and_then(|g| g.current()) else {
            return Ok(None);
        };
        let frame = dmabuf_frame(buffer, stream).map_err(Failure::Refused)?;
        match plane.import(&frame) {
            Imported::Ready(id) => {
                let (width, height) = visible(stream.size, layout.crop);
                Ok(Some(Ready::Dmabuf {
                    id,
                    coding: stream.coding,
                    source: Rect::new(layout.crop.left, layout.crop.top, width, height),
                    place: layout.render,
                }))
            }
            Imported::NotYet => Ok(None),
            Imported::Refused => Err(Failure::Refused("the compositor cannot import it".into())),
        }
    }

    /// Converts `buffer` into a free buffer of `plane`, laid out as the
    /// geometry says now. Returns which buffer, and where to show it; `None`
    /// if there is nothing to show it on yet, or no free buffer.
    fn convert(
        &mut self,
        plane: &dyn VideoPlane,
        buffer: &gst::BufferRef,
        info: &gst_video::VideoInfo,
    ) -> Result<Option<(usize, Rect)>, VeilError> {
        let failed = |what: &str| VeilError::Gstreamer(format!("video sink: {what}"));
        let Some((layout, scale)) = self.geometry.as_ref().and_then(|g| g.current()) else {
            return Ok(None);
        };
        let crop = visible((info.width(), info.height()), layout.crop);
        let size = buffer_size(layout, scale, plane.scales(), crop);

        if self.canvases.is_empty() || self.canvas_size != size {
            self.allocate(plane, size, FIRST_BUFFERS)?;
        }
        let prepared = self.prepared.and_then(|prepared| match prepared.ready {
            Ready::Converted { index, .. } => Some(index),
            Ready::Dmabuf { .. } => None,
        });
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
            converter.input == *info && converter.crop == layout.crop && converter.size == size
        });
        if !reuse {
            self.converter = Some(new_converter(info, layout.crop, crop, size)?);
        }
        let Some(converter) = &self.converter else {
            return Err(failed("no converter"));
        };

        let source = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, info)
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

/// The size of the part of a frame of `size` that `crop` leaves, at least one
/// pixel each way.
fn visible(size: (u32, u32), crop: Crop) -> (u32, u32) {
    let width = size.0.saturating_sub(crop.left + crop.right).max(1);
    let height = size.1.saturating_sub(crop.top + crop.bottom).max(1);
    (width, height)
}

/// Where the planes of a frame in dmabufs are: the frame's `GstVideoMeta`
/// says at which offset of the buffer each plane starts, and the memory found
/// there is the dmabuf (a decoder puts all of them in one, mostly).
fn dmabuf_frame(buffer: &gst::BufferRef, stream: DmaDrm) -> Result<DmabufFrame<'_>, String> {
    let meta = buffer
        .meta::<gst_video::VideoMeta>()
        .ok_or("the frame has no video meta")?;
    let count = meta.n_planes() as usize;
    let mut planes = Vec::with_capacity(count);
    for (&offset, &stride) in meta.offset().iter().zip(meta.stride()).take(count) {
        let (memories, skip) = buffer
            .find_memory(offset..offset + 1)
            .ok_or("a plane is outside the frame")?;
        let memory = buffer.peek_memory(memories.start);
        let dmabuf = memory
            .downcast_memory_ref::<gst_allocators::DmaBufMemory>()
            .ok_or("the frame is not in a dmabuf")?;
        // SAFETY: the descriptor belongs to the memory, which `buffer` holds
        // for as long as the frame borrows it.
        let fd = unsafe { BorrowedFd::borrow_raw(dmabuf.fd()) };
        let offset = memory
            .offset()
            .checked_add(skip)
            .and_then(|offset| u32::try_from(offset).ok())
            .ok_or("a plane offset is too large")?;
        let stride = u32::try_from(stride).map_err(|_| "a plane has a negative stride")?;
        planes.push(DmabufPlane { fd, offset, stride });
    }
    if planes.is_empty() {
        return Err("the frame has no planes".into());
    }
    Ok(DmabufFrame {
        format: stream.format,
        size: stream.size,
        planes,
    })
}

/// The name GStreamer gives a dmabuf format in caps (`drm-format`): the
/// fourcc, and its modifier unless it is linear, as `NV12:0x0100000000000002`.
/// `None` for a fourcc that is not text.
fn drm_format_name(format: DrmFormat) -> Option<String> {
    let bytes = format.fourcc.to_le_bytes();
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b' ')
    {
        return None;
    }
    let fourcc: String = bytes.iter().map(|&b| char::from(b)).collect();
    Some(match format.modifier {
        0 => fourcc,
        modifier => format!("{fourcc}:0x{modifier:016x}"),
    })
}

/// The format `drm_format_name` names.
fn parse_drm_format(name: &str) -> Option<DrmFormat> {
    let (fourcc, modifier) = match name.split_once(':') {
        Some((fourcc, modifier)) => {
            let hex = modifier.strip_prefix("0x")?;
            (fourcc, u64::from_str_radix(hex, 16).ok()?)
        }
        None => (name, 0),
    };
    let bytes: [u8; 4] = fourcc.as_bytes().try_into().ok()?;
    Some(DrmFormat {
        fourcc: u32::from_le_bytes(bytes),
        modifier,
    })
}

/// Whether `caps` (fixed) are of frames in dmabufs.
fn is_dmabuf(caps: &gst::CapsRef) -> bool {
    caps.features(0)
        .is_some_and(|features| features.contains(DMABUF))
}

/// The stream of fixed `DMA_DRM` caps.
fn dma_drm(caps: &gst::CapsRef) -> Option<DmaDrm> {
    let structure = caps.structure(0)?;
    if structure.get::<&str>("format").ok()? != "DMA_DRM" {
        return None;
    }
    let format = parse_drm_format(structure.get::<&str>("drm-format").ok()?)?;
    let width = u32::try_from(structure.get::<i32>("width").ok()?).ok()?;
    let height = u32::try_from(structure.get::<i32>("height").ok()?).ok()?;
    let coding = match format.is_ycbcr()? {
        true => Some(coding(structure.get::<&str>("colorimetry").ok(), height)),
        false => None,
    };
    (width > 0 && height > 0).then_some(DmaDrm {
        format,
        size: (width, height),
        coding,
    })
}

/// How the colours of a YCbCr stream of `height` rows with `colorimetry`
/// (caps) are coded. What the colorimetry leaves open is what GStreamer
/// assumes for such a stream (`video-info.c`): BT.2020 from 2160 rows on,
/// BT.709 above 576, BT.601 below, and the limited range.
fn coding(colorimetry: Option<&str>, height: u32) -> ColorCoding {
    let colorimetry = colorimetry.and_then(|c| c.parse::<gst_video::VideoColorimetry>().ok());
    let matrix = match colorimetry.as_ref().map(|c| c.matrix()) {
        Some(gst_video::VideoColorMatrix::Bt709) => ColorMatrix::Bt709,
        Some(gst_video::VideoColorMatrix::Bt601) => ColorMatrix::Bt601,
        Some(gst_video::VideoColorMatrix::Bt2020) => ColorMatrix::Bt2020,
        Some(gst_video::VideoColorMatrix::Smpte240m) => ColorMatrix::Smpte240m,
        Some(gst_video::VideoColorMatrix::Fcc) => ColorMatrix::Fcc,
        _ if height >= 2160 => ColorMatrix::Bt2020,
        _ if height > 576 => ColorMatrix::Bt709,
        _ => ColorMatrix::Bt601,
    };
    let full_range =
        colorimetry.is_some_and(|c| c.range() == gst_video::VideoColorRange::Range0_255);
    ColorCoding { matrix, full_range }
}

/// Caps of frames in dmabufs of `formats`; `None` if none can be named.
fn dmabuf_caps(formats: &[DrmFormat]) -> Option<gst::Caps> {
    // Only formats whose colours veil knows how to have shown.
    let names: Vec<String> = formats
        .iter()
        .filter(|format| format.is_ycbcr().is_some())
        .copied()
        .filter_map(drm_format_name)
        .collect();
    if names.is_empty() {
        return None;
    }
    Some(
        gst::Caps::builder("video/x-raw")
            .features([DMABUF])
            .field("format", "DMA_DRM")
            .field("drm-format", gst::List::new(names))
            .field("width", gst::IntRange::new(1, i32::MAX))
            .field("height", gst::IntRange::new(1, i32::MAX))
            .field(
                "framerate",
                gst::FractionRange::new(gst::Fraction::new(0, 1), gst::Fraction::new(i32::MAX, 1)),
            )
            .build(),
    )
}

/// Every raw format the converter reads, in system memory.
fn raw_caps() -> gst::Caps {
    gst_video::VideoCapsBuilder::new().build()
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

/// `input`, a YCbCr stream, with what its colorimetry leaves open filled in
/// as [`coding`] does. GStreamer's converter takes an unknown range for the
/// full one, which shows a video of the limited range (nearly every one)
/// washed out: VA-API decoders in system memory say `0:3:0:0`, BT.709 with
/// an unknown range (known issue 9). The frames themselves are mapped with
/// `input`: the converter only checks their format and size. `input` as it
/// is if it cannot be told otherwise.
fn with_known_coding(input: &gst_video::VideoInfo) -> gst_video::VideoInfo {
    if !input.is_yuv() {
        return input.clone();
    }
    let given = input.colorimetry();
    let known = coding(Some(&given.to_string()), input.height());
    let matrix = match known.matrix {
        ColorMatrix::Bt601 => gst_video::VideoColorMatrix::Bt601,
        ColorMatrix::Bt709 => gst_video::VideoColorMatrix::Bt709,
        ColorMatrix::Bt2020 => gst_video::VideoColorMatrix::Bt2020,
        ColorMatrix::Smpte240m => gst_video::VideoColorMatrix::Smpte240m,
        ColorMatrix::Fcc => gst_video::VideoColorMatrix::Fcc,
    };
    let range = if known.full_range {
        gst_video::VideoColorRange::Range0_255
    } else {
        gst_video::VideoColorRange::Range16_235
    };
    if given.matrix() == matrix && given.range() == range {
        return input.clone();
    }
    let colorimetry =
        gst_video::VideoColorimetry::new(range, matrix, given.transfer(), given.primaries());
    let filled = input.to_caps().and_then(|mut caps| {
        caps.make_mut().set("colorimetry", colorimetry.to_string());
        gst_video::VideoInfo::from_caps(&caps)
    });
    match filled {
        Ok(filled) => filled,
        Err(e) => {
            tracing::debug!("video converter: cannot say the coding {colorimetry}: {e}");
            input.clone()
        }
    }
}

fn new_converter(
    input: &gst_video::VideoInfo,
    crop: Crop,
    visible: (u32, u32),
    size: (u32, u32),
) -> Result<Converter, VeilError> {
    let failed = |e: glib::BoolError| VeilError::Gstreamer(format!("video converter: {e}"));
    let to_i32 = |v: u32| i32::try_from(v).unwrap_or(i32::MAX);
    let coded = with_known_coding(input);
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
    let converter =
        gst_video::VideoConverter::new(&coded, &output, Some(config)).map_err(failed)?;
    tracing::debug!(
        "video converter: {}x{} {:?} ({}), crop {crop:?}, to {}x{} XRGB8888",
        input.width(),
        input.height(),
        input.format(),
        coded.colorimetry(),
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

        /// Asks for the pipeline to be started again: the sink offers other
        /// caps now, and the decoder negotiates only when it starts.
        fn renegotiate(&self) {
            let sink = self.obj();
            let message =
                gst::message::Application::builder(gst::Structure::new_empty(RENEGOTIATE_MESSAGE))
                    .src(&*sink)
                    .build();
            if sink.post_message(message).is_err() {
                tracing::debug!("video sink: nobody reads the bus to start the pipeline again");
            }
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
                // Frames in dmabufs of any format (`caps` narrows them to the
                // compositor's), and every raw format the converter reads.
                let mut caps = gst::Caps::builder("video/x-raw")
                    .features([DMABUF])
                    .field("format", "DMA_DRM")
                    .build();
                caps.merge(raw_caps());
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
        /// Frames in the dmabufs the compositor takes first, then system
        /// memory. Only system memory once the compositor refused a dmabuf.
        fn caps(&self, filter: Option<&gst::Caps>) -> Option<gst::Caps> {
            let dmabuf = {
                let state = self.state();
                match &state.plane {
                    Some(plane) if !state.dmabuf_refused => dmabuf_caps(&plane.dmabuf_formats()),
                    _ => None,
                }
            };
            let mut caps = dmabuf.unwrap_or_else(gst::Caps::new_empty);
            caps.merge(raw_caps());
            Some(match filter {
                Some(filter) => filter.intersect_with_mode(&caps, gst::CapsIntersectMode::First),
                None => caps,
            })
        }

        fn set_caps(&self, caps: &gst::Caps) -> Result<(), gst::LoggableError> {
            let stream =
                if is_dmabuf(caps) {
                    Stream::Dmabuf(dma_drm(caps).ok_or_else(|| {
                        gst::loggable_error!(gst::CAT_RUST, "not a dmabuf format: {caps}")
                    })?)
                } else {
                    Stream::Raw(gst_video::VideoInfo::from_caps(caps).map_err(|_| {
                        gst::loggable_error!(gst::CAT_RUST, "not raw video: {caps}")
                    })?)
                };
            let mut state = self.state();
            // The dmabufs of an earlier stream do not come again.
            if let (Some(Stream::Dmabuf(_)), Some(plane)) = (&state.stream, &state.plane) {
                plane.forget_imports();
            }
            let wrong_colours = match &stream {
                Stream::Dmabuf(DmaDrm {
                    coding: Some(coding),
                    ..
                }) => state
                    .plane
                    .as_ref()
                    .is_some_and(|plane| !plane.shows_coding(*coding)),
                _ => false,
            };
            match &stream {
                Stream::Dmabuf(dma) if !wrong_colours => tracing::info!(
                    "video frames go to the compositor as they are, in dmabufs ({}, {:?})",
                    drm_format_name(dma.format).unwrap_or_default(),
                    dma.coding
                ),
                Stream::Dmabuf(dma) => tracing::info!(
                    "the compositor cannot be told how the video's colours are coded ({:?}); \
                     converting its frames on the CPU",
                    dma.coding
                ),
                Stream::Raw(info) => tracing::info!(
                    "video frames are converted on the CPU, from {:?} ({})",
                    info.format(),
                    info.colorimetry()
                ),
            }
            state.stream = Some(stream);
            state.converter = None;
            if wrong_colours {
                // The frames that come in dmabufs are dropped until the
                // pipeline has started again, with frames in system memory.
                state.dmabuf_refused = true;
                drop(state);
                self.renegotiate();
            }
            Ok(())
        }

        /// Frames with a `GstVideoMeta` (strides and offsets of their own) are
        /// read as they are: a decoder need not copy them into a plain layout.
        /// A decoder that hands out dmabufs needs a few more of them in its
        /// pool for the ones the sink holds while the compositor reads them.
        fn propose_allocation(
            &self,
            query: &mut gst::query::Allocation,
        ) -> Result<(), gst::LoggableError> {
            query.add_allocation_meta::<gst_video::VideoMeta>(None);
            if query.get().0.is_some_and(is_dmabuf) {
                query.add_allocation_pool(None::<&gst::BufferPool>, 0, DMABUF_HELD, 0);
            }
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
            match state.prepare(plane.as_ref(), buffer) {
                Ok(Some(ready)) => {
                    state.prepared = Some(Prepared { frame, ready });
                    Ok(gst::FlowSuccess::Ok)
                }
                Ok(None) => Ok(gst::FlowSuccess::Ok),
                Err(Failure::Refused(why)) => {
                    // The frame is dropped, and the pipeline started again
                    // with frames in system memory.
                    state.dmabuf_refused = true;
                    drop(state);
                    tracing::warn!(
                        "the compositor cannot show the video's frames in GPU memory ({why}); \
                         converting them on the CPU"
                    );
                    self.renegotiate();
                    Ok(gst::FlowSuccess::Ok)
                }
                Err(Failure::Error(e)) => {
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
            state.stream = None;
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
                    if prepared.ready.show(plane.as_ref(), buffer) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `NV12` with Intel's Y tiling, as VA-API decoders give it.
    const NV12_Y_TILED: DrmFormat = DrmFormat {
        fourcc: u32::from_le_bytes(*b"NV12"),
        modifier: 0x0100_0000_0000_0002,
    };

    #[test]
    fn dmabuf_formats_are_named_as_gstreamer_names_them() {
        assert_eq!(
            drm_format_name(NV12_Y_TILED).as_deref(),
            Some("NV12:0x0100000000000002")
        );
        // A linear one has no modifier in its name.
        let linear = DrmFormat {
            modifier: 0,
            ..NV12_Y_TILED
        };
        assert_eq!(drm_format_name(linear).as_deref(), Some("NV12"));
        let binary = DrmFormat {
            fourcc: 0x0001_0203,
            modifier: 0,
        };
        assert_eq!(drm_format_name(binary), None);
        for format in [NV12_Y_TILED, linear] {
            let name = drm_format_name(format).expect("a name");
            assert_eq!(parse_drm_format(&name), Some(format));
        }
        assert_eq!(parse_drm_format("NV12:2"), None);
        assert_eq!(parse_drm_format("NV1"), None);
    }

    #[test]
    fn the_sink_offers_the_compositors_dmabuf_formats_before_system_memory() {
        gst::init().expect("gstreamer init");
        let mut caps = dmabuf_caps(&[NV12_Y_TILED]).expect("dmabuf caps");
        caps.merge(raw_caps());
        let fixed = gst::Caps::builder("video/x-raw")
            .features([DMABUF])
            .field("format", "DMA_DRM")
            .field("drm-format", "NV12:0x0100000000000002")
            .field("width", 1920)
            .field("height", 1080)
            .field("framerate", gst::Fraction::new(30, 1))
            .build();
        assert!(fixed.is_subset(&caps));
        assert!(is_dmabuf(&fixed));
        assert_eq!(
            dma_drm(&fixed),
            Some(DmaDrm {
                format: NV12_Y_TILED,
                size: (1920, 1080),
                coding: Some(ColorCoding {
                    matrix: ColorMatrix::Bt709,
                    full_range: false
                })
            })
        );
        // Another modifier is not taken.
        let linear = gst::Caps::builder("video/x-raw")
            .features([DMABUF])
            .field("format", "DMA_DRM")
            .field("drm-format", "NV12")
            .field("width", 1920)
            .field("height", 1080)
            .field("framerate", gst::Fraction::new(30, 1))
            .build();
        assert!(!linear.is_subset(&caps));
        // System memory is always taken, after the dmabufs.
        let raw = gst_video::VideoCapsBuilder::new()
            .format(gst_video::VideoFormat::Nv12)
            .width(1920)
            .height(1080)
            .framerate(gst::Fraction::new(30, 1))
            .build();
        assert!(raw.is_subset(&caps));
        assert!(!is_dmabuf(&raw));
        assert!(caps.structure(0).is_some_and(|s| s.has_field("drm-format")));
        assert_eq!(dmabuf_caps(&[]), None);
    }

    /// The XRGB8888 pixel the converter makes of an NV12 frame of one
    /// colour, whose caps say `colorimetry`.
    fn converted(colorimetry: &str, (y, u, v): (u8, u8, u8)) -> [u8; 3] {
        let (width, height) = (64, 64);
        let caps = gst_video::VideoInfo::builder(gst_video::VideoFormat::Nv12, width, height)
            .build()
            .expect("NV12 info")
            .to_caps()
            .expect("caps");
        let mut caps = caps;
        caps.make_mut().set("colorimetry", colorimetry);
        let info = gst_video::VideoInfo::from_caps(&caps).expect("info with colorimetry");
        let mut buffer = gst::Buffer::with_size(info.size()).expect("buffer");
        {
            let mut frame =
                gst_video::VideoFrameRef::from_buffer_ref_writable(buffer.make_mut(), &info)
                    .expect("frame");
            frame.plane_data_mut(0).expect("luma").fill(y);
            for pair in frame.plane_data_mut(1).expect("chroma").chunks_mut(2) {
                pair.copy_from_slice(&[u, v]);
            }
        }
        let converter = new_converter(&info, Crop::default(), (width, height), (width, height))
            .expect("converter");
        let mut canvas = gst::Buffer::with_size(converter.output.size()).expect("canvas");
        let source =
            gst_video::VideoFrameRef::from_buffer_ref_readable(&buffer, &info).expect("source");
        let mut target = gst_video::VideoFrameRef::from_buffer_ref_writable(
            canvas.make_mut(),
            &converter.output,
        )
        .expect("target");
        converter.converter.frame_ref(&source, &mut target);
        let bgrx = &target.plane_data(0).expect("pixels")[..4];
        [bgrx[2], bgrx[1], bgrx[0]]
    }

    #[test]
    fn an_unknown_range_is_converted_as_the_limited_one() {
        gst::init().expect("gstreamer init");
        let near =
            |got: [u8; 3], want: [u8; 3]| got.iter().zip(want).all(|(&g, w)| g.abs_diff(w) <= 2);
        // What VA-API decoders say in system memory: BT.709, range unknown.
        let white = converted("0:3:0:0", (235, 128, 128));
        assert!(near(white, [255, 255, 255]), "white is {white:?}");
        let black = converted("0:3:0:0", (16, 128, 128));
        assert!(near(black, [0, 0, 0]), "black is {black:?}");
        // BT.709 red, limited range (ffmpeg's colour bars).
        let red = converted("0:3:0:0", (63, 102, 240));
        assert!(near(red, [255, 0, 0]), "red is {red:?}");
        // A range that is said is kept.
        let full = converted("1:3:0:0", (235, 128, 128));
        assert!(near(full, [235, 235, 235]), "full-range white is {full:?}");
        // Nothing said at all: BT.601 below 577 rows, the limited range.
        let red = converted("0:0:0:0", (81, 90, 240));
        assert!(near(red, [255, 0, 0]), "BT.601 red is {red:?}");
    }

    #[test]
    fn the_coding_is_the_colorimetry_or_what_gstreamer_assumes() {
        gst::init().expect("gstreamer init");
        let coding = |matrix, full_range| ColorCoding { matrix, full_range };
        assert_eq!(
            super::coding(Some("bt709"), 480),
            coding(ColorMatrix::Bt709, false)
        );
        assert_eq!(
            super::coding(Some("bt601"), 1080),
            coding(ColorMatrix::Bt601, false)
        );
        assert_eq!(
            super::coding(Some("1:4:0:0"), 1080),
            coding(ColorMatrix::Bt601, true)
        );
        assert_eq!(
            super::coding(None, 2160),
            coding(ColorMatrix::Bt2020, false)
        );
        assert_eq!(super::coding(None, 1080), coding(ColorMatrix::Bt709, false));
        assert_eq!(super::coding(None, 576), coding(ColorMatrix::Bt601, false));
        assert_eq!(
            super::coding(Some("not a colorimetry"), 720),
            coding(ColorMatrix::Bt709, false)
        );
    }
}
