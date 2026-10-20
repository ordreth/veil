use crate::error::VeilError;
use crate::scaling::{Color, ScaleOptions, SurfaceScale};
use std::os::raw::c_void;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod image;
pub mod loader;
pub mod media;
// The only module that may use `unsafe` (see `[lints]` in `Cargo.toml`): it
// passes the raw pointers of `WindowHandle` on to GStreamer.
#[allow(unsafe_code)]
pub mod video;

/// An engine as the daemon holds it. It is `Send`, because engines are built
/// on loader threads and an engine that has been replaced is dropped on
/// another thread.
pub type Engine = Box<dyn WallpaperEngine + Send>;

/// The version of the GStreamer library the process runs with (which may not
/// be the one veil was built against), for the log. It only reads constants of
/// the library: GStreamer is not initialised and starts no thread.
pub fn gstreamer_version() -> String {
    match gstreamer::version() {
        (major, minor, micro, 0) => format!("{major}.{minor}.{micro}"),
        (major, minor, micro, nano) => format!("{major}.{minor}.{micro}.{nano}"),
    }
}

/// A media file that has been checked, and that the engines showing it are
/// built from: one engine per output, because an engine (a video pipeline
/// above all) serves exactly one surface.
///
/// Whether it is a video or an image is decided by the content of the file
/// and not by its name ([`media::detect`]), and a path that cannot be a
/// wallpaper (missing, not a regular file, empty, unreadable, neither an image
/// nor a video) is refused there, before a decoder sees it. An image is decoded
/// once, here, and shared by every engine built from it.
///
/// Opening reads and decodes the file, and building a video engine starts
/// GStreamer, which can take long. The daemon does both through [`loader`],
/// off the event loop; the engines are `Send` for that.
pub struct Source {
    input: String,
    options: ScaleOptions,
    kind: SourceKind,
}

enum SourceKind {
    Video,
    /// The decoded picture, blended over the background colour. Every engine
    /// built from the source holds a share until it has drawn (or prepared)
    /// its first frame.
    Image(Arc<image::Picture>),
}

impl Source {
    /// Checks the file at `input` and, for an image, decodes it.
    pub fn open(input: &str, options: ScaleOptions) -> Result<Self, VeilError> {
        let kind = match media::detect(Path::new(input))? {
            media::MediaKind::Video => SourceKind::Video,
            media::MediaKind::Image => SourceKind::Image(Arc::new(image::Picture::open(
                Path::new(input),
                options.bg_color,
            )?)),
        };
        Ok(Self {
            input: input.to_string(),
            options,
            kind,
        })
    }

    /// Whether the file is played as a video.
    pub fn is_video(&self) -> bool {
        matches!(self.kind, SourceKind::Video)
    }

    /// One engine, for one output: `VideoEngine` for a video (and for a GIF
    /// that moves), `ImageEngine` for an image.
    pub fn engine(&self) -> Result<Engine, VeilError> {
        match &self.kind {
            SourceKind::Video => Ok(Box::new(video::VideoEngine::new(
                &self.input,
                self.options,
            )?)),
            SourceKind::Image(decoded) => Ok(Box::new(image::ImageEngine::sharing(
                PathBuf::from(&self.input),
                Arc::clone(decoded),
                self.options,
            ))),
        }
    }

    /// One engine per entry of `canvases`, in that order, each prepared for
    /// its canvas (`None`: the output has no surface right now, and nothing is
    /// prepared). An image is composited once per distinct canvas size, and
    /// engines with the same size share that frame.
    pub fn engines(self, canvases: &[Option<(u32, u32)>]) -> Result<Vec<Engine>, VeilError> {
        let decoded = match self.kind {
            SourceKind::Image(decoded) => decoded,
            SourceKind::Video => {
                let mut engines = Vec::with_capacity(canvases.len());
                for canvas in canvases {
                    let mut engine = video::VideoEngine::new(&self.input, self.options)?;
                    if let Some(canvas) = canvas {
                        engine.prepare(*canvas)?;
                    }
                    engines.push(Box::new(engine) as Engine);
                }
                return Ok(engines);
            }
        };

        let path = PathBuf::from(&self.input);
        let mut images: Vec<image::ImageEngine> = canvases
            .iter()
            .map(|_| image::ImageEngine::sharing(path.clone(), Arc::clone(&decoded), self.options))
            .collect();
        // The engines hold the only shares now: the picture is freed when the
        // last of them has composited its first frame.
        drop(decoded);
        let mut frames = Vec::new();
        for (engine, canvas) in images.iter_mut().zip(canvases) {
            if let Some(canvas) = canvas {
                engine.prepare_sharing(*canvas, &mut frames)?;
            }
        }
        Ok(images
            .into_iter()
            .map(|engine| Box::new(engine) as Engine)
            .collect())
    }
}

