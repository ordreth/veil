use super::{Attached, RenderContext, WallpaperEngine};
use crate::error::VeilError;
use crate::scaling::{self, Color, Placement, Rect, ScaleMode, ScaleOptions};
use fast_image_resize::{self as fir, PixelType};
use image::error::{ParameterError, ParameterErrorKind};
use image::{
    ColorType, DynamicImage, ImageDecoder, ImageError, ImageReader, ImageResult, Limits, RgbaImage,
};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Largest accepted image width or height, in pixels.
pub const MAX_IMAGE_DIMENSION: u32 = 16_384;

/// Largest single allocation the decoder may make (decompression-bomb guard).
pub const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;

/// Resampling filter for scaled modes. Images are scaled once per render, and the
/// SIMD resizer keeps even a 4K Lanczos pass short.
const FILTER: fir::ResizeAlg = fir::ResizeAlg::Convolution(fir::FilterType::Lanczos3);

/// Bytes per `wl_shm` XRGB8888 pixel (little-endian memory order: B, G, R, X).
const BYTES_PER_PIXEL: usize = 4;

/// Static wallpaper: a decoded image (or just a solid colour) composited on the CPU
/// into a single opaque buffer of the size it is asked to render at. That is the
/// surface's size in physical pixels, never the image's size.
///
/// The engines that show one file on several outputs are built from one decode
/// (`engine::Source`): they share the decoded picture, which compositing only
/// reads, and engines whose canvases have the same size share the frame
/// `prepare` composites. Each lets go of its share after its first frame; the
/// memory is freed with the last one.
pub struct ImageEngine {
    /// File to (re-)decode from. `None` for a solid-colour background.
    source: Option<PathBuf>,
    /// Decoded picture, kept only until the first frame is drawn or prepared.
    decoded: Option<Arc<Picture>>,
    /// The first frame, composited ahead of `attach` by `prepare`. It is used
    /// by the next render if the canvas has this size, and dropped by it either
    /// way.
    prepared: Option<Arc<Frame>>,
    options: ScaleOptions,
}

/// One composited frame: XRGB8888 pixels for a canvas of `size`.
pub(super) struct Frame {
    size: (u32, u32),
    pixels: Vec<u8>,
}

/// A decoded image, already blended over its background colour: opaque
/// XRGB8888 pixels in the canvas byte order, rows from the top, no padding.
/// Compositing only reads it, so several engines can draw from one.
pub(super) struct Picture {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Picture {
    /// Decodes the image file at `path` (see [`decode`]) and blends it over `bg`.
    pub(super) fn open(path: &Path, bg: Color) -> Result<Self, VeilError> {
        Ok(Self::blended(decode(path)?, bg))
    }

    /// `image` blended over `bg`.
    fn blended(image: RgbaImage, bg: Color) -> Self {
        let (width, height) = image.dimensions();
        // Flatten onto the background before resampling, so transparent edges
        // do not bleed hidden colours, and convert to the canvas byte order in
        // the same pass. From here on every pixel is opaque and resampling is
        // channel-agnostic.
        let mut pixels = image.into_raw();
        flatten_to_xrgb(&mut pixels, bg);
        Self {
            width,
            height,
            pixels,
        }
    }
}

impl ImageEngine {
    /// Decodes `input_path` up front, so a broken file is reported before the
    /// current wallpaper is replaced.
    pub fn new(input_path: &str, options: ScaleOptions) -> Result<Self, VeilError> {
        let source = PathBuf::from(input_path);
        let decoded = Picture::open(&source, options.bg_color)?;
        Ok(Self::sharing(source, Arc::new(decoded), options))
    }

    /// An engine for the file at `source`, which `decoded` holds already
    /// decoded and blended over `options.bg_color`. Other engines may share it.
    pub(super) fn sharing(source: PathBuf, decoded: Arc<Picture>, options: ScaleOptions) -> Self {
        Self {
            source: Some(source),
            decoded: Some(decoded),
            prepared: None,
            options,
        }
    }

    /// A background of a single colour, used when the daemon starts without `--input`.
    pub fn solid(bg_color: Color) -> Self {
        Self {
            source: None,
            decoded: None,
            prepared: None,
            options: ScaleOptions {
                bg_color,
                ..ScaleOptions::default()
            },
        }
    }

    /// Whether this is a solid-colour background with no image. It is shown with
    /// `RenderTarget::fill`, which needs no full-size buffer when the surface can
    /// stretch a single pixel.
    fn is_solid(&self) -> bool {
        self.source.is_none()
    }

    /// Shows the wallpaper on the target: a solid colour is filled, an image is
    /// composited into the one buffer the target provides. The buffer and how it
    /// maps to the surface are the target's business.
    fn render(&mut self, ctx: &mut RenderContext<'_>) -> Result<(), VeilError> {
        if self.is_solid() {
            return ctx.target.fill(self.options.bg_color);
        }
        ctx.target
            .draw(&mut |canvas, size| self.compose(canvas, size))
    }

