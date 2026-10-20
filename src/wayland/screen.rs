//! One output's wallpaper, and its surface while the output is there.
//!
//! The daemon keeps a [`Screen`] for every output it draws on. The screen
//! outlives the output's surface: when the output is unplugged its surface is
//! released and the wallpaper (the engine in `render`, detached) stays, to be
//! shown again when an output with that name returns. Everything that exists
//! only together with a surface is in [`OutputSurface`].

use smithay_client_toolkit::shell::{wlr_layer::LayerSurface, WaylandSurface};
use smithay_client_toolkit::shm::slot::SlotPool;
use wayland_client::protocol::wl_surface::WlSurface;

use super::answers::Ticket;
use super::binding::OutputKey;
use super::control::Record;
use super::render::RenderState;
use super::scale::ScaleInputs;
use super::surface::SurfaceScaling;
use super::unseen::StallWatch;
use crate::persist::Wallpaper;

pub struct Screen {
    pub key: OutputKey,
    /// The wallpaper engine, and the size (logical pixels) and scale it is
    /// shown at. `render.size()` is `Some` once `surface` has been configured.
    pub render: RenderState,
    /// The layer surface on the output. `Some` exactly while the output hosts
    /// it (`OutputBinding::output` is `Some` for `key`).
    pub surface: Option<OutputSurface>,
    /// The wallpaper whose video is attached here and waits for its first
    /// frame (`RenderState`'s engine that waits to take over), while there is
    /// one.
    pub starting: Option<Starting>,
    /// Which wallpaper the engine in `render` shows: a file, or only a
    /// colour. Reported by `veil status`.
    pub shows: Wallpaper,
    /// Paused by `veil pause`, until `veil resume`. The wallpaper is paused
    /// while this is set or the compositor hides it
    /// (`AppState::update_pause`). Not saved.
    pub held: bool,
}

/// A wallpaper that waits for its first frame to replace a screen's wallpaper.
pub struct Starting {
    /// The part of the `veil set` that is answered when it is shown, or
    /// cannot be. `None` for a wallpaper the daemon loaded by itself (for an
    /// output that appeared).
    pub ticket: Option<Ticket>,
    /// Which wallpaper it is, remembered once it is shown.
    pub record: Record,
}

/// The layer surface of one output, and what belongs to it. Its fields are
/// dropped in the order they are declared, which is the order the protocol
/// needs: the viewport and fractional-scale objects before the surface, and
/// the buffers after it.
pub struct OutputSurface {
    /// Viewport and fractional-scale objects of the layer surface.
    pub scaling: SurfaceScaling,
    pub layer: LayerSurface,
    /// Buffers of the surface; created on the first draw.
    pub pool: Option<SlotPool>,
    /// What the compositor has reported about the surface's scale.
    pub scale_inputs: ScaleInputs,
    /// Size from a configure that could not be resolved yet (a `0` dimension
    /// and an unknown output size). It is applied once the output reports its
    /// size.
    pub pending_configure: Option<(u32, u32)>,
    /// Whether a frame callback is outstanding on the surface (at most one).
    pub frame_callback_pending: bool,
    /// What the compositor did with the frames of the video that plays here,
    /// lately: whether it has stopped taking them.
    pub stall: StallWatch,
}

impl Screen {
    /// Size in physical pixels of the buffers of the surface (its logical size
    /// times its scale), or `None` while there is no configured surface.
    pub fn buffer_size(&self) -> Option<(u32, u32)> {
        self.surface.as_ref()?;
        Some(self.render.scale().physical_size(self.render.size()?))
    }

    /// Whether `surface` is this screen's layer surface.
    pub fn shows_on(&self, surface: &WlSurface) -> bool {
        self.surface
            .as_ref()
            .is_some_and(|own| own.layer.wl_surface() == surface)
    }
}

impl OutputSurface {
    pub fn has_viewport(&self) -> bool {
        self.scaling.viewport.is_some()
    }
}
