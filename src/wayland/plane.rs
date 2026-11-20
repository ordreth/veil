//! The subsurface a video shows its frames in: the Wayland side of
//! [`VideoPlane`].
//!
//! It replaces GStreamer's `waylandsink`, which brought Wayland client code of
//! its own onto veil's connection: it bound every `wl_output` and handled their
//! events on its own thread, and the second `wl_output.done` of any output
//! (every change of a monitor) crashed the process there
//! (`docs/known-issues.md`, entry 1). The plane binds nothing. Its objects are
//! made on veil's own queue, so their events are handled by the event loop,
//! where they only flip a flag.
//!
//! Requests are sent from the video's streaming thread (libwayland sends from
//! any thread). libwayland only buffers them, and the event loop flushes when
//! it wakes, which it does not for a frame of the video: the plane flushes
//! after every frame itself. Without that the compositor saw most frames late
//! and answered their callbacks late, and the sink dropped nine frames in ten
//! (measured). The plane's state is behind a mutex, so a frame shown from the
//! streaming thread and `close` from the event loop cannot interleave.
//!
//! # Frames in dmabufs
//!
//! A decoder on the GPU (VA-API) hands out its frames in dmabufs, which the
//! compositor can read as they are, in their own format (NV12, mostly):
//! nothing is converted or copied on the CPU. The plane makes a `wl_buffer`
//! of each dmabuf the first time it comes and keeps it (a decoder reuses a
//! few dmabufs over and over), crops it with the viewport's source, and
//! keeps the decoder's frame (`GstBuffer`) until the compositor releases the
//! buffer, so the decoder cannot write the next frame into memory on screen.
//!
//! A `wl_buffer` is made with `zwp_linux_buffer_params_v1.create`, and the
//! plane waits for the answer. Not with `create_immed`: a compositor that
//! cannot import a dmabuf made that way may end the connection (wlroots
//! does), and the daemon with it. The answers and the buffers' `release`
//! events come on a queue of the plane's own, which only the streaming thread
//! dispatches; the event loop reads the socket, as for everything else.
//!
//! The compositor turns a YCbCr frame into RGB, and is told how its colours
//! are coded through `wp_color_representation_v1` (`dmabuf.rs`). The surface
//! has that object only while it shows such a frame: a coding of YCbCr on a
//! surface that shows an RGB buffer is an error that ends the connection.

use std::os::fd::AsFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    error::GlobalError,
    globals::ProvidesBoundGlobal,
    reexports::protocols::wp::{
        color_representation::v1::client::{
            wp_color_representation_manager_v1::WpColorRepresentationManagerV1,
            wp_color_representation_surface_v1::WpColorRepresentationSurfaceV1,
        },
        linux_dmabuf::zv1::client::{
            zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
            zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        },
        viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
    },
    shm::raw::RawPool,
    subcompositor::SubcompositorState,
};
use wayland_client::{
    event_created_child,
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_callback::{self, WlCallback},
        wl_shm::{self, WlShm},
        wl_subsurface::WlSubsurface,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
};

use super::dmabuf::{self, PlaneDmabuf};
use super::state::AppState;
use crate::engine::{
    ColorCoding, DmabufFrame, DmabufId, DrmFormat, Imported, PlaneCanvas, VideoPlane,
};
use crate::error::VeilError;
use crate::scaling::Rect;

/// Bytes per `wl_shm` XRGB8888 pixel.
const BYTES_PER_PIXEL: usize = 4;

/// The `wl_shm` global, in the form `RawPool::new` takes it.
struct BoundShm<'a>(&'a WlShm);

impl ProvidesBoundGlobal<WlShm, 1> for BoundShm<'_> {
    fn bound_global(&self) -> Result<WlShm, GlobalError> {
        Ok(self.0.clone())
    }
}

/// The memory of one buffer: a pool of its own, exactly as large as the
/// buffer. The `wl_buffer` made from it lives in the plane.
struct ShmCanvas {
    pool: RawPool,
    len: usize,
}

impl AsMut<[u8]> for ShmCanvas {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.pool.mmap()[..self.len]
    }
}

/// User data of a buffer: set while the compositor holds it, cleared by its
/// `release`.
pub(super) struct Held(Arc<AtomicBool>);