    /// Draws the wallpaper into an XRGB8888 `canvas` of `size`, then frees the
    /// decoded image. A later call (after a resize) decodes the file again. If
    /// `prepare` has composited a frame of this size, it is copied instead.
    ///
    /// The canvas must hold exactly `size` pixels. Everything below writes to
    /// it by position, so one that does not is refused here.
    pub fn compose(&mut self, canvas: &mut [u8], size: (u32, u32)) -> Result<(), VeilError> {
        let expected = (size.0 as usize)
            .checked_mul(size.1 as usize)
            .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL));
        if expected != Some(canvas.len()) {
            return Err(VeilError::BufferCreation(format!(
                "a canvas of {} bytes does not hold {}x{} pixels",
                canvas.len(),
                size.0,
                size.1
            )));
        }
        // Taken out in any case: for another size it is of no use any more.
        if let Some(frame) = self.prepared.take() {
            if frame.size == size && frame.pixels.len() == canvas.len() {
                canvas.copy_from_slice(&frame.pixels);
                return Ok(());
            }
        }
        // This engine's share of the picture goes with this frame, whether
        // other engines still hold one or not.
        let picture = match (self.decoded.take(), &self.source) {
            (Some(picture), _) => Some(picture),
            (None, Some(path)) => Some(Arc::new(Picture::open(path, self.options.bg_color)?)),
            (None, None) => None,
        };
        draw(picture.as_deref(), self.options, size, canvas)
    }

    /// Like `prepare`, but a frame for a canvas of this size that another
    /// engine of the same file has composited already (one of `frames`) is
    /// shared instead of composited again. A frame composited here is added to
    /// `frames`.
    pub(super) fn prepare_sharing(
        &mut self,
        canvas: (u32, u32),
        frames: &mut Vec<Arc<Frame>>,
    ) -> Result<(), VeilError> {
        if let Some(frame) = frames.iter().find(|frame| frame.size == canvas) {
            self.prepared = Some(Arc::clone(frame));
            self.decoded = None;
            return Ok(());
        }
        self.prepare(canvas)?;
        frames.extend(self.prepared.clone());
        Ok(())
    }

    /// Whether the decoded image is still held in memory (only before the first
    /// frame is drawn or prepared). For the tests.
    #[cfg(test)]
    pub fn holds_decoded_image(&self) -> bool {
        self.decoded.is_some()
    }

    /// Whether a frame composited by `prepare` is held (only until the next
    /// render). For the tests.
    #[cfg(test)]
    pub fn holds_prepared_frame(&self) -> bool {
        self.prepared.is_some()
    }
}

impl WallpaperEngine for ImageEngine {
    /// Composites the first frame for a canvas of `canvas` pixels, so that the
    /// `attach` on the event loop only has to copy it. The decoded image is
    /// freed here already. The frame is as large as the surface's buffer, and
    /// is held only until that `attach`.
    fn prepare(&mut self, canvas: (u32, u32)) -> Result<(), VeilError> {
        let len = (canvas.0 as usize)
            .checked_mul(canvas.1 as usize)
            .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL));
        let Some(len @ 1..) = len else {
            return Ok(());
        };
        if self.is_solid() {
            return Ok(());
        }
        let mut pixels = vec![0u8; len];
        self.compose(&mut pixels, canvas)?;
        self.prepared = Some(Arc::new(Frame {
            size: canvas,
            pixels,
        }));
        Ok(())
    }

    fn attach(&mut self, ctx: &mut RenderContext<'_>) -> Result<Attached, VeilError> {
        self.render(ctx)?;
        Ok(Attached::Shown)
    }

    /// Images are composited at the buffer size, so a new size or scale means
    /// drawing again (from the file: the decoded copy is gone after the first frame).
    fn resize(&mut self, ctx: &mut RenderContext<'_>) -> Result<(), VeilError> {
        self.render(ctx)
    }

    /// Nothing is attached, but neither the decoded image nor a prepared frame
    /// must be kept while there is no surface to draw on. The next render
    /// decodes the file again.
    fn detach(&mut self) {
        self.decoded = None;
        self.prepared = None;
    }

    fn is_dynamic(&self) -> bool {
        false
    }
}

/// What a decoder may do with a file: the guard against oversized and
/// bomb-like images. It holds wherever an image file is decoded, also where
/// only its frames are counted (`media`).
pub(super) fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits
}

/// Decodes an image file to RGBA8, rejecting oversized or bomb-like inputs. The
/// format is detected from the content, with the extension as a fallback.
///
/// Both allocations of a decode are held to [`MAX_DECODE_ALLOC`] before
/// anything is decoded: the decoder's own buffer, and its conversion to RGBA8,
/// which `image` does not count. That copy is up to eight times larger (an
/// 8-bit grey 16384x16384 PNG is 256 MiB decoded and 1 GiB as RGBA8). An image without pixels is refused like the formats that refuse
/// it themselves (farbfeld and PNM accept a 0x0 image).
fn decode(path: &Path) -> Result<RgbaImage, VeilError> {
    let io_error = |source| VeilError::PathResolution {
        path: path.display().to_string(),
        source,
    };

    let mut reader = ImageReader::open(path)
        .map_err(io_error)?
        .with_guessed_format()
        .map_err(io_error)?;
    reader.limits(limits());
    decode_within_limits(reader).map_err(|source| VeilError::Image {
        path: path.display().to_string(),
        source,
    })
}

/// The decoding part of [`decode`], for a reader that has its format and
/// its limits.
fn decode_within_limits(reader: ImageReader<BufReader<File>>) -> ImageResult<RgbaImage> {
    let mut decoder = reader.into_decoder()?;

    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(ImageError::Parameter(ParameterError::from_kind(
            ParameterErrorKind::DimensionMismatch,
        )));
    }
    // What `ImageReader::decode` does, which `into_decoder` leaves out.
    let mut budget = limits();
    budget.reserve(decoder.total_bytes())?;
    decoder.set_limits(budget)?;
    if decoder.color_type() != ColorType::Rgba8 {
        limits().reserve(u64::from(width) * u64::from(height) * BYTES_PER_PIXEL as u64)?;
    }

    Ok(DynamicImage::from_decoder(decoder)?.into_rgba8())
}

