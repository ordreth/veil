//! Scaling modes and placement geometry, shared by every engine.
//!
//! Everything here is pure: given the media size, the surface size and a mode, it
//! computes which part of the media is shown and where it lands on the surface.
//! [`SurfaceScale`] converts between the logical pixels of a surface and the
//! physical pixels of its buffer on HiDPI outputs.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// How media is fitted to the output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ScaleMode {
    /// Scale to cover the whole output, cropping the overflow (keeps aspect ratio).
    #[default]
    Fill,
    /// Scale to fit inside the output, with background bars (keeps aspect ratio).
    Fit,
    /// Scale to exactly the output size (ignores aspect ratio).
    Stretch,
    /// Native size, centred; cropped if larger than the output.
    Center,
    /// Native size, repeated from the top-left corner.
    Tile,
}

impl ScaleMode {
    /// The name, as `--mode` and the files take it.
    pub const fn name(self) -> &'static str {
        match self {
            ScaleMode::Fill => "fill",
            ScaleMode::Fit => "fit",
            ScaleMode::Stretch => "stretch",
            ScaleMode::Center => "center",
            ScaleMode::Tile => "tile",
        }
    }
}

/// An opaque sRGB colour, written as `#rrggbb` (the `#` is optional).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    /// The colour as one opaque `wl_shm` XRGB8888 pixel (memory order B, G, R, X).
    pub const fn to_xrgb8888(self) -> [u8; 4] {
        [self.b, self.g, self.r, 0xff]
    }
}

impl FromStr for Color {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("invalid colour '{s}': expected #rrggbb");
        let hex = s.strip_prefix('#').unwrap_or(s);
        // The digits are checked here and not left to `from_str_radix`, which
        // also accepts a sign.
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        let [_, r, g, b] = u32::from_str_radix(hex, 16)
            .map_err(|_| invalid())?
            .to_be_bytes();
        Ok(Self { r, g, b })
    }
}

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Color> for String {
    fn from(color: Color) -> Self {
        color.to_string()
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// Per-wallpaper presentation options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScaleOptions {
    pub mode: ScaleMode,
    /// Colour of the areas the media does not cover (letterbox bars, transparency).
    pub bg_color: Color,
}

/// An axis-aligned rectangle in pixels. Always non-empty when produced by [`place`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn full((width, height): (u32, u32)) -> Self {
        Self::new(0, 0, width, height)
    }
}

/// Scale of a surface: how many physical (buffer) pixels one logical
/// (surface-local) pixel covers. Stored in 120ths, the unit of
/// `wp_fractional_scale_v1`, so every scale a compositor can send is exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceScale(u32);

impl Default for SurfaceScale {
    fn default() -> Self {
        Self::ONE
    }
}

impl SurfaceScale {
    /// Denominator of the stored fraction.
    pub const DENOMINATOR: u32 = 120;

    /// One physical pixel per logical pixel.
    pub const ONE: Self = Self(Self::DENOMINATOR);

    /// A scale of `value / 120`. A `0` is not a scale and is treated as 1.
    pub fn from_120ths(value: u32) -> Self {
        if value == 0 {
            Self::ONE
        } else {
            Self(value)
        }
    }

    /// An integer scale factor. Anything below 1 is treated as 1.
    pub fn from_integer(factor: i32) -> Self {
        let factor = u32::try_from(factor).unwrap_or(1).max(1);
        Self(factor.saturating_mul(Self::DENOMINATOR))
    }

    /// The scale as a decimal number, for reports (`veil status`). Nothing is
    /// computed with it.
    pub fn as_f64(self) -> f64 {
        f64::from(self.0) / f64::from(Self::DENOMINATOR)
    }

    /// The smallest integer scale that is not below this one (`wl_surface`
    /// buffer scales are integers).
    pub fn ceil(self) -> i32 {
        i32::try_from(self.0.div_ceil(Self::DENOMINATOR)).unwrap_or(i32::MAX)
    }