/// User data of a frame callback: set when the compositor has drawn the frame.
pub(super) struct Taken(Arc<AtomicBool>);

/// User data of the frame callback of a parked plane: the surface it is a
/// subsurface of, which the compositor draws again.
pub(super) struct Drawn(WlSurface);

struct PlaneBuffer {
    buffer: WlBuffer,
    held: Arc<AtomicBool>,
}

/// What the plane has told the compositor, so a frame repeats only what
/// changed.
struct Shown {
    position: Option<(i32, i32)>,
    destination: Option<(i32, i32)>,
    /// The viewport's source: the crop of a dmabuf frame. `None` for the
    /// whole buffer (a `wl_shm` one is cropped by the converter).
    source: Option<Rect>,
    buffer_scale: i32,
}

struct Objects {
    parent: WlSurface,
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: Option<WpViewport>,
    buffers: Vec<PlaneBuffer>,
    buffer_size: (u32, u32),
    shown: Shown,
    /// How the colours of the YCbCr frame shown are coded, and what told the
    /// compositor. `None` while the plane shows RGB.
    representation: Option<(WpColorRepresentationSurfaceV1, ColorCoding)>,
}

impl Objects {
    /// Takes the coding of YCbCr off the surface, before an RGB buffer.
    fn unset_coding(&mut self) {
        if let Some((representation, _)) = self.representation.take() {
            representation.destroy();
        }
    }
}

/// A desynchronized subsurface of a layer surface, with a few `wl_shm`
/// buffers. Desynchronized, as `waylandsink` made it: a frame takes effect
/// with the plane's own commit. The event loop still commits the parent once
/// per frame while a video plays, which Hyprland needs (`docs/known-issues.md`,
/// entry 6).
pub(super) struct Plane {
    /// `None` once closed.
    objects: Mutex<Option<Objects>>,
    /// Whether the frame shown last has been drawn (no frame callback is
    /// outstanding).
    taken: Arc<AtomicBool>,
    shm: WlShm,
    /// Dmabuf frames: `None` if the compositor takes none, or the plane
    /// cannot crop and scale them (no viewport).
    dmabuf: Option<Imports>,
    qh: QueueHandle<AppState>,
    connection: Connection,
}

impl Plane {
    /// A plane above `parent`, covering nothing until its first frame.
    /// `dmabuf` is the global and the formats to show dmabufs with.
    pub(super) fn new(
        parent: &WlSurface,
        subcompositor: &SubcompositorState,
        viewporter: Option<&WpViewporter>,
        shm: &WlShm,
        dmabuf: Option<PlaneDmabuf>,
        qh: &QueueHandle<AppState>,
        connection: &Connection,
    ) -> Self {
        let (subsurface, surface) = subcompositor.create_subsurface(parent.clone(), qh);
        subsurface.set_desync();
        let viewport = viewporter.map(|viewporter| viewporter.get_viewport(&surface, qh, ()));
        let dmabuf = dmabuf
            .filter(|_| viewport.is_some())
            .map(|dmabuf| Imports::new(dmabuf, connection));
        Self {
            objects: Mutex::new(Some(Objects {
                parent: parent.clone(),
                surface,
                subsurface,
                viewport,
                buffers: Vec::new(),
                buffer_size: (0, 0),
                shown: Shown {
                    position: None,
                    destination: None,
                    source: None,
                    buffer_scale: 1,
                },
                representation: None,
            })),
            taken: Arc::new(AtomicBool::new(true)),
            shm: shm.clone(),
            dmabuf,
            qh: qh.clone(),
            connection: connection.clone(),
        }
    }

    fn objects(&self) -> std::sync::MutexGuard<'_, Option<Objects>> {
        self.objects.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends what is buffered now. A socket that is full is flushed again by
    /// the event loop, once the compositor has read it.
    fn flush(&self) {
        if let Err(e) = self.connection.flush() {
            tracing::debug!("video plane: flushing the connection: {e}");
        }
    }
}

/// Converts a buffer or rectangle dimension for the protocol.
fn to_i32(value: u32) -> Result<i32, VeilError> {
    i32::try_from(value).map_err(|_| VeilError::BufferCreation(format!("size {value} too large")))
}