/// Composites `picture` onto an XRGB8888 `canvas` of `size`, according to
/// `options`. Pixels the picture does not cover get the background colour (the
/// picture is already blended over it), so the result is fully opaque. The
/// picture is only read.
fn draw(
    picture: Option<&Picture>,
    options: ScaleOptions,
    size: (u32, u32),
    canvas: &mut [u8],
) -> Result<(), VeilError> {
    canvas
        .as_chunks_mut::<BYTES_PER_PIXEL>()
        .0
        .fill(options.bg_color.to_xrgb8888());

    let Some(Picture {
        width,
        height,
        pixels,
    }) = picture
    else {
        return Ok(());
    };
    let (width, height) = (*width, *height);
    let Some(placement) = scaling::place(options.mode, (width, height), size) else {
        return Ok(());
    };

    let (src, dst) = (placement.src, placement.dst);
    if options.mode == ScaleMode::Tile {
        tile(pixels, (width, height), size, canvas);
    } else if (src.width, src.height) == (dst.width, dst.height) {
        copy_rect(pixels, width, src, canvas, size.0, dst);
    } else {
        resample(pixels, (width, height), placement, canvas, size)?;
    }
    Ok(())
}

/// Blends every RGBA pixel over `bg` and rewrites it in place as opaque XRGB8888.
fn flatten_to_xrgb(pixels: &mut [u8], bg: Color) {
    for pixel in pixels.as_chunks_mut::<BYTES_PER_PIXEL>().0 {
        let [r, g, b, a] = *pixel;
        *pixel = if a == 0xff {
            [b, g, r, 0xff]
        } else {
            let a = u16::from(a);
            let over =
                |c: u8, bg: u8| ((u16::from(c) * a + u16::from(bg) * (255 - a) + 127) / 255) as u8;
            [over(b, bg.b), over(g, bg.g), over(r, bg.r), 0xff]
        };
    }
}

/// Scales the `placement.src` region of the XRGB8888 `pixels` into `placement.dst`
/// on the canvas. A destination covering the whole canvas is written in place.
fn resample(
    pixels: &[u8],
    (width, height): (u32, u32),
    placement: Placement,
    canvas: &mut [u8],
    size: (u32, u32),
) -> Result<(), VeilError> {
    let scale_error = |e: &dyn std::fmt::Display| VeilError::Scaling(e.to_string());
    let (src, dst) = (placement.src, placement.dst);

    let source = fir::images::ImageRef::new(width, height, pixels, PixelType::U8x4)
        .map_err(|e| scale_error(&e))?;
    let options = fir::ResizeOptions::new()
        .resize_alg(FILTER)
        .use_alpha(false)
        .crop(
            f64::from(src.x),
            f64::from(src.y),
            f64::from(src.width),
            f64::from(src.height),
        );
    let mut resizer = fir::Resizer::new();

    if dst == Rect::new(0, 0, size.0, size.1) {
        let mut target = fir::images::Image::from_slice_u8(size.0, size.1, canvas, PixelType::U8x4)
            .map_err(|e| scale_error(&e))?;
        resizer
            .resize(&source, &mut target, &options)
            .map_err(|e| scale_error(&e))?;
    } else {
        let mut scaled = fir::images::Image::new(dst.width, dst.height, PixelType::U8x4);
        resizer
            .resize(&source, &mut scaled, &options)
            .map_err(|e| scale_error(&e))?;
        let whole = Rect::new(0, 0, dst.width, dst.height);
        copy_rect(scaled.buffer(), dst.width, whole, canvas, size.0, dst);
    }
    Ok(())
}

/// Copies the `src` region of a 4-byte-per-pixel image `src_width` pixels wide to
/// the same-sized `dst` region of a canvas `canvas_width` pixels wide.
fn copy_rect(
    pixels: &[u8],
    src_width: u32,
    src: Rect,
    canvas: &mut [u8],
    canvas_width: u32,
    dst: Rect,
) {
    let row_bytes = src.width as usize * BYTES_PER_PIXEL;
    for row in 0..src.height as usize {
        let from = ((src.y as usize + row) * src_width as usize + src.x as usize) * BYTES_PER_PIXEL;
        let to =
            ((dst.y as usize + row) * canvas_width as usize + dst.x as usize) * BYTES_PER_PIXEL;
        canvas[to..to + row_bytes].copy_from_slice(&pixels[from..from + row_bytes]);
    }
}