/// Raw Wayland handles of the surface an engine is shown on, for an engine that
/// brings its own Wayland client code (GStreamer's `waylandsink`).
///
/// How long each pointer is valid is a promise of whoever implements
/// [`RenderTarget::window`], and what the `unsafe` code of an engine relies
/// on. The two differ. Engines that do not need the pointers never read them,
/// so a target without a real surface (a test double) may return nulls; an
/// engine that needs them must check for that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowHandle {
    /// `wl_display*` of veil's own connection to the compositor.
    ///
    /// Valid **until the process ends**: the connection is never closed once a
    /// target can hand this out, not even when the compositor is gone (the
    /// object stays, it only stops working). Nothing shorter can be promised
    /// to a video sink. It keeps this pointer, and a thread that reads the
    /// connection, until the sink object is finalized, and GStreamer may do
    /// that on one of its own threads a moment after the engine was dropped.
    pub display: *mut c_void,
    /// `wl_surface*` of the surface the wallpaper is shown on.
    ///
    /// Valid **until the engine's `detach` returns**: the Wayland side calls
    /// `detach`, and waits for every engine it is still shutting down, before
    /// it destroys the surface, on every way out. An engine must not use it
    /// later, and must not keep it in anything that outlives its `detach`.
    pub surface: *mut c_void,
}

/// Writes one frame: called with the canvas of a [`RenderTarget`] and the canvas
/// size in pixels.
pub type Compose<'a> = dyn FnMut(&mut [u8], (u32, u32)) -> Result<(), VeilError> + 'a;

/// The surface an engine draws on. The Wayland side implements it, so buffers,
/// their sizes and every `wl_surface` request stay there, and an engine only
/// produces pixels.
pub trait RenderTarget {
    /// Shows a single opaque colour on the whole surface.
    fn fill(&mut self, color: Color) -> Result<(), VeilError>;

    /// Shows one opaque frame. `compose` is called once with a canvas that covers
    /// the surface and with the canvas size in pixels (the surface's size in
    /// physical pixels). The canvas is `wl_shm` XRGB8888: four bytes per pixel in
    /// the memory order B, G, R, X, rows from the top with no padding. Its content
    /// is undefined before the call, so `compose` must write every pixel. If
    /// `compose` fails, nothing is shown and its error is returned.
    fn draw(&mut self, compose: &mut Compose<'_>) -> Result<(), VeilError>;

    /// Raw handles of the surface, for an engine that renders into it by itself.
    fn window(&self) -> WindowHandle;
}

/// What an engine reports about itself, from a thread of its own.
#[derive(Debug)]
pub enum EngineReport {
    /// The answer of an engine whose `attach` said [`Attached::Starting`]: its
    /// first frame is on the surface (`Ok`), or it cannot show anything (`Err`).
    Started(Result<(), VeilError>),
    /// An engine that had started cannot show its wallpaper any more, and has
    /// given up trying (a video whose pipeline failed and could not be
    /// restarted). It shows nothing now, and is detached.
    Lost(VeilError),
    /// The engine has moved its picture on the surface by itself (a video
    /// whose frame size became known on its streaming thread, in a mode that
    /// places it by that size). It shows the picture in a subsurface, and where
    /// a subsurface is placed is state of its parent: the surface must be
    /// committed for the move to take effect.
    Moved,
}

/// Receives what an engine reports through an [`EngineSignal`]: the serial of
/// the signal, and the report. It is called on a thread of the engine.
pub type ReportSink = Arc<dyn Fn(u64, EngineReport) + Send + Sync>;

/// How an engine tells the Wayland side what happens to it when that is not
/// the result of a call: that a start which takes time (see
/// [`Attached::Starting`]) is over, that it has stopped working later, or that
/// it has moved its picture by itself. It may be used from any thread.
#[derive(Clone)]
pub struct EngineSignal {
    serial: u64,
    sink: ReportSink,
}

impl std::fmt::Debug for EngineSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineSignal")
            .field("serial", &self.serial)
            .finish_non_exhaustive()
    }
}