impl VideoPlane for Plane {
    fn allocate(&self, size: (u32, u32), count: usize) -> Result<Vec<PlaneCanvas>, VeilError> {
        let mut guard = self.objects();
        let Some(objects) = guard.as_mut() else {
            return Err(VeilError::BufferCreation(
                "the video plane was closed".into(),
            ));
        };
        let invalid = || VeilError::BufferCreation(format!("invalid size {}x{}", size.0, size.1));
        if size.0 == 0 || size.1 == 0 {
            return Err(invalid());
        }
        let width = to_i32(size.0)?;
        let height = to_i32(size.1)?;
        let stride = width
            .checked_mul(BYTES_PER_PIXEL as i32)
            .ok_or_else(invalid)?;
        let len = usize::try_from(stride)
            .ok()
            .and_then(|stride| stride.checked_mul(size.1 as usize))
            .ok_or_else(invalid)?;

        let mut buffers = Vec::with_capacity(count);
        let mut canvases = Vec::with_capacity(count);
        for _ in 0..count {
            let mut pool = RawPool::new(len, &BoundShm(&self.shm))
                .map_err(|e| VeilError::BufferCreation(e.to_string()))?;
            let held = Arc::new(AtomicBool::new(false));
            let buffer = pool.create_buffer(
                0,
                width,
                height,
                stride,
                wl_shm::Format::Xrgb8888,
                Held(Arc::clone(&held)),
                &self.qh,
            );
            buffers.push(PlaneBuffer { buffer, held });
            canvases.push(PlaneCanvas::new(Box::new(ShmCanvas { pool, len })));
        }

        // A buffer the compositor still shows keeps its content after
        // `destroy`: the compositor maps the memory itself.
        for old in std::mem::replace(&mut objects.buffers, buffers) {
            old.buffer.destroy();
        }
        objects.buffer_size = size;
        Ok(canvases)
    }

    fn is_free(&self, index: usize) -> bool {
        self.objects().as_ref().is_some_and(|objects| {
            objects
                .buffers
                .get(index)
                .is_some_and(|buffer| !buffer.held.load(Ordering::Acquire))
        })
    }

    fn ready(&self) -> bool {
        self.taken.load(Ordering::Acquire)
    }

    fn scales(&self) -> bool {
        self.objects()
            .as_ref()
            .is_some_and(|objects| objects.viewport.is_some())
    }

    fn show(&self, index: usize, place: Rect) -> bool {
        let mut guard = self.objects();
        let Some(objects) = guard.as_mut() else {
            return false;
        };
        let Some(buffer) = objects.buffers.get(index) else {
            return false;
        };
        let Some(place) = Place::of(place) else {
            return false;
        };
        let (buffer_width, buffer_height) = objects.buffer_size;
        buffer.held.store(true, Ordering::Release);
        let buffer = buffer.buffer.clone();

        // An RGB buffer: no coding of YCbCr on the surface.
        objects.unset_coding();
        let surface = &objects.surface;
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, buffer_width as i32, buffer_height as i32);

        match &objects.viewport {
            Some(viewport) => {
                // The whole buffer: the converter has cropped it.
                if objects.shown.source.take().is_some() {
                    viewport.set_source(-1.0, -1.0, -1.0, -1.0);
                }
                place_in_viewport(viewport, &mut objects.shown, place);
            }
            None => {
                // The buffer is the place's physical size at an integer scale.
                let scale = (buffer_width / place.rect.width).max(1) as i32;
                if objects.shown.buffer_scale != scale {
                    surface.set_buffer_scale(scale);
                    objects.shown.buffer_scale = scale;
                }
            }
        }

