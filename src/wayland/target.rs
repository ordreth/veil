//! How an engine draws on the layer surface: the Wayland side of
//! [`RenderTarget`]. Buffer allocation, buffer sizes and every `wl_surface`
//! request live here, so the engines only produce pixels.

use std::mem::ManuallyDrop;
use std::os::raw::c_void;

use smithay_client_toolkit::shm::{slot::SlotPool, Shm};
use wayland_client::{
    protocol::{wl_shm, wl_surface::WlSurface},
    Connection, Proxy,
};

use crate::engine::{Compose, RenderTarget, WindowHandle};
use crate::error::VeilError;
use crate::scaling::Color;

/// Bytes per `wl_shm` XRGB8888 pixel.
const BYTES_PER_PIXEL: usize = 4;

/// The connection to the compositor, kept open for as long as the process runs.
/// It is the only source of the `wl_display*` an engine is given
/// ([`WindowHandle::display`]).
///
/// `waylandsink` keeps that pointer, and reads the connection through it on a
/// thread of its own, until the sink object is finalized. That is not a moment
/// veil controls: GStreamer can hold the last reference to a pipeline on one of
/// its threads for a moment after the engine was dropped, and finalize the sink
/// there. No order of drops on veil's side can therefore promise that the
/// display is still there, so it is never freed: this holds a clone of the
/// connection that is not dropped, and libwayland disconnects only with the
/// last one. The kernel closes the socket when the process ends; whoever
/// leaves flushes the connection first.
pub(super) struct PinnedDisplay(ManuallyDrop<Connection>);

impl PinnedDisplay {
    pub fn new(connection: &Connection) -> Self {
        Self(ManuallyDrop::new(connection.clone()))
    }

    /// The `wl_display*` of the connection. It is valid until the process ends.
    pub fn ptr(&self) -> *mut c_void {
        self.0.backend().display_ptr().cast()
    }
}

/// The layer surface as a place to draw on, for the duration of one engine call.
///
/// Every frame is one opaque `wl_shm` buffer from the surface's `SlotPool`, which
/// is created on first use. The caller has already told the compositor how
/// buffers map to the surface (`AppState::sync_buffer_mapping`).
pub(super) struct SurfaceTarget<'a> {
    pub shm: &'a Shm,
    pub pool: &'a mut Option<SlotPool>,
    pub surface: &'a WlSurface,
    /// The connection `surface` belongs to.
    pub display: &'a PinnedDisplay,
    /// Buffer size in physical pixels: the surface's logical size times its scale.
    pub buffer: (u32, u32),
    /// Whether a `wp_viewport` stretches the buffer to the surface. A solid colour
    /// then needs a single pixel instead of a full-size buffer.
    pub viewport: bool,
}

impl SurfaceTarget<'_> {
    /// Lets `compose` fill one buffer of `size` pixels, then attaches it, damages
    /// the full rectangle and commits.
    fn present(
        &mut self,
        size: (u32, u32),
        compose: &mut dyn FnMut(&mut [u8]) -> Result<(), VeilError>,
    ) -> Result<(), VeilError> {
        let invalid = || VeilError::BufferCreation(format!("invalid size {}x{}", size.0, size.1));
        let width = i32::try_from(size.0).map_err(|_| invalid())?;
        let height = i32::try_from(size.1).map_err(|_| invalid())?;
        let stride = width
            .checked_mul(BYTES_PER_PIXEL as i32)
            .ok_or_else(invalid)?;

        let pool = match self.pool {
            Some(pool) => pool,
            None => {
                let byte_len = size.0 as usize * size.1 as usize * BYTES_PER_PIXEL;
                self.pool.insert(
                    SlotPool::new(byte_len, self.shm)
                        .map_err(|e| VeilError::BufferCreation(e.to_string()))?,
                )
            }
        };
        let (buffer, canvas) = pool
            .create_buffer(width, height, stride, wl_shm::Format::Xrgb8888)
            .map_err(|e| VeilError::BufferCreation(e.to_string()))?;

        compose(canvas)?;

        self.surface.attach(Some(buffer.wl_buffer()), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        self.surface.commit();

        Ok(())
    }
}

impl RenderTarget for SurfaceTarget<'_> {
    /// Every pixel is the same, so with a viewport one pixel is enough. This is
    /// the only place that makes that choice.
    fn fill(&mut self, color: Color) -> Result<(), VeilError> {
        let size = if self.viewport { (1, 1) } else { self.buffer };
        let pixel = color.to_xrgb8888();
        self.present(size, &mut |canvas| {
            canvas.as_chunks_mut::<BYTES_PER_PIXEL>().0.fill(pixel);
            Ok(())
        })
    }

    fn draw(&mut self, compose: &mut Compose<'_>) -> Result<(), VeilError> {
        let size = self.buffer;
        self.present(size, &mut |canvas| compose(canvas, size))
    }

    fn window(&self) -> WindowHandle {
        WindowHandle {
            display: self.display.ptr(),
            surface: self.surface.id().as_ptr().cast(),
        }
    }
}
