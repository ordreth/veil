use smithay_client_toolkit::{
    compositor::CompositorState,
    reexports::protocols::wp::{
        fractional_scale::v1::client::{
            wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
            wp_fractional_scale_v1::WpFractionalScaleV1,
        },
        viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
    },
    shell::wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface},
};
use wayland_client::{globals::GlobalList, protocol::wl_output, QueueHandle};

use super::state::AppState;
use smithay_client_toolkit::shell::WaylandSurface;

/// Optional globals used to render sharply on scaled outputs.
pub struct ScaleGlobals {
    /// Lets a buffer of any size be shown at the surface's logical size.
    viewporter: Option<WpViewporter>,
    /// Reports non-integer scales.
    fractional_scale: Option<WpFractionalScaleManagerV1>,
}

impl ScaleGlobals {
    /// Binds whichever of the globals the compositor offers.
    pub fn bind(globals: &GlobalList, qh: &QueueHandle<AppState>) -> Self {
        let viewporter = globals.bind(qh, 1..=1, ()).ok();
        let fractional_scale = globals.bind(qh, 1..=1, ()).ok();
        if viewporter.is_none() {
            tracing::info!(
                "compositor has no wp_viewporter; falling back to integer buffer scales"
            );
        }
        if fractional_scale.is_none() {
            tracing::debug!("compositor has no wp_fractional_scale_manager_v1");
        }
        Self {
            viewporter,
            fractional_scale,
        }
    }
}

/// Per-surface scale objects of the layer surface. They are destroyed on drop,
/// which must happen before the surface itself is destroyed.
pub struct SurfaceScaling {
    /// Maps the buffer to the surface's logical size, when `wp_viewporter` exists.
    pub viewport: Option<WpViewport>,
    /// Kept only to receive `preferred_scale` events. Its user data is the
    /// surface, which tells the handler whose scale an event is about.
    fractional_scale: Option<WpFractionalScaleV1>,
}

impl Drop for SurfaceScaling {
    fn drop(&mut self) {
        if let Some(viewport) = &self.viewport {
            viewport.destroy();
        }
        if let Some(fractional_scale) = &self.fractional_scale {
            fractional_scale.destroy();
        }
    }
}

/// Creates and configures a Layer-Shell surface for a target output:
/// background layer, anchored to all 4 edges, exclusive_zone -1,
/// and no keyboard interactivity.
///
/// The scale objects are created before the initial commit, so a compositor that
/// already knows the surface's scale can send it ahead of the first configure.
pub fn create_layer_surface(
    compositor: &CompositorState,
    layer_shell: &LayerShell,
    scale_globals: &ScaleGlobals,
    qh: &QueueHandle<AppState>,
    output: &wl_output::WlOutput,
) -> (LayerSurface, SurfaceScaling) {
    let surface = compositor.create_surface(qh);
    let scaling = SurfaceScaling {
        viewport: scale_globals
            .viewporter
            .as_ref()
            .map(|viewporter| viewporter.get_viewport(&surface, qh, ())),
        fractional_scale: scale_globals
            .fractional_scale
            .as_ref()
            .map(|manager| manager.get_fractional_scale(&surface, qh, surface.clone())),
    };
    let layer = layer_shell.create_layer_surface(
        qh,
        surface,
        Layer::Background,
        Some("veil"),
        Some(output),
    );

    layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.set_margin(0, 0, 0, 0);
    layer.commit();

    (layer, scaling)
}
