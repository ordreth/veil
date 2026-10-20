//! Choice of the scale the layer surface is rendered at.
//!
//! A compositor can report the scale of a surface in two ways: as a fraction
//! (`wp_fractional_scale_v1`) and as an integer (`wl_surface.preferred_buffer_scale`,
//! or the scale of the outputs the surface is on). A fractional scale can only be
//! honoured with a `wp_viewport`, because `wl_surface.set_buffer_scale` takes integers.

use crate::scaling::SurfaceScale;

/// What is known about the scale of the layer surface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScaleInputs {
    /// `wp_fractional_scale_v1.preferred_scale`, in 120ths.
    pub fractional: Option<u32>,
    /// Integer scale the compositor reported for the surface.
    pub integer: Option<i32>,
    /// Integer `wl_output` scale of the bound output. It is only a first guess, used
    /// until the compositor reports a scale for the surface itself, so that the
    /// first frame on a HiDPI output does not have to be drawn twice.
    pub output_hint: Option<i32>,
}

impl ScaleInputs {
    /// The scale to render at. `has_viewport` tells whether the surface has a
    /// `wp_viewport`; without one the result is always an integer scale.
    pub fn effective(&self, has_viewport: bool) -> SurfaceScale {
        let fractional = self
            .fractional
            .filter(|&scale| scale > 0)
            .map(SurfaceScale::from_120ths);
        if let (true, Some(scale)) = (has_viewport, fractional) {
            return scale;
        }

        let integer = self
            .integer
            .filter(|&factor| factor > 0)
            .or(fractional.map(SurfaceScale::ceil))
            .or(self.output_hint)
            .unwrap_or(1);
        SurfaceScale::from_integer(integer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(
        fractional: Option<u32>,
        integer: Option<i32>,
        output_hint: Option<i32>,
    ) -> ScaleInputs {
        ScaleInputs {
            fractional,
            integer,
            output_hint,
        }
    }

    #[test]
    fn nothing_known_means_scale_one() {
        for has_viewport in [true, false] {
            assert_eq!(
                ScaleInputs::default().effective(has_viewport),
                SurfaceScale::ONE
            );
        }
    }

    #[test]
    fn fractional_scale_wins_with_a_viewport() {
        for scale in [150, 180, 210, 240, 96] {
            assert_eq!(
                inputs(Some(scale), Some(2), Some(2)).effective(true),
                SurfaceScale::from_120ths(scale)
            );
        }
    }

    #[test]
    fn without_a_viewport_the_scale_is_an_integer() {
        // The compositor's own integer scale is preferred.
        assert_eq!(
            inputs(Some(150), Some(2), Some(1)).effective(false),
            SurfaceScale::from_integer(2)
        );
        // Otherwise the fractional scale is rounded up, so nothing is upscaled.
        assert_eq!(
            inputs(Some(150), None, Some(1)).effective(false),
            SurfaceScale::from_integer(2)
        );
        assert_eq!(
            inputs(Some(300), None, None).effective(false),
            SurfaceScale::from_integer(3)
        );
    }

    #[test]
    fn integer_scale_is_used_when_no_fraction_is_known() {
        for has_viewport in [true, false] {
            assert_eq!(
                inputs(None, Some(2), Some(1)).effective(has_viewport),
                SurfaceScale::from_integer(2)
            );
        }
    }

    #[test]
    fn output_scale_is_only_a_first_guess() {
        for has_viewport in [true, false] {
            assert_eq!(
                inputs(None, None, Some(2)).effective(has_viewport),
                SurfaceScale::from_integer(2)
            );
            assert_eq!(
                inputs(None, Some(1), Some(2)).effective(has_viewport),
                SurfaceScale::ONE
            );
        }
        assert_eq!(
            inputs(Some(180), None, Some(2)).effective(true),
            SurfaceScale::from_120ths(180)
        );
    }

    #[test]
    fn bogus_values_are_ignored() {
        assert_eq!(
            inputs(Some(0), Some(0), Some(0)).effective(true),
            SurfaceScale::ONE
        );
        assert_eq!(
            inputs(Some(0), Some(-1), Some(2)).effective(false),
            SurfaceScale::from_integer(2)
        );
    }
}