    /// A logical length in physical pixels, rounded half away from zero as
    /// `wp_fractional_scale_v1` requires. A non-zero length never becomes zero.
    pub fn to_physical(self, logical: u32) -> u32 {
        if logical == 0 {
            return 0;
        }
        let den = u64::from(Self::DENOMINATOR);
        let physical = (u64::from(logical) * u64::from(self.0) + den / 2) / den;
        physical.clamp(1, u64::from(u32::MAX)) as u32
    }

    /// The buffer size, in physical pixels, of a surface of `logical` size.
    pub fn physical_size(self, logical: (u32, u32)) -> (u32, u32) {
        (self.to_physical(logical.0), self.to_physical(logical.1))
    }

    /// Converts `rect`, given in the physical pixels of a surface of `logical`
    /// size (which must not be empty), to logical pixels. Edges on the surface
    /// border stay on it, and the result is never empty or out of bounds.
    pub fn rect_to_logical(self, rect: Rect, logical: (u32, u32)) -> Rect {
        let physical = self.physical_size(logical);
        let (x, width) = self.span_to_logical(rect.x, rect.width, physical.0, logical.0);
        let (y, height) = self.span_to_logical(rect.y, rect.height, physical.1, logical.1);
        Rect::new(x, y, width, height)
    }

    /// One axis of [`Self::rect_to_logical`]: `(offset, length)` in logical pixels.
    fn span_to_logical(self, start: u32, len: u32, physical: u32, logical: u32) -> (u32, u32) {
        let edge = |position: u64| {
            if position >= u64::from(physical) {
                return logical;
            }
            let den = u64::from(Self::DENOMINATOR);
            let scaled = (position * den + u64::from(self.0) / 2) / u64::from(self.0);
            scaled.min(u64::from(logical)) as u32
        };
        let low = edge(u64::from(start));
        let high = edge(u64::from(start) + u64::from(len));
        if high > low {
            (low, high - low)
        } else if low < logical {
            (low, 1)
        } else {
            (low.saturating_sub(1), 1)
        }
    }
}

impl fmt::Display for SurfaceScale {
    /// Formats the scale as a decimal factor, e.g. `1`, `1.5` or `1.333`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let den = u64::from(Self::DENOMINATOR);
        let thousandths = (u64::from(self.0) * 1000 + den / 2) / den;
        let fraction = format!("{:03}", thousandths % 1000);
        let fraction = fraction.trim_end_matches('0');
        if fraction.is_empty() {
            write!(f, "{}", thousandths / 1000)
        } else {
            write!(f, "{}.{fraction}", thousandths / 1000)
        }
    }
}

/// Where media is drawn: the `src` region of the media is scaled to the `dst`
/// region of the surface. Anything outside `dst` shows the background colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub src: Rect,
    pub dst: Rect,
}