        let moved = self.commit(objects, place);
        drop(guard);
        self.flush();
        moved
    }

    fn park(&self) {
        let mut guard = self.objects();
        let Some(objects) = guard.as_mut() else {
            return;
        };
        // The compositor keeps the frame on screen without them: their
        // memory is not written again.
        for buffer in objects.buffers.drain(..) {
            buffer.buffer.destroy();
        }
        objects.buffer_size = (0, 0);
        // A dmabuf frame on screen stays held, with the decoder's memory, so
        // nothing writes into it until the plane is closed.
        if let Some(imports) = &self.dmabuf {
            imports.cache().forget(false);
        }
        objects
            .surface
            .frame(&self.qh, Drawn(objects.parent.clone()));
        objects.surface.commit();
        drop(guard);
        self.flush();
    }

    fn close(&self) {
        let Some(mut objects) = self.objects().take() else {
            return;
        };
        objects.unset_coding();
        if let Some(viewport) = &objects.viewport {
            viewport.destroy();
        }
        objects.subsurface.destroy();
        objects.surface.destroy();
        for buffer in &objects.buffers {
            buffer.buffer.destroy();
        }
        if let Some(imports) = &self.dmabuf {
            imports.cache().forget(true);
        }
        // No callback will come for a surface that is gone.
        self.taken.store(true, Ordering::Release);
        self.flush();
    }

    fn dmabuf_formats(&self) -> Arc<[DrmFormat]> {
        let Some(imports) = &self.dmabuf else {
            return Arc::from([]);
        };
        let refused = imports.refused();
        if refused.is_empty() {
            return Arc::clone(&imports.formats);
        }
        imports
            .formats
            .iter()
            .filter(|format| !refused.contains(format))
            .copied()
            .collect()
    }

    fn import(&self, frame: &DmabufFrame<'_>) -> Imported {
        let Some(imports) = &self.dmabuf else {
            return Imported::Refused;
        };
        // Version 4 of the protocol makes any other format an error that
        // ends the connection.
        if !imports.formats.contains(&frame.format) {
            return Imported::Refused;
        }
        if self.objects().is_none() {
            return Imported::NotYet;
        }
        let key = match ImportKey::of(frame) {
            Ok(key) => key,
            Err(e) => {
                tracing::warn!("video plane: cannot identify a dmabuf: {e}");
                return Imported::Refused;
            }
        };
        let mut cache = imports.cache();
        cache.dispatch();
        if let Some(id) = cache.find(&key) {
            return Imported::Ready(id);
        }

        let slot = Arc::new(Slot::default());
        let params = imports
            .global
            .create_params(&cache.queue.handle(), Arc::clone(&slot));
        let modifier = frame.format.modifier;
        for (index, plane) in (0u32..).zip(&frame.planes) {
            params.add(
                plane.fd,
                index,
                plane.offset,
                plane.stride,
                (modifier >> 32) as u32,
                modifier as u32,
            );
        }
        let (Ok(width), Ok(height)) = (i32::try_from(frame.size.0), i32::try_from(frame.size.1))
        else {
            params.destroy();
            return Imported::Refused;
        };
        params.create(
            width,
            height,
            frame.format.fourcc,
            zwp_linux_buffer_params_v1::Flags::empty(),
        );
        self.flush();

        match cache.wait(&slot) {
            Answer::Created(buffer) => {
                params.destroy();
                Imported::Ready(cache.insert(key, buffer))
            }
            Answer::Failed => {
                params.destroy();
                drop(cache);
                let mut refused = imports.refused();
                if !refused.contains(&frame.format) {
                    refused.push(frame.format);
                }
                Imported::Refused
            }
            // The answer, if it comes, destroys both (`Dispatch` below).
            Answer::Waiting | Answer::Abandoned => {
                tracing::debug!("video plane: the compositor did not answer an import in time");
                Imported::NotYet
            }
        }
    }

    /// A coding the compositor can be told, or the one it assumes when it is
    /// told nothing.
    fn shows_coding(&self, coding: ColorCoding) -> bool {
        match self.dmabuf.as_ref().map(|imports| &imports.colors) {
            Some(Some((_, codings))) => codings.contains(&coding) || coding == ColorCoding::ASSUMED,
            Some(None) => coding == ColorCoding::ASSUMED,
            None => false,
        }
    }

    fn show_dmabuf(
        &self,
        id: DmabufId,
        keep: Box<dyn Send>,
        coding: Option<ColorCoding>,
        source: Rect,
        place: Rect,
    ) -> bool {
        let Some(imports) = &self.dmabuf else {
            return false;
        };
        let mut guard = self.objects();
        let Some(objects) = guard.as_mut() else {
            return false;
        };
        let Some(viewport) = objects.viewport.clone() else {
            return false;
        };
        let Some(place) = Place::of(place) else {
            return false;
        };
        let mut cache = imports.cache();
        cache.dispatch();
        let Some(import) = cache.imports.iter().find(|import| import.id == id) else {
            return false;
        };
        let (width, height) = import.key.size;
        let buffer = import.buffer.clone();
        drop(cache);
        // A source outside the buffer is an error that ends the connection.
        let inside = source.width > 0
            && source.height > 0
            && source
                .x
                .checked_add(source.width)
                .is_some_and(|right| right <= width)
            && source
                .y
                .checked_add(source.height)
                .is_some_and(|bottom| bottom <= height);
        if !inside {
            tracing::warn!("video plane: crop {source:?} is outside the frame ({width}x{height})");
            return false;
        }
        let Some(hold) = buffer.data::<Hold>() else {
            return false;
        };
        // The coding has to be one the compositor was said to take: any
        // other is an error that ends the connection.
        match (coding, &imports.colors) {
            // Not one it can be told: the one it assumes, told nothing.
            (Some(coding), Some((_, codings))) if !codings.contains(&coding) => {
                if coding != ColorCoding::ASSUMED {
                    return false;
                }
                objects.unset_coding();
            }
            (Some(coding), Some((manager, _))) => {
                let current = objects.representation.as_ref().map(|(_, current)| *current);
                if current != Some(coding) {
                    let representation = match objects.representation.take() {
                        Some((representation, _)) => representation,
                        None => manager.get_surface(&objects.surface, &self.qh, ()),
                    };
                    let (coefficients, range) = dmabuf::coefficients_and_range(coding);
                    representation.set_coefficients_and_range(coefficients, range);
                    objects.representation = Some((representation, coding));
                }
            }
            (Some(coding), None) if coding != ColorCoding::ASSUMED => return false,
            (Some(_), None) => {}
            (None, _) => objects.unset_coding(),
        }
        hold.hold(keep);

        let surface = &objects.surface;
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width as i32, height as i32);
        if objects.shown.source != Some(source) {
            viewport.set_source(
                f64::from(source.x),
                f64::from(source.y),
                f64::from(source.width),
                f64::from(source.height),
            );
            objects.shown.source = Some(source);
        }
        place_in_viewport(&viewport, &mut objects.shown, place);

        let moved = self.commit(objects, place);
        drop(guard);
        self.flush();
        moved
    }

    fn forget_imports(&self) {
        if let Some(imports) = &self.dmabuf {
            imports.cache().forget(false);
        }
    }
}

