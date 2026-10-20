//! Surface size resolution for layer-surface configures.
//!
//! The surface is anchored to all four edges, so the compositor should always send
//! the output's size. A `0` in a configured dimension means "client decides", and
//! then the output's own size is used. If that is not known yet, the caller waits
//! instead of inventing one.

use smithay_client_toolkit::output::OutputInfo;
use wayland_client::protocol::wl_output::Transform;

/// The parts of an output's state that determine its size in surface-local
/// (logical) coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputSize {
    /// Logical size from `xdg_output`. It already accounts for scale and transform.
    pub logical: Option<(i32, i32)>,
    /// Physical size of the current mode, before transform and scale.
    pub current_mode: Option<(i32, i32)>,
    /// Integer `wl_output` scale factor.
    pub scale_factor: i32,
    pub transform: Transform,
}

impl From<&OutputInfo> for OutputSize {
    fn from(info: &OutputInfo) -> Self {
        Self {
            logical: info.logical_size,
            current_mode: info
                .modes
                .iter()
                .find(|mode| mode.current)
                .map(|mode| mode.dimensions),
            scale_factor: info.scale_factor,
            transform: info.transform,
        }
    }
}

impl OutputSize {
    /// The output's size in logical pixels: the `xdg_output` logical size if it is
    /// known, otherwise the current mode rotated by the transform and divided
    /// (rounding up, so the surface covers the output) by the scale factor.
    pub fn logical_size(&self) -> Option<(u32, u32)> {
        if let Some(size) = self.logical.and_then(positive) {
            return Some(size);
        }

        let (w, h) = positive(self.current_mode?)?;
        let (w, h) = if swaps_axes(self.transform) {
            (h, w)
        } else {
            (w, h)
        };
        let scale = self.scale_factor.max(1) as u32;
        Some((w.div_ceil(scale), h.div_ceil(scale)))
    }
}

/// Resolves the size to render at for a configure of `configured`. Each dimension
/// the compositor left at `0` is taken from `output`. Returns `None` when a
/// dimension is still unknown, meaning the caller must wait for more information.
pub fn resolve_surface_size(
    configured: (u32, u32),
    output: Option<&OutputSize>,
) -> Option<(u32, u32)> {
    let (width, height) = configured;
    if width != 0 && height != 0 {
        return Some(configured);
    }

    let (output_width, output_height) = output?.logical_size()?;
    Some((
        if width == 0 { output_width } else { width },
        if height == 0 { output_height } else { height },
    ))
}

fn positive((w, h): (i32, i32)) -> Option<(u32, u32)> {
    (w > 0 && h > 0).then_some((w as u32, h as u32))
}

fn swaps_axes(transform: Transform) -> bool {
    matches!(
        transform,
        Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(
        logical: Option<(i32, i32)>,
        current_mode: Option<(i32, i32)>,
        scale_factor: i32,
        transform: Transform,
    ) -> OutputSize {
        OutputSize {
            logical,
            current_mode,
            scale_factor,
            transform,
        }
    }

    #[test]
    fn configured_size_wins_when_complete() {
        let out = output(Some((1920, 1080)), Some((1920, 1080)), 1, Transform::Normal);
        assert_eq!(
            resolve_surface_size((2560, 1440), Some(&out)),
            Some((2560, 1440))
        );
        assert_eq!(resolve_surface_size((3840, 2160), None), Some((3840, 2160)));
    }

    #[test]
    fn zero_configure_uses_logical_size() {
        for (w, h) in [(2560, 1440), (3840, 2160), (1280, 720)] {
            let out = output(Some((w, h)), Some((7680, 4320)), 1, Transform::Normal);
            assert_eq!(
                resolve_surface_size((0, 0), Some(&out)),
                Some((w as u32, h as u32))
            );
        }
    }

    #[test]
    fn logical_size_already_includes_scale_and_transform() {
        // A 4K panel rotated 90 degrees at scale 2: xdg_output reports 1080x1920.
        let out = output(Some((1080, 1920)), Some((3840, 2160)), 2, Transform::_90);
        assert_eq!(resolve_surface_size((0, 0), Some(&out)), Some((1080, 1920)));
    }

    #[test]
    fn zero_configure_falls_back_to_current_mode() {
        let out = output(None, Some((2560, 1440)), 1, Transform::Normal);
        assert_eq!(resolve_surface_size((0, 0), Some(&out)), Some((2560, 1440)));

        // A non-positive logical size is ignored too.
        let out = output(Some((0, 0)), Some((3840, 2160)), 1, Transform::Normal);
        assert_eq!(resolve_surface_size((0, 0), Some(&out)), Some((3840, 2160)));
    }

    #[test]
    fn mode_fallback_divides_by_scale_rounding_up() {
        let out = output(None, Some((3840, 2160)), 2, Transform::Normal);
        assert_eq!(resolve_surface_size((0, 0), Some(&out)), Some((1920, 1080)));

        let out = output(None, Some((1366, 769)), 2, Transform::Normal);
        assert_eq!(resolve_surface_size((0, 0), Some(&out)), Some((683, 385)));

        // A bogus scale factor is treated as 1.
        let out = output(None, Some((1920, 1080)), 0, Transform::Normal);
        assert_eq!(resolve_surface_size((0, 0), Some(&out)), Some((1920, 1080)));
    }

    #[test]
    fn mode_fallback_swaps_axes_for_quarter_turns() {
        for transform in [
            Transform::_90,
            Transform::_270,
            Transform::Flipped90,
            Transform::Flipped270,
        ] {
            let out = output(None, Some((2560, 1440)), 1, transform);
            assert_eq!(
                resolve_surface_size((0, 0), Some(&out)),
                Some((1440, 2560)),
                "{transform:?}"
            );
        }
        for transform in [Transform::_180, Transform::Flipped, Transform::Flipped180] {
            let out = output(None, Some((2560, 1440)), 1, transform);
            assert_eq!(
                resolve_surface_size((0, 0), Some(&out)),
                Some((2560, 1440)),
                "{transform:?}"
            );
        }
    }

    #[test]
    fn only_zero_dimensions_are_filled_in() {
        let out = output(Some((2560, 1440)), None, 1, Transform::Normal);
        assert_eq!(
            resolve_surface_size((0, 1080), Some(&out)),
            Some((2560, 1080))
        );
        assert_eq!(
            resolve_surface_size((1920, 0), Some(&out)),
            Some((1920, 1440))
        );
    }

    #[test]
    fn unknown_output_size_means_wait() {
        assert_eq!(resolve_surface_size((0, 0), None), None);
        assert_eq!(resolve_surface_size((0, 1080), None), None);

        let out = output(None, None, 1, Transform::Normal);
        assert_eq!(resolve_surface_size((0, 0), Some(&out)), None);

        let out = output(None, Some((0, 0)), 1, Transform::Normal);
        assert_eq!(resolve_surface_size((0, 0), Some(&out)), None);
    }
}