/// Computes the placement of media of size `media` on a surface of size `surface`.
///
/// Returns `None` if either size is empty. For [`ScaleMode::Tile`] the placement is
/// the first tile at the top-left corner; repeating it is up to the renderer.
pub fn place(mode: ScaleMode, media: (u32, u32), surface: (u32, u32)) -> Option<Placement> {
    let (mw, mh) = media;
    let (sw, sh) = surface;
    if mw == 0 || mh == 0 || sw == 0 || sh == 0 {
        return None;
    }

    // Compare aspect ratios without floats: media is wider than the surface when
    // mw / mh > sw / sh.
    let media_is_wider = u64::from(mw) * u64::from(sh) > u64::from(mh) * u64::from(sw);

    let placement = match mode {
        ScaleMode::Stretch => Placement {
            src: Rect::full(media),
            dst: Rect::full(surface),
        },
        ScaleMode::Fill => {
            // Crop the media to the surface's aspect ratio, centred.
            let (cw, ch) = if media_is_wider {
                (scale_dim(mh, sw, sh, mw), mh)
            } else {
                (mw, scale_dim(mw, sh, sw, mh))
            };
            Placement {
                src: Rect::new((mw - cw) / 2, (mh - ch) / 2, cw, ch),
                dst: Rect::full(surface),
            }
        }
        ScaleMode::Fit => {
            // Shrink or grow the media to the largest size inside the surface, centred.
            let (dw, dh) = if media_is_wider {
                (sw, scale_dim(mh, sw, mw, sh))
            } else {
                (scale_dim(mw, sh, mh, sw), sh)
            };
            Placement {
                src: Rect::full(media),
                dst: Rect::new((sw - dw) / 2, (sh - dh) / 2, dw, dh),
            }
        }
        ScaleMode::Center => {
            let (src_x, dst_x, w) = center_axis(mw, sw);
            let (src_y, dst_y, h) = center_axis(mh, sh);
            Placement {
                src: Rect::new(src_x, src_y, w, h),
                dst: Rect::new(dst_x, dst_y, w, h),
            }
        }
        ScaleMode::Tile => Placement {
            src: Rect::full(media),
            dst: Rect::full(media),
        },
    };
    Some(placement)
}

/// `value * num / den`, rounded to nearest and clamped to `1..=limit`, where `limit`
/// is the dimension the result must fit in.
fn scale_dim(value: u32, num: u32, den: u32, limit: u32) -> u32 {
    let scaled = (u64::from(value) * u64::from(num) + u64::from(den) / 2) / u64::from(den);
    scaled.clamp(1, u64::from(limit)) as u32
}