/// Where a frame goes: a rectangle of the parent surface that is not empty
/// and fits the protocol's integers.
#[derive(Clone, Copy)]
struct Place {
    rect: Rect,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl Place {
    fn of(rect: Rect) -> Option<Self> {
        let (Ok(x), Ok(y), Ok(width), Ok(height)) = (
            i32::try_from(rect.x),
            i32::try_from(rect.y),
            i32::try_from(rect.width),
            i32::try_from(rect.height),
        ) else {
            return None;
        };
        (width > 0 && height > 0).then_some(Self {
            rect,
            x,
            y,
            width,
            height,
        })
    }
}

/// Scales the plane's buffer to the size of `place`.
fn place_in_viewport(viewport: &WpViewport, shown: &mut Shown, place: Place) {
    if shown.destination != Some((place.width, place.height)) {
        viewport.set_destination(place.width, place.height);
        shown.destination = Some((place.width, place.height));
    }
}

impl Plane {
    /// Moves the plane to `place` and commits the buffer attached, with a
    /// frame callback that says when the compositor has drawn it. Returns
    /// whether the plane moved.
    fn commit(&self, objects: &mut Objects, place: Place) -> bool {
        let moved = objects.shown.position != Some((place.x, place.y));
        if moved {
            objects.subsurface.set_position(place.x, place.y);
            objects.shown.position = Some((place.x, place.y));
        }
        self.taken.store(false, Ordering::Release);
        objects
            .surface
            .frame(&self.qh, Taken(Arc::clone(&self.taken)));
        objects.surface.commit();
        moved
    }
}

/// How long the streaming thread waits for the compositor to import a
/// dmabuf. It answers within a millisecond or two; more means that the event
/// loop, which reads its answers, is busy (or waits for this very thread, as
/// it does when it stops a pipeline).
const IMPORT_TIMEOUT: Duration = Duration::from_millis(250);

/// How often the streaming thread looks for the answer while it waits.
const IMPORT_POLL: Duration = Duration::from_micros(250);

/// Imported dmabufs a plane keeps at most. A decoder's pool has about a
/// dozen (the frames it refers to, and a few for the sink); one that changes
/// its dmabufs leaves the old ones behind, and they are dropped.
const MAX_IMPORTS: usize = 32;

/// What a plane needs to show dmabufs.
struct Imports {
    /// veil's binding of `zwp_linux_dmabuf_v1` (`dmabuf.rs`).
    global: ZwpLinuxDmabufV1,
    formats: Arc<[DrmFormat]>,
    /// How to tell the compositor how colours are coded, and which codings.
    colors: Option<(WpColorRepresentationManagerV1, Arc<[ColorCoding]>)>,
    /// Formats the compositor refused to import (`dmabuf.rs`).
    refused: Arc<Mutex<Vec<DrmFormat>>>,
    cache: Mutex<Cache>,
}

impl Imports {
    fn new(dmabuf: PlaneDmabuf, connection: &Connection) -> Self {
        Self {
            global: dmabuf.global,
            formats: dmabuf.formats,
            colors: dmabuf.colors,
            refused: dmabuf.refused,
            cache: Mutex::new(Cache {
                queue: connection.new_event_queue(),
                imports: Vec::new(),
                next_id: 0,
                clock: 0,
            }),
        }
    }

