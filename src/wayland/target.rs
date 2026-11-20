//! How an engine draws on the layer surface: the Wayland side of
//! [`RenderTarget`]. Buffer allocation, buffer sizes and every `wl_surface`
//! request live here, so the engines only produce pixels.

use std::sync::Arc;

use smithay_client_toolkit::{
    reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter,
    shm::{slot::SlotPool, Shm},
    subcompositor::SubcompositorState,
};
use wayland_client::{
    protocol::{wl_shm, wl_surface::WlSurface},
    Connection, QueueHandle,
};

use super::dmabuf::Dmabuf;
use super::plane::Plane;
use super::state::AppState;
use crate::engine::{Compose, RenderTarget, VideoPlane};
use crate::error::VeilError;
use crate::scaling::Color;

/// Bytes per `wl_shm` XRGB8888 pixel.
const BYTES_PER_PIXEL: usize = 4;

/// The layer surface as a place to draw on, for the duration of one engine call.
///
/// Every frame is one opaque `wl_shm` buffer from the surface's `SlotPool`, which
/// is created on first use. The caller has already told the compositor how
/// buffers map to the surface (`AppState::sync_buffer_mapping`).
pub(super) struct SurfaceTarget<'a> {
    pub shm: &'a Shm,
    pub pool: &'a mut Option<SlotPool>,
    pub surface: &'a WlSurface,
    /// What a video plane is made with: `None` if the compositor has no
    /// `wl_subcompositor`.
    pub subcompositor: Option<&'a SubcompositorState>,
    pub viewporter: Option<&'a WpViewporter>,
    /// What a video plane shows dmabuf frames with.
    pub dmabuf: &'a Dmabuf,
    pub qh: &'a QueueHandle<AppState>,
    pub connection: &'a Connection,
    /// Buffer size in physical pixels: the surface's logical size times its scale.
    pub buffer: (u32, u32),
    /// Whether a `wp_viewport` stretches the buffer to the surface. A solid colour
    /// then needs a single pixel instead of a full-size buffer.
    pub viewport: bool,
}

/// The first `stride * height` bytes of `canvas`, which is all a buffer holds.
///
/// The pool rounds every slot up (to a multiple of 64 bytes), so a canvas can be
/// a few bytes longer than the buffer: 1900x1009 needs 7668400 bytes and gets
/// 7668416. Engines refuse a canvas that is not exactly the size they draw, so
/// the padding is cut off here. `None` if the canvas is too short.
fn exact_canvas(canvas: &mut [u8], stride: i32, height: i32) -> Option<&mut [u8]> {
    let len = usize::try_from(stride)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    canvas.get_mut(..len)
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

        compose(exact_canvas(canvas, stride, height).ok_or_else(invalid)?)?;

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

    fn video_plane(&mut self) -> Result<Arc<dyn VideoPlane>, VeilError> {
        let subcompositor = self.subcompositor.ok_or_else(|| {
            VeilError::BufferCreation("the compositor has no wl_subcompositor".into())
        })?;
        Ok(Arc::new(Plane::new(
            self.surface,
            subcompositor,
            self.viewporter,
            self.shm.wl_shm(),
            self.dmabuf.for_plane(),
            self.qh,
            self.connection,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::exact_canvas;

    #[test]
    fn padding_added_by_the_pool_is_cut_off() {
        // 1900x1009 at 4 bytes per pixel: the pool hands out 16 bytes too many.
        let mut canvas = vec![0u8; 7_668_416];
        let exact =
            exact_canvas(&mut canvas, 1900 * 4, 1009).expect("a canvas that is long enough");
        assert_eq!(exact.len(), 7_668_400);
    }

    #[test]
    fn an_exact_canvas_is_kept_whole() {
        let mut canvas = vec![0u8; 1920 * 1080 * 4];
        assert_eq!(
            exact_canvas(&mut canvas, 1920 * 4, 1080)
                .expect("a canvas that is long enough")
                .len(),
            1920 * 1080 * 4
        );
    }

    #[test]
    fn a_canvas_that_is_too_short_is_refused() {
        let mut canvas = vec![0u8; 100];
        assert!(exact_canvas(&mut canvas, 40, 3).is_none());
    }
}