impl EngineSignal {
    /// A signal that delivers to `sink`, together with `serial`.
    pub fn new(serial: u64, sink: ReportSink) -> Self {
        Self { serial, sink }
    }

    /// A signal nobody listens to.
    pub fn unheard() -> Self {
        Self::new(0, Arc::new(|_, _| {}))
    }

    /// Reports that the engine has its first frame on the surface (`Ok`), or
    /// that it cannot show anything (`Err`). Only the first of these reports
    /// counts.
    pub fn started(&self, started: Result<(), VeilError>) {
        (self.sink)(self.serial, EngineReport::Started(started));
    }

    /// Reports that the engine, after it had started, has stopped showing its
    /// wallpaper for good.
    pub fn lost(&self, error: VeilError) {
        (self.sink)(self.serial, EngineReport::Lost(error));
    }

    /// Reports that the engine has moved its picture on the surface, and that
    /// the surface needs a commit for that to take effect.
    pub fn moved(&self) {
        (self.sink)(self.serial, EngineReport::Moved);
    }
}

/// What an engine is told about the surface when it has to show its wallpaper.
pub struct RenderContext<'a> {
    /// Surface size in logical pixels.
    pub size: (u32, u32),
    /// Scale of the surface: its buffers are `scale` times `size`.
    pub scale: SurfaceScale,
    /// Where the pixels go.
    pub target: &'a mut dyn RenderTarget,
    /// Whether the surface shows another wallpaper that must stay as it is for
    /// now. Only an engine whose `attach` answers [`Attached::Starting`] has to
    /// care: it must not draw on the target before `take_over`, neither in
    /// `attach` nor in a `resize` that comes meanwhile. Without it, such an
    /// engine may draw at once (a background colour under a video).
    pub keep_previous: bool,
    /// Where the engine reports what happens to it later: the end of a start
    /// that takes time ([`Attached::Starting`]), or that it has stopped
    /// working. An engine that needs it keeps a clone.
    pub signal: EngineSignal,
}

impl<'a> RenderContext<'a> {
    /// A context for a surface that shows nothing yet, with nobody waiting for a
    /// start report.
    pub fn new(size: (u32, u32), scale: SurfaceScale, target: &'a mut dyn RenderTarget) -> Self {
        Self {
            size,
            scale,
            target,
            keep_previous: false,
            signal: EngineSignal::unheard(),
        }
    }
}

/// What the compositor did with the frames an engine handed it, counted since
/// some start (a counter may go back to zero).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameCounts {
    /// Frames the compositor took.
    pub rendered: u64,
    /// Frames that were replaced by the next one before the compositor took
    /// them: it did not draw the surface in time.
    pub dropped: u64,
}

/// What a video stream is, as far as it has been negotiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    /// Size of the decoded frames in pixels, once known.
    pub size: Option<(u32, u32)>,
    /// Frames per second as a fraction `(numerator, denominator)`, once known
    /// and if the stream has a fixed rate.
    pub frame_rate: Option<(u32, u32)>,
}

/// How far an `attach` got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attached {
    /// The wallpaper is on the surface.
    Shown,
    /// The engine is on the surface, but its first frame comes later (a video
    /// that has to be opened and decoded). It reports through
    /// [`RenderContext::signal`] when the frame is there, or that it cannot
    /// be. Until then the previous wallpaper stays; after a success the engine
    /// gets `take_over`, after a failure it is detached or dropped.
    Starting,
}