    fn cache(&self) -> MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn refused(&self) -> MutexGuard<'_, Vec<DrmFormat>> {
        self.refused.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The dmabufs a plane has imported, and the queue their events come on.
struct Cache {
    queue: EventQueue<ImportEvents>,
    imports: Vec<Import>,
    next_id: u64,
    /// Counts lookups, for which import was used last.
    clock: u64,
}

struct Import {
    id: DmabufId,
    key: ImportKey,
    /// Its user data is a [`Hold`].
    buffer: WlBuffer,
    used: u64,
}

impl Import {
    fn held(&self) -> bool {
        self.buffer.data::<Hold>().is_some_and(Hold::held)
    }

    /// Destroys the buffer, and lets the frame go.
    fn destroy(self) {
        if let Some(hold) = self.buffer.data::<Hold>() {
            hold.release();
        }
        self.buffer.destroy();
    }
}

impl Cache {
    /// Handles what came for the plane's buffers: answers and `release`s.
    fn dispatch(&mut self) {
        if let Err(e) = self.queue.dispatch_pending(&mut ImportEvents) {
            tracing::debug!("video plane: dispatching the dmabuf queue: {e}");
        }
    }

    fn find(&mut self, key: &ImportKey) -> Option<DmabufId> {
        self.clock += 1;
        let clock = self.clock;
        let import = self.imports.iter_mut().find(|import| &import.key == key)?;
        import.used = clock;
        Some(import.id)
    }

    /// Waits for the compositor's answer to an import, for at most
    /// [`IMPORT_TIMEOUT`]. The event loop reads it from the socket; it is
    /// handled here.
    fn wait(&mut self, slot: &Slot) -> Answer {
        let deadline = Instant::now() + IMPORT_TIMEOUT;
        loop {
            self.dispatch();
            if let Some(answer) = slot.take() {
                return answer;
            }
            if Instant::now() >= deadline {
                return slot.abandon();
            }
            std::thread::sleep(IMPORT_POLL);
        }
    }

    /// Keeps `buffer`, and drops the import used longest ago that the
    /// compositor does not hold, while there are too many.
    fn insert(&mut self, key: ImportKey, buffer: WlBuffer) -> DmabufId {
        self.clock += 1;
        let id = DmabufId(self.next_id);
        self.next_id += 1;
        self.imports.push(Import {
            id,
            key,
            buffer,
            used: self.clock,
        });
        while self.imports.len() > MAX_IMPORTS {
            let oldest = self
                .imports
                .iter()
                .enumerate()
                .filter(|(_, import)| !import.held())
                .min_by_key(|(_, import)| import.used)
                .map(|(index, _)| index);
            let Some(index) = oldest else {
                break;
            };
            self.imports.swap_remove(index).destroy();
        }
        id
    }