/// Repeats a 4-byte-per-pixel tile over the whole canvas from the top-left corner.
fn tile(pixels: &[u8], (width, height): (u32, u32), size: (u32, u32), canvas: &mut [u8]) {
    let tile_stride = width as usize * BYTES_PER_PIXEL;
    let stride = size.0 as usize * BYTES_PER_PIXEL;
    for (y, row) in canvas.chunks_exact_mut(stride).enumerate() {
        let ty = y % height as usize;
        let src = &pixels[ty * tile_stride..(ty + 1) * tile_stride];
        for chunk in row.chunks_mut(tile_stride) {
            chunk.copy_from_slice(&src[..chunk.len()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testing::{FakeTarget, Shown};
    use crate::scaling::SurfaceScale;
    use image::Rgba;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE_BG: Color = Color { r: 0, g: 0, b: 255 };

    /// XRGB8888 bytes of an opaque colour.
    fn xrgb([r, g, b, _]: [u8; 4]) -> [u8; 4] {
        [b, g, r, 0xff]
    }

    fn render(image: Option<RgbaImage>, mode: ScaleMode, size: (u32, u32)) -> Vec<[u8; 4]> {
        let mut canvas = vec![0u8; size.0 as usize * size.1 as usize * BYTES_PER_PIXEL];
        let options = ScaleOptions {
            mode,
            bg_color: BLUE_BG,
        };
        let picture = image.map(|image| Picture::blended(image, BLUE_BG));
        draw(picture.as_ref(), options, size, &mut canvas).expect("draw");
        canvas.as_chunks::<BYTES_PER_PIXEL>().0.to_vec()
    }

    fn solid_image(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba(color))
    }

    /// Writes a PNG to a unique temp path; the caller removes it.
    fn temp_png(name: &str, image: &RgbaImage) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("veil-test-{}-{name}.png", std::process::id()));
        image.save(&path).expect("write test PNG");
        path
    }

    #[test]
    fn no_image_fills_the_background() {
        let pixels = render(None, ScaleMode::Fill, (3, 2));
        assert!(pixels.iter().all(|&p| p == xrgb([0, 0, 255, 255])));
    }

    #[test]
    fn fit_letterboxes_with_the_background() {
        // A 2:1 image on a 4x4 surface occupies rows 1 and 2.
        let pixels = render(Some(solid_image(2, 1, RED)), ScaleMode::Fit, (4, 4));
        for (i, pixel) in pixels.iter().enumerate() {
            let row = i / 4;
            let expected = if row == 1 || row == 2 {
                RED
            } else {
                [0, 0, 255, 255]
            };
            assert_eq!(*pixel, xrgb(expected), "pixel {i}");
        }
    }

    #[test]
    fn fill_and_stretch_cover_the_whole_surface() {
        for mode in [ScaleMode::Fill, ScaleMode::Stretch] {
            let pixels = render(Some(solid_image(7, 3, RED)), mode, (5, 5));
            assert!(pixels.iter().all(|&p| p == xrgb(RED)), "{mode:?}");
        }
    }

    #[test]
    fn center_crops_large_images_to_their_middle() {
        // 3x1 image red|green|red on a 1x1 surface shows the green middle.
        let mut image = solid_image(3, 1, RED);
        image.put_pixel(1, 0, Rgba(GREEN));
        assert_eq!(
            render(Some(image), ScaleMode::Center, (1, 1)),
            [xrgb(GREEN)]
        );
    }

    #[test]
    fn center_keeps_small_images_at_native_size() {
        let pixels = render(Some(solid_image(1, 1, RED)), ScaleMode::Center, (3, 3));
        for (i, pixel) in pixels.iter().enumerate() {
            let expected = if i == 4 { RED } else { [0, 0, 255, 255] };
            assert_eq!(*pixel, xrgb(expected), "pixel {i}");
        }
    }

    #[test]
    fn tile_repeats_from_the_top_left() {
        let mut image = solid_image(2, 1, RED);
        image.put_pixel(1, 0, Rgba(GREEN));
        let pixels = render(Some(image), ScaleMode::Tile, (5, 2));
        let row = [RED, GREEN, RED, GREEN, RED].map(xrgb);
        assert_eq!(pixels[..5], row);
        assert_eq!(pixels[5..], row);
    }

    #[test]
    fn transparency_is_blended_over_the_background() {
        let pixels = render(
            Some(solid_image(1, 1, [255, 0, 0, 128])),
            ScaleMode::Stretch,
            (1, 1),
        );
        // red = 255 * 128/255, blue = 255 * 127/255, rounded to nearest.
        assert_eq!(pixels, [[127, 0, 128, 0xff]]);
    }

    #[test]
    fn decoded_image_is_freed_after_compose_and_decoded_again_on_demand() {
        let path = temp_png("free-after-render", &solid_image(4, 2, RED));
        let mut engine = ImageEngine::new(path.to_str().expect("utf-8 temp path"), {
            ScaleOptions::default()
        })
        .expect("decode test PNG");
        assert!(engine.holds_decoded_image());

        let mut canvas = vec![0u8; 8 * 4 * BYTES_PER_PIXEL];
        engine.compose(&mut canvas, (8, 4)).expect("first compose");
        assert!(!engine.holds_decoded_image());
        let first = canvas.clone();

        // A resize composes again by re-decoding the file, and does not keep it.
        canvas.fill(0);
        engine.compose(&mut canvas, (8, 4)).expect("second compose");
        assert!(!engine.holds_decoded_image());
        assert_eq!(canvas, first);

        std::fs::remove_file(path).expect("remove test PNG");
    }

    #[test]
    fn oversized_images_are_rejected() {
        let path = temp_png("too-wide", &solid_image(MAX_IMAGE_DIMENSION + 1, 1, RED));
        let result = decode(&path);
        std::fs::remove_file(&path).expect("remove test PNG");
        assert!(
            matches!(
                result,
                Err(VeilError::Image {
                    source: image::ImageError::Limits(_),
                    ..
                })
            ),
            "{result:?}"
        );
    }

    #[test]
    fn format_is_detected_from_content() {
        // A PNG with a misleading extension still decodes.
        let png = temp_png("renamed", &solid_image(2, 2, GREEN));
        let renamed = png.with_extension("jpg");
        std::fs::rename(&png, &renamed).expect("rename test PNG");
        let result = decode(&renamed);
        std::fs::remove_file(&renamed).expect("remove test image");
        assert_eq!(result.expect("decodes").dimensions(), (2, 2));
    }

    #[test]
    fn missing_file_reports_the_path() {
        let err = decode(Path::new("/nonexistent/veil.png")).expect_err("missing file");
        assert!(
            err.to_string()
                .starts_with("Failed to resolve path '/nonexistent/veil.png'"),
            "{err}"
        );
    }

    #[test]
    fn only_the_background_engine_is_solid() {
        let path = temp_png("not-solid", &solid_image(2, 2, RED));
        let engine = ImageEngine::new(
            path.to_str().expect("utf-8 temp path"),
            ScaleOptions::default(),
        );
        std::fs::remove_file(&path).expect("remove test PNG");
        let mut engine = engine.expect("decode test PNG");

        // An image is never solid, before or after its decoded copy is dropped.
        assert!(!engine.is_solid());
        let mut canvas = vec![0u8; 4 * BYTES_PER_PIXEL];
        engine.compose(&mut canvas, (2, 2)).expect("compose");
        assert!(!engine.holds_decoded_image());
        assert!(!engine.is_solid());

        assert!(ImageEngine::solid(BLUE_BG).is_solid());
    }

    #[test]
    fn solid_engine_holds_nothing() {
        let mut engine = ImageEngine::solid(BLUE_BG);
        assert!(!engine.holds_decoded_image());
        let mut canvas = vec![0u8; 2 * BYTES_PER_PIXEL];
        engine.compose(&mut canvas, (2, 1)).expect("compose");
        assert_eq!(canvas, [255, 0, 0, 255, 255, 0, 0, 255]);
    }

    /// A context for a 4x2 surface at scale 2; the canvas size is the target's.
    fn context(target: &mut FakeTarget) -> RenderContext<'_> {
        RenderContext::new((4, 2), SurfaceScale::from_integer(2), target)
    }

    fn red_engine(name: &str) -> (ImageEngine, PathBuf) {
        let path = temp_png(name, &solid_image(2, 2, RED));
        let engine = ImageEngine::new(
            path.to_str().expect("utf-8 temp path"),
            ScaleOptions::default(),
        )
        .expect("decode test PNG");
        (engine, path)
    }

    #[test]
    fn solid_engine_fills_the_target_instead_of_compositing_a_frame() {
        // The target can then show it as a single stretched pixel.
        let mut target = FakeTarget::new((8, 4));
        let mut engine = ImageEngine::solid(BLUE_BG);
        engine.attach(&mut context(&mut target)).expect("attach");
        engine.resize(&mut context(&mut target)).expect("resize");
        assert_eq!(target.shown, [Shown::Fill(BLUE_BG), Shown::Fill(BLUE_BG)]);
    }

    #[test]
    fn image_is_composited_at_the_size_of_the_targets_canvas() {
        let (mut engine, path) = red_engine("through-the-trait");

        // Neither the image's size (2x2) nor the logical size in the context
        // (4x2) sizes the frame: the target's canvas does.
        let mut target = FakeTarget::new((8, 4));
        engine.attach(&mut context(&mut target)).expect("attach");
        assert!(!engine.holds_decoded_image());
        // A resize draws again, from the file.
        let mut resized = FakeTarget::new((6, 6));
        engine.resize(&mut context(&mut resized)).expect("resize");
        std::fs::remove_file(&path).expect("remove test PNG");

        let red_frame = |size: (u32, u32)| Shown::Frame {
            size,
            pixels: xrgb(RED).repeat(size.0 as usize * size.1 as usize),
        };
        assert_eq!(target.shown, [red_frame((8, 4))]);
        assert_eq!(resized.shown, [red_frame((6, 6))]);
    }

    #[test]
    fn a_canvas_that_does_not_hold_its_size_is_refused_and_not_written() {
        let (mut engine, path) = red_engine("wrong-canvas");
        for mode in [
            ScaleMode::Fill,
            ScaleMode::Fit,
            ScaleMode::Stretch,
            ScaleMode::Center,
            ScaleMode::Tile,
        ] {
            engine.options.mode = mode;
            // Too short, too long, not a whole number of pixels, and a size
            // whose byte count does not fit in a `usize`.
            for (len, size) in [
                (4 * 4 * 3 * 4, (4, 4)),
                (4 * 4 * 5 * 4, (4, 4)),
                (4 * 4 * 4 + 1, (4, 4)),
                (0, (4, 4)),
                (16, (u32::MAX, u32::MAX)),
            ] {
                let mut canvas = vec![0x55u8; len];
                let composed = engine.compose(&mut canvas, size);
                assert!(
                    matches!(composed, Err(VeilError::BufferCreation(_))),
                    "{mode:?}, {len} bytes for {size:?}: {composed:?}"
                );
                assert!(canvas.iter().all(|&byte| byte == 0x55), "{mode:?}");
            }
        }
        // The engine is still usable: nothing was consumed by the refusals.
        let mut canvas = vec![0u8; 2 * 2 * 4];
        let composed = engine.compose(&mut canvas, (2, 2));
        std::fs::remove_file(&path).expect("remove test PNG");
        composed.expect("compose");
        assert_eq!(canvas, xrgb(RED).repeat(4));
    }

    #[test]
    fn detach_drops_the_decoded_image_and_a_later_attach_decodes_again() {
        let (mut engine, path) = red_engine("detach");
        assert!(engine.holds_decoded_image());
        engine.detach();
        assert!(!engine.holds_decoded_image());

        let mut target = FakeTarget::new((2, 2));
        let attached = engine.attach(&mut context(&mut target));
        std::fs::remove_file(&path).expect("remove test PNG");
        attached.expect("attach after detach");
        assert_eq!(target.shown.len(), 1);
    }

    #[test]
    fn nothing_is_shown_when_the_file_can_no_longer_be_decoded() {
        let (mut engine, path) = red_engine("deleted");
        engine.detach();
        std::fs::remove_file(&path).expect("remove test PNG");

        let mut target = FakeTarget::new((2, 2));
        let err = engine
            .attach(&mut context(&mut target))
            .expect_err("the file is gone");
        assert!(matches!(err, VeilError::PathResolution { .. }), "{err:?}");
        assert!(target.shown.is_empty());
    }

    /// A 4x2 image, red on the left half and green on the right, so a frame
    /// shows whether it was scaled for the right canvas.
    fn two_tone_engine(name: &str, mode: ScaleMode) -> (ImageEngine, PathBuf) {
        let image = RgbaImage::from_fn(4, 2, |x, _| Rgba(if x < 2 { RED } else { GREEN }));
        let path = temp_png(name, &image);
        let options = ScaleOptions {
            mode,
            bg_color: BLUE_BG,
        };
        let engine = ImageEngine::new(path.to_str().expect("utf-8 temp path"), options)
            .expect("decode test PNG");
        (engine, path)
    }

    #[test]
    fn a_prepared_frame_is_what_an_unprepared_attach_would_draw() {
        for mode in [
            ScaleMode::Fill,
            ScaleMode::Fit,
            ScaleMode::Stretch,
            ScaleMode::Center,
            ScaleMode::Tile,
        ] {
            let (mut plain, plain_path) = two_tone_engine("plain", mode);
            let mut expected = FakeTarget::new((6, 6));
            plain.attach(&mut context(&mut expected)).expect("attach");
            std::fs::remove_file(&plain_path).expect("remove test PNG");

            let (mut prepared, path) = two_tone_engine("prepared", mode);
            prepared.prepare((6, 6)).expect("prepare");
            // The work is done: the decoded image is gone, and so may be the file.
            assert!(!prepared.holds_decoded_image(), "{mode:?}");
            assert!(prepared.holds_prepared_frame(), "{mode:?}");
            std::fs::remove_file(&path).expect("remove test PNG");

            let mut target = FakeTarget::new((6, 6));
            prepared.attach(&mut context(&mut target)).expect("attach");
            assert_eq!(target.shown, expected.shown, "{mode:?}");
            // The frame was for that one attach.
            assert!(!prepared.holds_prepared_frame(), "{mode:?}");
        }
    }

    #[test]
    fn a_frame_prepared_for_another_size_is_dropped_and_the_file_decoded_again() {
        let (mut engine, path) = two_tone_engine("resized-meanwhile", ScaleMode::Stretch);
        engine.prepare((8, 4)).expect("prepare");

        // The surface changed between the load and the attach.
        let mut target = FakeTarget::new((4, 2));
        let attached = engine.attach(&mut context(&mut target));
        std::fs::remove_file(&path).expect("remove test PNG");
        attached.expect("attach at another size");

        assert!(!engine.holds_prepared_frame());
        let [Shown::Frame { size, pixels }] = target.shown.as_slice() else {
            panic!("expected one frame, got {:?}", target.shown);
        };
        assert_eq!(*size, (4, 2));
        // The image at its own size: red, red, green, green in each row.
        let row = [xrgb(RED), xrgb(RED), xrgb(GREEN), xrgb(GREEN)].concat();
        assert_eq!(*pixels, row.repeat(2));
    }

    #[test]
    fn detach_drops_a_prepared_frame() {
        let (mut engine, path) = two_tone_engine("detach-prepared", ScaleMode::Fill);
        engine.prepare((8, 4)).expect("prepare");
        engine.detach();
        std::fs::remove_file(&path).expect("remove test PNG");
        assert!(!engine.holds_prepared_frame());
        assert!(!engine.holds_decoded_image());
    }

    #[test]
    fn a_solid_colour_and_an_empty_canvas_prepare_nothing() {
        let mut solid = ImageEngine::solid(BLUE_BG);
        solid.prepare((8, 4)).expect("prepare");
        assert!(!solid.holds_prepared_frame());

        let (mut engine, path) = two_tone_engine("empty-canvas", ScaleMode::Fill);
        engine.prepare((0, 4)).expect("prepare for an empty canvas");
        std::fs::remove_file(&path).expect("remove test PNG");
        assert!(!engine.holds_prepared_frame());
        // Nothing was drawn, so the decoded image is still there for `attach`.
        assert!(engine.holds_decoded_image());
    }

    #[test]
    fn engines_of_one_file_share_the_decoded_image_and_frames_of_the_same_size() {
        let (first, path) = two_tone_engine("sharing", ScaleMode::Fill);
        std::fs::remove_file(&path).expect("remove test PNG");
        let decoded = first.decoded.clone().expect("decoded");
        let options = first.options;
        drop(first);
        let mut engines: Vec<ImageEngine> = (0..3)
            .map(|_| ImageEngine::sharing(path.clone(), Arc::clone(&decoded), options))
            .collect();
        // One picture in memory, whatever the number of engines.
        assert_eq!(Arc::strong_count(&decoded), 4);
        drop(decoded);

        let mut frames = Vec::new();
        for (engine, canvas) in engines.iter_mut().zip([(8, 4), (6, 6), (8, 4)]) {
            engine
                .prepare_sharing(canvas, &mut frames)
                .expect("prepare");
        }
        // Two sizes, two frames; the third engine took the first one's.
        assert_eq!(frames.len(), 2);
        let prepared = |i: usize| engines[i].prepared.clone().expect("a prepared frame");
        assert!(Arc::ptr_eq(&prepared(0), &prepared(2)));
        assert!(!Arc::ptr_eq(&prepared(0), &prepared(1)));
        // None of them holds the decoded picture any more: it is freed.
        assert!(engines.iter().all(|engine| !engine.holds_decoded_image()));

        // The shared frame is what each one draws (the file is gone).
        drop(frames);
        let mut target = FakeTarget::new((8, 4));
        for i in [0, 2] {
            engines[i]
                .attach(&mut RenderContext::new(
                    (8, 4),
                    SurfaceScale::ONE,
                    &mut target,
                ))
                .expect("attach");
        }
        assert_eq!(target.shown.len(), 2);
        assert_eq!(target.shown[0], target.shown[1]);
    }

    /// Writes `bytes` to a unique temp path ending in `name`; the caller removes it.
    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("veil-test-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).expect("write the test file");
        path
    }

    /// What the daemon does with a file it is given: detect it, decode it.
    fn open(name: &str, bytes: &[u8]) -> Result<(), VeilError> {
        let path = temp_file(name, bytes);
        let opened = crate::engine::Source::open(
            path.to_str().expect("UTF-8 temp path"),
            ScaleOptions::default(),
        );
        std::fs::remove_file(&path).expect("remove the test file");
        opened.map(|_| ())
    }

    /// The bytes of `image` encoded as `format`.
    fn encoded(image: impl Into<DynamicImage>, format: image::ImageFormat) -> Vec<u8> {
        let mut bytes = Vec::new();
        image
            .into()
            .write_to(&mut std::io::Cursor::new(&mut bytes), format)
            .expect("encode the test image");
        bytes
    }

    /// The CRC of a PNG chunk.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A PNG that says it is `width`x`height` pixels of `depth` bits in colour
    /// type `colour` (0 grey, 6 RGBA), with valid chunks and no pixel data: a
    /// decoder learns the size, but cannot decode a single row.
    fn png_header(width: u32, height: u32, depth: u8, colour: u8) -> Vec<u8> {
        let chunk = |kind: &[u8; 4], data: &[u8]| {
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            let length = u32::try_from(data.len()).expect("a small chunk");
            let mut chunk = length.to_be_bytes().to_vec();
            chunk.extend_from_slice(&body);
            chunk.extend_from_slice(&crc32(&body).to_be_bytes());
            chunk
        };
        let mut header = width.to_be_bytes().to_vec();
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[depth, colour, 0, 0, 0]);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(chunk(b"IHDR", &header));
        // An empty zlib stream.
        png.extend(chunk(
            b"IDAT",
            &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
        ));
        png.extend(chunk(b"IEND", &[]));
        png
    }

    #[test]
    fn a_damaged_file_is_refused_as_an_image_that_cannot_be_decoded() {
        let png = encoded(solid_image(64, 64, RED), image::ImageFormat::Png);
        let jpeg = encoded(
            DynamicImage::ImageRgba8(solid_image(64, 64, RED)).into_rgb8(),
            image::ImageFormat::Jpeg,
        );
        let gif = encoded(solid_image(64, 64, RED), image::ImageFormat::Gif);
        let mut bad_checksum = png.clone();
        // A byte of the pixel data; the chunk's CRC no longer matches.
        let in_the_data = bad_checksum.len() - 20;
        bad_checksum[in_the_data] ^= 0xFF;

        let cases: &[(&str, &[u8])] = &[
            ("half.png", &png[..png.len() / 2]),
            ("signature-only.png", &png[..8]),
            ("bad-checksum.png", &bad_checksum),
            ("half.jpg", &jpeg[..jpeg.len() / 2]),
            ("header-only.jpg", &jpeg[..40]),
            ("half.gif", &gif[..gif.len() / 2]),
        ];
        for (name, bytes) in cases {
            // Recognised as a picture by its signature, refused by its
            // decoder, with the file's name.
            let opened = open(name, bytes);
            let Err(error @ VeilError::Image { path, .. }) = &opened else {
                panic!("{name}: {opened:?}");
            };
            assert!(path.ends_with(name), "{path}");
            assert!(error.to_string().contains(path.as_str()), "{error}");
        }
    }

    #[test]
    fn an_image_damaged_after_it_was_shown_is_not_shown_again() {
        let (mut engine, path) = red_engine("damaged-later");
        let mut target = FakeTarget::new((2, 2));
        engine.attach(&mut context(&mut target)).expect("attach");
        engine.detach();

        let png = std::fs::read(&path).expect("read the test PNG");
        std::fs::write(&path, &png[..png.len() / 2]).expect("damage the test PNG");
        let attached = engine.attach(&mut context(&mut target));
        std::fs::remove_file(&path).expect("remove test PNG");
        assert!(
            matches!(attached, Err(VeilError::Image { .. })),
            "{attached:?}"
        );
        assert_eq!(target.shown.len(), 1, "only the frame of the first attach");
    }

    #[test]
    fn an_image_without_pixels_is_refused_in_every_format() {
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&[0; 8]); // width and height
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]);

        let cases: &[(&str, &[u8])] = &[
            ("empty.png", &png_header(0, 0, 8, 6)),
            ("no-columns.png", &png_header(0, 5, 8, 6)),
            ("no-rows.png", &png_header(5, 0, 8, 6)),
            (
                "empty.gif",
                b"GIF89a\0\0\0\0\0\0\0\x2c\0\0\0\0\0\0\0\0\0\x02\x02\x44\x01\0\x3b",
            ),
            ("empty.bmp", &bmp),
            ("empty.qoi", b"qoif\0\0\0\0\0\0\0\0\x04\0\0\0\0\0\0\0\0\x01"),
            (
                "empty.tga",
                &[0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 24, 0],
            ),
            // These two decoders accept an empty image; `decode` does not.
            ("empty.ff", b"farbfeld\0\0\0\0\0\0\0\0"),
            ("empty.ppm", b"P6\n0 0\n255\n"),
        ];
        for (name, bytes) in cases {
            let opened = open(name, bytes);
            assert!(
                matches!(opened, Err(VeilError::Image { .. })),
                "{name}: {opened:?}"
            );
        }
    }

    #[test]
    fn huge_images_are_refused_before_anything_is_decoded() {
        let limit = MAX_IMAGE_DIMENSION;
        let refused = |name: &str, png: Vec<u8>| {
            let opened = open(name, &png);
            assert!(
                matches!(
                    opened,
                    Err(VeilError::Image {
                        source: ImageError::Limits(_),
                        ..
                    })
                ),
                "{name}: {opened:?}"
            );
        };
        refused("header-too-wide.png", png_header(limit + 1, 1, 8, 6));
        refused("header-too-tall.png", png_header(1, limit + 1, 8, 6));
        // 1 GiB of RGBA pixels, more than one allocation may take.
        refused("too-many-rgba.png", png_header(limit, limit, 8, 6));
        // 256 MiB of grey pixels, but 1 GiB once they are RGBA.
        refused("too-many-grey.png", png_header(limit, limit, 8, 0));

        // On the limits: let through, then refused for the pixels they lack.
        let passed = |name: &str, png: Vec<u8>| {
            let opened = open(name, &png);
            assert!(
                matches!(&opened, Err(VeilError::Image { source, .. }) if !matches!(source, ImageError::Limits(_))),
                "{name}: {opened:?}"
            );
        };
        passed("as-wide-as-allowed.png", png_header(limit, 1, 8, 6));
        // 128 MiB of grey pixels, 512 MiB as RGBA.
        passed("as-many-as-allowed.png", png_header(limit, limit / 2, 8, 0));
    }

    /// Whether two pixels differ by at most `by` in every channel (JPEG is lossy).
    fn close(a: [u8; 4], b: [u8; 4], by: u8) -> bool {
        a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= by)
    }

    /// A 4x2 CMYK JPEG with an Adobe marker, red on the left half and green on
    /// the right, made with `magick -size 4x2 xc:red -fill '#00ff00' -draw
    /// 'rectangle 2,0 3,1' -colorspace CMYK -strip -quality 100` (ImageMagick;
    /// `image` cannot write CMYK).
    const CMYK_JPEG: &[u8] = &[
        0xff, 0xd8, 0xff, 0xee, 0x00, 0x0e, 0x41, 0x64, 0x6f, 0x62, 0x65, 0x00, 0x64, 0x00, 0x00,
        0x00, 0x00, 0x02, 0xff, 0xdb, 0x00, 0x43, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0xff, 0xdb, 0x00,
        0x43, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0xff, 0xc0, 0x00, 0x14, 0x08, 0x00, 0x02, 0x00, 0x04,
        0x04, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0x04, 0x11, 0x00, 0xff, 0xc4,
        0x00, 0x15, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x06, 0x0a, 0xff, 0xc4, 0x00, 0x1a, 0x10, 0x00, 0x01, 0x05, 0x01,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x07,
        0x08, 0x38, 0x86, 0xb6, 0xff, 0xc4, 0x00, 0x15, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09, 0x0a, 0xff, 0xc4, 0x00,
        0x1f, 0x11, 0x00, 0x00, 0x03, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x05, 0x07, 0x06, 0x08, 0x0a, 0x37, 0x39, 0x85, 0x88, 0xb6, 0xb8,
        0xff, 0xda, 0x00, 0x0e, 0x04, 0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x04, 0x00, 0x00, 0x3f,
        0x00, 0x37, 0x2f, 0xac, 0x4b, 0x87, 0x92, 0xe1, 0x93, 0x25, 0x27, 0x22, 0xd2, 0xcd, 0x9a,
        0xbc, 0xec, 0x06, 0xa1, 0x82, 0x86, 0x96, 0x89, 0xae, 0x57, 0x91, 0xbd, 0x68, 0xbc, 0x0b,
        0xf8, 0x3f, 0xff, 0xd9,
    ];

    #[test]
    fn a_cmyk_jpeg_shows_its_colours() {
        let path = temp_file("cmyk.jpg", CMYK_JPEG);
        let decoded = decode(&path);
        std::fs::remove_file(&path).expect("remove the test JPEG");

        let pixels = render(Some(decoded.expect("decodes")), ScaleMode::Center, (4, 2));
        for (i, pixel) in pixels.into_iter().enumerate() {
            let expected = if i % 4 < 2 { RED } else { GREEN };
            assert!(close(pixel, xrgb(expected), 2), "pixel {i}: {pixel:?}");
        }
    }

    #[test]
    fn sixteen_bit_images_keep_their_colours_and_transparency() {
        use image::{ImageBuffer, Luma, LumaA};

        // Opaque red, and green at half opacity over the blue background.
        let rgba: ImageBuffer<Rgba<u16>, _> = ImageBuffer::from_fn(2, 1, |x, _| {
            Rgba(if x == 0 {
                [u16::MAX, 0, 0, u16::MAX]
            } else {
                [0, u16::MAX, 0, 0x8080]
            })
        });
        let path = temp_file("rgba16.png", &encoded(rgba, image::ImageFormat::Png));
        let decoded = decode(&path);
        std::fs::remove_file(&path).expect("remove the test PNG");
        let pixels = render(Some(decoded.expect("decodes")), ScaleMode::Center, (2, 1));
        assert_eq!(pixels, [xrgb(RED), xrgb([0, 128, 127, 255])]);

        // Grey, without and with alpha.
        let grey: ImageBuffer<Luma<u16>, _> = ImageBuffer::from_pixel(1, 1, Luma([0x8080]));
        let clear: ImageBuffer<LumaA<u16>, _> = ImageBuffer::from_pixel(1, 1, LumaA([u16::MAX, 0]));
        for (name, bytes, expected) in [
            (
                "grey16.png",
                encoded(grey, image::ImageFormat::Png),
                [128, 128, 128, 255],
            ),
            (
                "clear16.png",
                encoded(clear, image::ImageFormat::Png),
                [0, 0, 255, 255],
            ),
        ] {
            let path = temp_file(name, &bytes);
            let decoded = decode(&path);
            std::fs::remove_file(&path).expect("remove the test PNG");
            let pixels = render(Some(decoded.expect("decodes")), ScaleMode::Center, (1, 1));
            assert_eq!(pixels, [xrgb(expected)], "{name}");
        }
    }
}