/// Rendering engine contract. Decouples the Wayland orchestrator
/// (surface lifecycle and visibility state) from media decoding implementations.
///
/// The calls an engine sees, in order: possibly `prepare` (once, right after it
/// was built, on another thread), then `attach` once per surface (and, if that
/// answered `Starting` and the start was reported as a success, `take_over`),
/// then `resize` for every change of that surface, then `detach` before the
/// surface goes away. After `detach` the next call is `attach` again, for a new
/// surface. An `attach` that failed may be repeated without a `detach` in
/// between. `resize` and `detach` may come while the engine is still starting.
///
/// `pause` and `resume` may come at any point of that, also before `attach`
/// and after `detach`: they say whether the wallpaper is to play while it is
/// shown, and the engine keeps the latest of them for every surface it is
/// attached to later. A new engine plays.
pub trait WallpaperEngine {
    /// Does ahead of time what `attach` would otherwise do while it draws, for
    /// a surface whose canvas is `canvas` physical pixels. It is called on a
    /// loader thread, before the engine is handed to the event loop, so slow
    /// work done here does not hold the loop up. It is only a head start:
    /// `attach` must work without it, and for any other size. It may be called
    /// again, for another size, before `attach`.
    fn prepare(&mut self, _canvas: (u32, u32)) -> Result<(), VeilError> {
        Ok(())
    }

    /// Puts the wallpaper on a surface it is not shown on yet: the first frame
    /// of an image, or the start of a video. If this fails, the surface must
    /// show what it showed before.
    fn attach(&mut self, ctx: &mut RenderContext<'_>) -> Result<Attached, VeilError>;

    /// Called after the engine has reported a successful start: it is the
    /// wallpaper of the surface from now on, and the previous one is removed. A
    /// video draws its background and starts to play. If this fails, the
    /// engine is dropped and the surface must show what it showed before.
    fn take_over(&mut self, _ctx: &mut RenderContext<'_>) -> Result<(), VeilError> {
        Ok(())
    }

    /// Shows the wallpaper again after the size or the scale of the surface it is
    /// attached to has changed.
    fn resize(&mut self, ctx: &mut RenderContext<'_>) -> Result<(), VeilError>;

    /// Releases the surface the engine was attached to, and everything that is
    /// only needed while it is shown (a pipeline, decoded pixels). It is called
    /// before the surface is destroyed (its output was unplugged or the compositor
    /// closed it), and for a wallpaper that is set while there is no surface. The
    /// engine must stop using the surface before this returns, and must work again
    /// after a later `attach` to a new surface. Calling it twice is harmless.
    fn detach(&mut self);

    /// The wallpaper cannot be seen (a fullscreen window is in front of it):
    /// a video stops producing frames, and keeps the one it shows. It may do
    /// that later, for example once it has started, and it reports what goes
    /// wrong in doing so the way it reports any other failure. An engine that
    /// does no work while it is shown has nothing to do.
    fn pause(&mut self) {}

    /// The wallpaper can be seen again: a video plays on from where it was
    /// paused.
    fn resume(&mut self) {}

    /// What the compositor did with the frames of a wallpaper that has taken
    /// over its surface, if the engine knows: `None` for an engine that hands
    /// it no stream of frames (an image), and while there is nothing to count.
    fn frame_counts(&self) -> Option<FrameCounts> {
        None
    }

    /// What the stream of a dynamic engine is (`veil status`): `None` for an
    /// engine without one (an image), and while it has no pipeline.
    fn stream(&self) -> Option<StreamInfo> {
        None
    }

    /// Indicates whether the engine requires continuous repainting (true for video, false for static images).
    fn is_dynamic(&self) -> bool;
}

/// Test doubles shared by the engine tests and the lifecycle tests in
/// `wayland::render`.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// What a [`FakeTarget`] was asked to show.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum Shown {
        Fill(Color),
        Frame { size: (u32, u32), pixels: Vec<u8> },
    }

    /// A [`RenderTarget`] with no compositor behind it: it hands out a canvas of
    /// a fixed size and records what was shown.
    pub(crate) struct FakeTarget {
        size: (u32, u32),
        pub(crate) shown: Vec<Shown>,
    }

    impl FakeTarget {
        /// A target whose canvas is `size` pixels.
        pub(crate) fn new(size: (u32, u32)) -> Self {
            Self {
                size,
                shown: Vec::new(),
            }
        }
    }

    impl RenderTarget for FakeTarget {
        fn fill(&mut self, color: Color) -> Result<(), VeilError> {
            self.shown.push(Shown::Fill(color));
            Ok(())
        }