/// One axis of native-size centring: `(src offset, dst offset, length)`.
fn center_axis(media: u32, surface: u32) -> (u32, u32, u32) {
    if media > surface {
        ((media - surface) / 2, 0, surface)
    } else {
        (0, (surface - media) / 2, media)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODES: [ScaleMode; 5] = [
        ScaleMode::Fill,
        ScaleMode::Fit,
        ScaleMode::Stretch,
        ScaleMode::Center,
        ScaleMode::Tile,
    ];

    fn place_ok(mode: ScaleMode, media: (u32, u32), surface: (u32, u32)) -> Placement {
        place(mode, media, surface).expect("non-empty sizes")
    }

    #[test]
    fn fill_crops_wide_media_horizontally() {
        // 21:9 image on a 16:9 output: keep the full height, crop the sides.
        let p = place_ok(ScaleMode::Fill, (2560, 1080), (1920, 1080));
        assert_eq!(p.src, Rect::new(320, 0, 1920, 1080));
        assert_eq!(p.dst, Rect::new(0, 0, 1920, 1080));
    }

    #[test]
    fn fill_crops_tall_media_vertically() {
        // 4:3 image on a 16:9 output: keep the full width, crop top and bottom.
        let p = place_ok(ScaleMode::Fill, (640, 480), (1920, 1080));
        assert_eq!(p.src, Rect::new(0, 60, 640, 360));
        assert_eq!(p.dst, Rect::new(0, 0, 1920, 1080));
    }

    #[test]
    fn fill_with_matching_aspect_uses_everything() {
        let p = place_ok(ScaleMode::Fill, (3840, 2160), (1920, 1080));
        assert_eq!(p.src, Rect::new(0, 0, 3840, 2160));
        assert_eq!(p.dst, Rect::new(0, 0, 1920, 1080));
    }

    #[test]
    fn fit_letterboxes_wide_media() {
        let p = place_ok(ScaleMode::Fit, (2560, 1080), (1920, 1080));
        assert_eq!(p.src, Rect::new(0, 0, 2560, 1080));
        assert_eq!(p.dst, Rect::new(0, 135, 1920, 810));
    }

    #[test]
    fn fit_pillarboxes_tall_media() {
        let p = place_ok(ScaleMode::Fit, (640, 480), (1920, 1080));
        assert_eq!(p.src, Rect::new(0, 0, 640, 480));
        assert_eq!(p.dst, Rect::new(240, 0, 1440, 1080));
    }

    #[test]
    fn fit_with_matching_aspect_fills_the_surface() {
        let p = place_ok(ScaleMode::Fit, (3840, 2160), (2560, 1440));
        assert_eq!(p.dst, Rect::new(0, 0, 2560, 1440));
    }

    #[test]
    fn stretch_maps_everything_to_everything() {
        let p = place_ok(ScaleMode::Stretch, (640, 480), (2560, 1080));
        assert_eq!(p.src, Rect::new(0, 0, 640, 480));
        assert_eq!(p.dst, Rect::new(0, 0, 2560, 1080));
    }

    #[test]
    fn center_small_media_is_surrounded_by_background() {
        let p = place_ok(ScaleMode::Center, (640, 480), (1920, 1080));
        assert_eq!(p.src, Rect::new(0, 0, 640, 480));
        assert_eq!(p.dst, Rect::new(640, 300, 640, 480));
    }

    #[test]
    fn center_large_media_is_cropped() {
        let p = place_ok(ScaleMode::Center, (3840, 2160), (1920, 1080));
        assert_eq!(p.src, Rect::new(960, 540, 1920, 1080));
        assert_eq!(p.dst, Rect::new(0, 0, 1920, 1080));

        // Larger on one axis only.
        let p = place_ok(ScaleMode::Center, (2560, 480), (1920, 1080));
        assert_eq!(p.src, Rect::new(320, 0, 1920, 480));
        assert_eq!(p.dst, Rect::new(0, 300, 1920, 480));
    }

    #[test]
    fn tile_starts_at_the_top_left_at_native_size() {
        let p = place_ok(ScaleMode::Tile, (100, 50), (1920, 1080));
        assert_eq!(p.src, Rect::new(0, 0, 100, 50));
        assert_eq!(p.dst, Rect::new(0, 0, 100, 50));
    }

    #[test]
    fn empty_sizes_have_no_placement() {
        for mode in MODES {
            assert_eq!(place(mode, (0, 10), (10, 10)), None);
            assert_eq!(place(mode, (10, 0), (10, 10)), None);
            assert_eq!(place(mode, (10, 10), (0, 10)), None);
            assert_eq!(place(mode, (10, 10), (10, 0)), None);
        }
    }

    #[test]
    fn extreme_aspect_ratios_never_produce_empty_or_out_of_bounds_rects() {
        let sizes = [
            (1, 1),
            (1, 16384),
            (16384, 1),
            (3, 7),
            (640, 480),
            (1920, 1080),
            (2560, 1080),
            (3840, 2160),
            (16384, 16384),
        ];
        for mode in MODES {
            for media in sizes {
                for surface in sizes {
                    let p = place_ok(mode, media, surface);
                    let ctx = format!("{mode:?} {media:?} on {surface:?}: {p:?}");
                    assert!(p.src.width > 0 && p.src.height > 0, "{ctx}");
                    assert!(p.dst.width > 0 && p.dst.height > 0, "{ctx}");
                    assert!(p.src.x + p.src.width <= media.0, "{ctx}");
                    assert!(p.src.y + p.src.height <= media.1, "{ctx}");
                    if mode != ScaleMode::Tile {
                        assert!(p.dst.x + p.dst.width <= surface.0, "{ctx}");
                        assert!(p.dst.y + p.dst.height <= surface.1, "{ctx}");
                    }
                }
            }
        }
    }

    #[test]
    fn integer_scales_multiply_exactly() {
        assert_eq!(SurfaceScale::default(), SurfaceScale::ONE);
        assert_eq!(SurfaceScale::ONE.physical_size((1920, 1080)), (1920, 1080));
        assert_eq!(
            SurfaceScale::from_integer(2).physical_size((1920, 1080)),
            (3840, 2160)
        );
        assert_eq!(
            SurfaceScale::from_integer(3).physical_size((1280, 720)),
            (3840, 2160)
        );
        assert_eq!(
            SurfaceScale::from_integer(2),
            SurfaceScale::from_120ths(240)
        );
    }

    #[test]
    fn fractional_scales_give_the_output_mode_size() {
        // (scale in 120ths, logical size, physical size)
        let cases = [
            (150, (1536, 864), (1920, 1080)),  // 1.25
            (150, (3072, 1728), (3840, 2160)), // 1.25
            (180, (1280, 720), (1920, 1080)),  // 1.5
            (180, (2560, 1440), (3840, 2160)), // 1.5
            (210, (2194, 1234), (3840, 2160)), // 1.75, logical size rounded
            (160, (2880, 1620), (3840, 2160)), // 1.333
            (96, (2400, 1350), (1920, 1080)),  // 0.8
        ];
        for (scale, logical, physical) in cases {
            assert_eq!(
                SurfaceScale::from_120ths(scale).physical_size(logical),
                physical,
                "scale {scale}/120 of {logical:?}"
            );
        }
    }

    #[test]
    fn physical_lengths_round_half_away_from_zero() {
        let scale = SurfaceScale::from_120ths(150); // 1.25
        assert_eq!(scale.to_physical(1), 1); // 1.25
        assert_eq!(scale.to_physical(2), 3); // 2.5 rounds up
        assert_eq!(scale.to_physical(3), 4); // 3.75
        assert_eq!(scale.to_physical(5), 6); // 6.25

        let scale = SurfaceScale::from_120ths(180); // 1.5
        assert_eq!(scale.to_physical(1), 2); // 1.5 rounds up
        assert_eq!(scale.to_physical(3), 5); // 4.5 rounds up

        // A non-empty length stays non-empty, and an empty one stays empty.
        let small = SurfaceScale::from_120ths(30); // 0.25
        assert_eq!(small.to_physical(1), 1);
        assert_eq!(small.to_physical(0), 0);
        assert_eq!(
            SurfaceScale::from_integer(2).to_physical(u32::MAX),
            u32::MAX
        );
    }

    #[test]
    fn invalid_scales_are_treated_as_one() {
        assert_eq!(SurfaceScale::from_120ths(0), SurfaceScale::ONE);
        assert_eq!(SurfaceScale::from_integer(0), SurfaceScale::ONE);
        assert_eq!(SurfaceScale::from_integer(-3), SurfaceScale::ONE);
    }

    #[test]
    fn ceil_is_the_covering_integer_scale() {
        for (scale, expected) in [(60, 1), (120, 1), (121, 2), (150, 2), (240, 2), (300, 3)] {
            assert_eq!(SurfaceScale::from_120ths(scale).ceil(), expected, "{scale}");
        }
    }

    #[test]
    fn scale_is_displayed_as_a_decimal_factor() {
        for (scale, expected) in [
            (120, "1"),
            (240, "2"),
            (150, "1.25"),
            (180, "1.5"),
            (210, "1.75"),
            (160, "1.333"),
            (96, "0.8"),
        ] {
            assert_eq!(SurfaceScale::from_120ths(scale).to_string(), expected);
        }
    }

    #[test]
    fn rects_convert_back_to_logical_pixels() {
        // 2x: a 640x360 frame centred on a 3840x2160 buffer.
        assert_eq!(
            SurfaceScale::from_integer(2)
                .rect_to_logical(Rect::new(1600, 900, 640, 360), (1920, 1080)),
            Rect::new(800, 450, 320, 180)
        );
        // 1.5x: pillarbox of a 4:3 video on a 1920x1080 buffer (1280x720 logical).
        assert_eq!(
            SurfaceScale::from_120ths(180)
                .rect_to_logical(Rect::new(240, 0, 1440, 1080), (1280, 720)),
            Rect::new(160, 0, 960, 720)
        );
        // At 1x nothing changes.
        assert_eq!(
            SurfaceScale::ONE.rect_to_logical(Rect::new(7, 9, 11, 13), (100, 100)),
            Rect::new(7, 9, 11, 13)
        );
    }

    #[test]
    fn full_buffer_rects_map_to_the_full_surface() {
        // Also below 1x, where a physical pixel is wider than a logical one and
        // plain rounding would miss the border.
        let sizes = [(1, 1), (3, 7), (1366, 768), (2194, 1234), (2400, 1350)];
        for scale in [30, 96, 120, 150, 160, 180, 210, 240, 360] {
            let scale = SurfaceScale::from_120ths(scale);
            for logical in sizes {
                let (pw, ph) = scale.physical_size(logical);
                assert_eq!(
                    scale.rect_to_logical(Rect::new(0, 0, pw, ph), logical),
                    Rect::new(0, 0, logical.0, logical.1),
                    "scale {scale} of {logical:?}"
                );
            }
        }
    }

    #[test]
    fn logical_rects_are_never_empty_or_out_of_bounds() {
        let sizes = [(1, 1), (2, 3), (640, 480), (1366, 768), (2560, 1080)];
        for scale in [30, 96, 120, 150, 180, 210, 240] {
            let scale = SurfaceScale::from_120ths(scale);
            for logical in sizes {
                let (pw, ph) = scale.physical_size(logical);
                for (x, y, w, h) in [
                    (0, 0, 1, 1),
                    (pw - 1, ph - 1, 1, 1),
                    (pw / 2, ph / 2, 1, 1),
                    (0, ph / 3, pw, 1),
                    (pw / 3, 0, 1, ph),
                ] {
                    let r = scale.rect_to_logical(Rect::new(x, y, w, h), logical);
                    let ctx = format!("scale {scale}, {logical:?}, ({x}, {y}, {w}, {h}): {r:?}");
                    assert!(r.width > 0 && r.height > 0, "{ctx}");
                    assert!(r.x + r.width <= logical.0, "{ctx}");
                    assert!(r.y + r.height <= logical.1, "{ctx}");
                }
            }
        }
    }

    #[test]
    fn color_parses_hex_with_or_without_hash() {
        let expected = Color {
            r: 0x12,
            g: 0xab,
            b: 0xff,
        };
        assert_eq!("#12abff".parse::<Color>(), Ok(expected));
        assert_eq!("12ABFF".parse::<Color>(), Ok(expected));
        assert_eq!(expected.to_string(), "#12abff");
    }

    #[test]
    fn color_as_a_pixel_is_opaque_and_in_shm_byte_order() {
        let color = Color { r: 1, g: 2, b: 3 };
        assert_eq!(color.to_xrgb8888(), [3, 2, 1, 0xff]);
    }

    #[test]
    fn invalid_colors_are_rejected() {
        for s in [
            "",
            "#",
            "#fff",
            "#12345",
            "#1234567",
            "#gggggg",
            "red",
            "#12 456",
            "+12345",
            "#+12345",
            "#-12345",
            "##12345",
            "#12345é",
            "é12345",
            // Six bytes, but not six digits.
            "é1234",
            "#１２３４５６",
        ] {
            assert!(s.parse::<Color>().is_err(), "{s:?}");
        }
    }

    #[test]
    fn mode_and_color_serialize_as_strings() {
        assert_eq!(
            serde_json::to_string(&ScaleMode::Center).expect("serialize"),
            r#""center""#
        );
        assert_eq!(
            serde_json::to_string(&Color { r: 1, g: 2, b: 3 }).expect("serialize"),
            r##""#010203""##
        );
        assert!(serde_json::from_str::<Color>(r#""nope""#).is_err());
        assert!(serde_json::from_str::<ScaleMode>(r#""zoom""#).is_err());
    }
}