    /// Destroys the imports the compositor does not hold, or (`all`) every
    /// one: the compositor keeps a frame on screen after its buffer is
    /// destroyed.
    fn forget(&mut self, all: bool) {
        self.dispatch();
        let (gone, kept) = std::mem::take(&mut self.imports)
            .into_iter()
            .partition(|import| all || !import.held());
        self.imports = kept;
        for import in gone {
            import.destroy();
        }
    }
}

/// Which dmabuf a frame is in, as the kernel knows it: the decoder hands out
/// the same few over and over, each time with a new file descriptor.
#[derive(Debug, PartialEq, Eq)]
struct ImportKey {
    format: DrmFormat,
    size: (u32, u32),
    /// Device and inode of each plane's dmabuf, with its offset and stride.
    planes: Vec<(u64, u64, u32, u32)>,
}

impl ImportKey {
    fn of(frame: &DmabufFrame<'_>) -> rustix::io::Result<Self> {
        let planes = frame
            .planes
            .iter()
            .map(|plane| {
                let stat = rustix::fs::fstat(plane.fd.as_fd())?;
                Ok((stat.st_dev, stat.st_ino, plane.offset, plane.stride))
            })
            .collect::<rustix::io::Result<_>>()?;
        Ok(Self {
            format: frame.format,
            size: frame.size,
            planes,
        })
    }
}

/// The answer to one import.
enum Answer {
    Waiting,
    Created(WlBuffer),
    Failed,
    /// The plane stopped waiting; the answer destroys what it brings.
    Abandoned,
}

/// User data of a `zwp_linux_buffer_params_v1`: its answer.
pub(super) struct Slot(Mutex<Answer>);

impl Default for Slot {
    fn default() -> Self {
        Self(Mutex::new(Answer::Waiting))
    }
}

impl Slot {
    fn lock(&self) -> MutexGuard<'_, Answer> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn take(&self) -> Option<Answer> {
        let mut answer = self.lock();
        match *answer {
            Answer::Waiting | Answer::Abandoned => None,
            Answer::Created(_) | Answer::Failed => {
                Some(std::mem::replace(&mut *answer, Answer::Abandoned))
            }
        }
    }

    /// Gives up waiting: an answer that comes later is thrown away.
    fn abandon(&self) -> Answer {
        std::mem::replace(&mut *self.lock(), Answer::Abandoned)
    }

    /// Records the answer. `false` if nobody waits for it any more.
    fn answer(&self, answer: Answer) -> Result<(), Answer> {
        let mut slot = self.lock();
        match *slot {
            Answer::Waiting => {
                *slot = answer;
                Ok(())
            }
            _ => Err(answer),
        }
    }
}

/// User data of a buffer made from a dmabuf: the decoder's frame, while the
/// compositor holds the buffer.
#[derive(Default)]
pub(super) struct Hold(Mutex<Option<Box<dyn Send>>>);

impl Hold {
    fn lock(&self) -> MutexGuard<'_, Option<Box<dyn Send>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn held(&self) -> bool {
        self.lock().is_some()
    }

    fn hold(&self, frame: Box<dyn Send>) {
        let previous = self.lock().replace(frame);
        drop(previous);
    }

    fn release(&self) {
        let frame = self.lock().take();
        drop(frame);
    }
}

/// The state of a plane's own queue: everything is in the user data.
pub(super) struct ImportEvents;

impl Dispatch<ZwpLinuxBufferParamsV1, Arc<Slot>> for ImportEvents {
    fn event(
        _: &mut Self,
        params: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        slot: &Arc<Slot>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let answer = match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => Answer::Created(buffer),
            zwp_linux_buffer_params_v1::Event::Failed => Answer::Failed,
            _ => return,
        };
        if let Err(late) = slot.answer(answer) {
            if let Answer::Created(buffer) = late {
                buffer.destroy();
            }
            params.destroy();
        }
    }

    event_created_child!(ImportEvents, ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (WlBuffer, Hold::default())
    ]);
}

impl Dispatch<WlBuffer, Hold> for ImportEvents {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        hold: &Hold,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            hold.release();
        }
    }
}

impl Drop for Plane {
    fn drop(&mut self) {
        self.close();
    }
}

impl Dispatch<WlBuffer, Held> for AppState {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        held: &Held,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            held.0.store(false, Ordering::Release);
        }
    }
}

impl Dispatch<WlCallback, Drawn> for AppState {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        drawn: &Drawn,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.drawn_again(&drawn.0);
        }
    }
}

impl Dispatch<WlCallback, Taken> for AppState {
    fn event(
        _: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        taken: &Taken,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            taken.0.store(true, Ordering::Release);
        }
    }
}
