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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use smithay_client_toolkit::{
    error::GlobalError,
    globals::ProvidesBoundGlobal,
    reexports::protocols::wp::viewporter::client::{
        wp_viewport::WpViewport, wp_viewporter::WpViewporter,
    },
    shm::raw::RawPool,
    subcompositor::SubcompositorState,
};
use wayland_client::{
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_callback::{self, WlCallback},
        wl_shm::{self, WlShm},
        wl_subsurface::WlSubsurface,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, QueueHandle,
};

use super::state::AppState;
use crate::engine::{PlaneCanvas, VideoPlane};
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
    qh: QueueHandle<AppState>,
    connection: Connection,
}

impl Plane {
    /// A plane above `parent`, covering nothing until its first frame.
    pub(super) fn new(
        parent: &WlSurface,
        subcompositor: &SubcompositorState,
        viewporter: Option<&WpViewporter>,
        shm: &WlShm,
        qh: &QueueHandle<AppState>,
        connection: &Connection,
    ) -> Self {
        let (subsurface, surface) = subcompositor.create_subsurface(parent.clone(), qh);
        subsurface.set_desync();
        let viewport = viewporter.map(|viewporter| viewporter.get_viewport(&surface, qh, ()));
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
                    buffer_scale: 1,
                },
            })),
            taken: Arc::new(AtomicBool::new(true)),
            shm: shm.clone(),
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
        let (Ok(x), Ok(y), Ok(width), Ok(height)) = (
            i32::try_from(place.x),
            i32::try_from(place.y),
            i32::try_from(place.width),
            i32::try_from(place.height),
        ) else {
            return false;
        };
        if width == 0 || height == 0 {
            return false;
        }
        let (buffer_width, buffer_height) = objects.buffer_size;

        let surface = &objects.surface;
        buffer.held.store(true, Ordering::Release);
        surface.attach(Some(&buffer.buffer), 0, 0);
        surface.damage_buffer(0, 0, buffer_width as i32, buffer_height as i32);

        match &objects.viewport {
            Some(viewport) => {
                if objects.shown.destination != Some((width, height)) {
                    viewport.set_destination(width, height);
                    objects.shown.destination = Some((width, height));
                }
            }
            None => {
                // The buffer is the place's physical size at an integer scale.
                let scale = (buffer_width / place.width).max(1) as i32;
                if objects.shown.buffer_scale != scale {
                    surface.set_buffer_scale(scale);
                    objects.shown.buffer_scale = scale;
                }
            }
        }

        let moved = objects.shown.position != Some((x, y));
        if moved {
            objects.subsurface.set_position(x, y);
            objects.shown.position = Some((x, y));
        }

        self.taken.store(false, Ordering::Release);
        surface.frame(&self.qh, Taken(Arc::clone(&self.taken)));
        surface.commit();
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
        objects
            .surface
            .frame(&self.qh, Drawn(objects.parent.clone()));
        objects.surface.commit();
        drop(guard);
        self.flush();
    }

    fn close(&self) {
        let Some(objects) = self.objects().take() else {
            return;
        };
        if let Some(viewport) = &objects.viewport {
            viewport.destroy();
        }
        objects.subsurface.destroy();
        objects.surface.destroy();
        for buffer in &objects.buffers {
            buffer.buffer.destroy();
        }
        // No callback will come for a surface that is gone.
        self.taken.store(true, Ordering::Release);
        self.flush();
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