        fn draw(&mut self, compose: &mut Compose<'_>) -> Result<(), VeilError> {
            let mut pixels = vec![0u8; self.size.0 as usize * self.size.1 as usize * 4];
            compose(&mut pixels, self.size)?;
            self.shown.push(Shown::Frame {
                size: self.size,
                pixels,
            });
            Ok(())
        }

        fn window(&self) -> WindowHandle {
            WindowHandle {
                display: std::ptr::null_mut(),
                surface: std::ptr::null_mut(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One engine for the file `input`, as the daemon builds it.
    fn from_path(input: &str, options: ScaleOptions) -> Result<Engine, VeilError> {
        Source::open(input, options)?.engine()
    }

    /// `from_path` fails; a `Box<dyn WallpaperEngine>` has no `Debug` for `expect_err`.
    fn error_of(input: &str) -> VeilError {
        match from_path(input, ScaleOptions::default()) {
            Ok(_) => panic!("{input:?} must not give an engine"),
            Err(e) => e,
        }
    }

    #[test]
    fn a_missing_file_is_refused_whatever_its_type() {
        let dir = std::env::temp_dir();
        for name in ["veil-missing.png", "veil-missing.mp4", "veil-missing"] {
            let path = dir.join(format!("{}-{name}", std::process::id()));
            let error = error_of(path.to_str().expect("UTF-8 temp dir"));
            assert!(
                matches!(error, VeilError::PathResolution { .. }),
                "{name}: {error:?}"
            );
        }
        assert!(matches!(error_of(""), VeilError::PathResolution { .. }));
    }

    /// Writes `content` to a file called `name` in the temp directory; the
    /// caller removes it.
    fn temp_file(name: &str, content: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("veil-engine-{}-{name}", std::process::id()));
        std::fs::write(&path, content).expect("write the test file");
        path
    }

    /// A 2x2 PNG of one colour, as the bytes of a file.
    fn png(color: [u8; 4]) -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        ::image::RgbaImage::from_pixel(2, 2, ::image::Rgba(color))
            .write_to(&mut bytes, ::image::ImageFormat::Png)
            .expect("encode the PNG");
        bytes.into_inner()
    }

    #[test]
    fn a_png_that_is_named_like_a_video_is_shown_as_the_image_it_is() {
        use crate::scaling::ScaleMode;
        use testing::{FakeTarget, Shown};

        for name in ["picture.mp4", "picture.mkv", "picture.webm", "picture"] {
            let path = temp_file(name, &png([255, 0, 0, 255]));
            let options = ScaleOptions {
                mode: ScaleMode::Stretch,
                ..ScaleOptions::default()
            };
            let engine = from_path(path.to_str().expect("UTF-8 temp dir"), options);
            let mut engine = match engine {
                Ok(engine) => engine,
                Err(e) => panic!("{name}: {e}"),
            };
            assert!(!engine.is_dynamic(), "{name} was taken for a video");

            let mut target = FakeTarget::new((2, 2));
            let attached = engine.attach(&mut RenderContext::new(
                (2, 2),
                SurfaceScale::ONE,
                &mut target,
            ));
            std::fs::remove_file(&path).expect("remove the test file");
            assert!(matches!(attached, Ok(Attached::Shown)), "{name}");
            // Red, in the byte order of the canvas (B, G, R, X).
            assert_eq!(
                target.shown,
                [Shown::Frame {
                    size: (2, 2),
                    pixels: [0, 0, 255, 255].repeat(4),
                }],
                "{name}"
            );
        }
    }

    #[test]
    fn a_video_that_is_named_like_an_image_goes_to_the_video_engine() {
        // The start of a Matroska file is enough: building the engine does not
        // open the file yet.
        let matroska = [0x1A, 0x45, 0xDF, 0xA3, 0x01, 0, 0, 0, 0, 0, 0, 0x23];
        for name in ["clip.png", "clip.jpg", "clip"] {
            let path = temp_file(name, &matroska);
            let engine = from_path(
                path.to_str().expect("UTF-8 temp dir"),
                ScaleOptions::default(),
            );
            std::fs::remove_file(&path).expect("remove the test file");
            match engine {
                Ok(engine) => assert!(engine.is_dynamic(), "{name} was taken for an image"),
                // The image decoder must not have been asked.
                Err(e @ VeilError::Image { .. }) => panic!("{name} was taken for an image: {e}"),
                // No GStreamer, or not the plugins a video needs.
                Err(e) => eprintln!("skipping {name}: no video engine can be built here: {e}"),
            }
        }
    }

    #[test]
    fn what_cannot_be_a_wallpaper_is_refused_before_a_decoder_sees_it() {
        for name in ["empty.png", "empty.mp4"] {
            let path = temp_file(name, b"");
            let error = error_of(path.to_str().expect("UTF-8 temp dir"));
            std::fs::remove_file(&path).expect("remove the test file");
            assert!(
                matches!(error, VeilError::EmptyFile(_)),
                "{name}: {error:?}"
            );
            assert!(error.to_string().ends_with("is empty"), "{error}");
        }

        let path = temp_file("notes.txt", b"just some notes\n");
        let error = error_of(path.to_str().expect("UTF-8 temp dir"));
        std::fs::remove_file(&path).expect("remove the test file");
        assert!(matches!(error, VeilError::UnknownMedia(_)), "{error:?}");
        assert!(
            error.to_string().contains("neither an image nor a video"),
            "{error}"
        );
    }

    #[test]
    fn a_directory_is_refused_even_if_it_is_named_like_a_video() {
        let dir = std::env::temp_dir().join(format!("veil-dir-{}.mp4", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the directory");
        let error = error_of(dir.to_str().expect("UTF-8 temp dir"));
        std::fs::remove_dir(&dir).expect("remove the directory");

        assert!(matches!(error, VeilError::NotAFile(_)), "{error:?}");
        assert!(
            error.to_string().contains("is not a regular file"),
            "{error}"
        );
    }

    /// What `engine` shows when it is attached to a surface with this canvas.
    fn shown_on(engine: &mut Engine, canvas: (u32, u32)) -> Vec<testing::Shown> {
        let mut target = testing::FakeTarget::new(canvas);
        engine
            .attach(&mut RenderContext::new(
                canvas,
                SurfaceScale::ONE,
                &mut target,
            ))
            .expect("attach");
        target.shown
    }

    #[test]
    fn engines_for_several_outputs_show_what_one_engine_would_show() {
        // Two colours side by side, so that a wrong scaling shows.
        let mut picture = ::image::RgbaImage::from_pixel(4, 2, ::image::Rgba([255, 0, 0, 255]));
        for y in 0..2 {
            picture.put_pixel(3, y, ::image::Rgba([0, 0, 255, 255]));
        }
        let path =
            std::env::temp_dir().join(format!("veil-engine-{}-outputs.png", std::process::id()));
        picture.save(&path).expect("write the PNG");
        let input = path.to_str().expect("UTF-8 temp dir");
        let options = ScaleOptions::default();

        // What a single engine shows on each canvas.
        let canvases = [Some((4, 2)), Some((8, 3)), Some((4, 2)), None];
        let expected: Vec<_> = [(4, 2), (8, 3), (4, 2), (5, 5)]
            .into_iter()
            .map(|canvas| {
                let mut engine = Source::open(input, options)
                    .and_then(|source| source.engine())
                    .expect("one engine");
                shown_on(&mut engine, canvas)
            })
            .collect();

        let source = Source::open(input, options).expect("open");
        assert!(!source.is_video());
        let mut engines = source.engines(&canvases).expect("engines");
        assert_eq!(engines.len(), 4);
        assert!(engines.iter().all(|engine| !engine.is_dynamic()));
        // The prepared ones need the file no more, and the one without a
        // canvas still holds the decoded picture.
        std::fs::remove_file(&path).expect("remove the PNG");

        for (i, (engine, canvas)) in engines
            .iter_mut()
            .zip([(4, 2), (8, 3), (4, 2), (5, 5)])
            .enumerate()
        {
            assert_eq!(shown_on(engine, canvas), expected[i], "engine {i}");
        }
    }

    #[test]
    fn no_canvas_means_no_engine() {
        let path = temp_file("none.png", &png([1, 2, 3, 255]));
        let source = Source::open(
            path.to_str().expect("UTF-8 temp dir"),
            ScaleOptions::default(),
        );
        std::fs::remove_file(&path).expect("remove the test file");
        let engines = source
            .expect("open")
            .engines(&[])
            .expect("nothing to build");
        assert!(engines.is_empty());
    }
}
