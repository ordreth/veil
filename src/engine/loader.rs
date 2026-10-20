//! Building engines off the event loop.
//!
//! Everything about a new wallpaper that can be slow happens in [`load`]:
//! checking the file, decoding an image and scaling it for the surfaces, or
//! starting GStreamer and building pipelines. The daemon runs it on a thread
//! of its own ([`spawn`]), so the event loop keeps serving the compositor and
//! the control socket meanwhile. The thread ends with the load: an idle daemon
//! has no loader thread.

use super::image::ImageEngine;
use super::{Engine, Source};
use crate::error::VeilError;
use crate::scaling::ScaleOptions;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// What to load, and for which surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadRequest {
    /// Path of the media file. `None`: no file, the background colour of
    /// `options` alone (`veil clear`).
    pub input: Option<String>,
    pub options: ScaleOptions,
    /// One entry per output the wallpaper is for: the canvas size, in physical
    /// pixels, of its surface, or `None` if it has no surface right now.
    pub canvases: Vec<Option<(u32, u32)>>,
}

/// The engines that are ready to be attached, one per entry of
/// `LoadRequest::canvases` and in that order, or the reason there are none.
pub type Loaded = Result<Vec<Engine>, VeilError>;

/// Builds the engines for `request` and lets them prepare their first frames
/// for the surfaces. The file is read and decoded once, whatever the number of
/// outputs. This blocks for as long as the file takes.
pub fn load(request: &LoadRequest) -> Loaded {
    let Some(input) = &request.input else {
        let color = request.options.bg_color;
        return Ok(request
            .canvases
            .iter()
            .map(|_| -> Engine { Box::new(ImageEngine::solid(color)) })
            .collect());
    };
    Source::open(input, request.options)?.engines(&request.canvases)
}

/// Runs [`load`] on a new thread and hands the result to `deliver`, which is
/// called on that thread. An error means that the thread could not be started;
/// `deliver` is not called then.
///
/// The thread inherits the signal mask of the caller. The daemon calls this
/// from its event loop thread, where SIGINT and SIGTERM are blocked, so neither
/// this thread nor the ones an engine starts from it can take those signals.
pub fn spawn(
    request: LoadRequest,
    deliver: impl FnOnce(Loaded) + Send + 'static,
) -> std::io::Result<()> {
    let what = match &request.input {
        Some(input) => input.clone(),
        None => format!("the background colour {}", request.options.bg_color),
    };
    on_loader_thread(what, move || load(&request), deliver)
}

/// Lets engines that are already loaded prepare their first frames again, for
/// surfaces whose canvas has changed while the engines were loaded: each engine
/// whose entry in `canvases` is `Some` is prepared for that size, the others
/// are left as they are. Like [`spawn`], this runs on a new thread and delivers
/// the engines there, in the same order. If the thread cannot be started, the
/// engines are dropped with the error.
pub fn spawn_prepare(
    mut engines: Vec<Engine>,
    canvases: Vec<Option<(u32, u32)>>,
    deliver: impl FnOnce(Loaded) + Send + 'static,
) -> std::io::Result<()> {
    let work = move || {
        for (engine, canvas) in engines.iter_mut().zip(canvases) {
            if let Some(canvas) = canvas {
                engine.prepare(canvas)?;
            }
        }
        Ok(engines)
    };
    on_loader_thread("a wallpaper for a resized surface".into(), work, deliver)
}

/// Runs `work` on a thread named `veil-loader` and hands its result to `deliver`.
fn on_loader_thread(
    what: String,
    work: impl FnOnce() -> Loaded + Send + 'static,
    deliver: impl FnOnce(Loaded) + Send + 'static,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("veil-loader".into())
        .spawn(move || {
            // A decoder that panics must not leave the daemon waiting for a
            // result that never comes. (Release builds abort on panic.)
            let loaded = catch_unwind(AssertUnwindSafe(work))
                .unwrap_or_else(|_| Err(VeilError::Load(format!("loading '{what}' panicked"))));
            deliver(loaded);
        })
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testing::{FakeTarget, Shown};
    use crate::engine::{RenderContext, WallpaperEngine};
    use crate::scaling::{ScaleMode, SurfaceScale};
    use image::{Rgba, RgbaImage};
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Writes a 4x2 red PNG to a unique temp path; the caller removes it.
    fn temp_png(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("veil-loader-{}-{name}.png", std::process::id()));
        RgbaImage::from_pixel(4, 2, Rgba([255, 0, 0, 255]))
            .save(&path)
            .expect("write test PNG");
        path
    }

    fn request(path: &std::path::Path, canvas: Option<(u32, u32)>) -> LoadRequest {
        LoadRequest {
            input: Some(path.to_str().expect("UTF-8 temp path").to_string()),
            options: ScaleOptions {
                mode: ScaleMode::Stretch,
                ..ScaleOptions::default()
            },
            canvases: vec![canvas],
        }
    }

    /// The one engine `request` (for one output) gives.
    fn load_one(request: &LoadRequest) -> Engine {
        let mut engines = load(request).expect("load");
        assert_eq!(engines.len(), 1);
        engines.pop().expect("one engine")
    }

    /// What `engine` shows when it is attached to a surface with this canvas.
    fn attach(engine: &mut dyn WallpaperEngine, canvas: (u32, u32)) -> Vec<Shown> {
        let mut target = FakeTarget::new(canvas);
        engine
            .attach(&mut RenderContext::new(
                canvas,
                SurfaceScale::default(),
                &mut target,
            ))
            .expect("attach");
        target.shown
    }

    #[test]
    fn a_missing_file_is_an_error_not_an_engine() {
        let path =
            std::env::temp_dir().join(format!("veil-loader-{}-none.png", std::process::id()));
        let error = match load(&request(&path, Some((8, 8)))) {
            Ok(_) => panic!("a missing file must not give an engine"),
            Err(e) => e,
        };
        assert!(
            matches!(error, VeilError::PathResolution { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_loaded_image_needs_its_file_no_more_for_the_surface_it_was_loaded_for() {
        let path = temp_png("prepared");
        let mut engine = load_one(&request(&path, Some((8, 4))));
        // Everything the first frame needs was done by the load: the file can
        // go, and the attach still shows the image.
        std::fs::remove_file(&path).expect("remove the PNG");

        let shown = attach(engine.as_mut(), (8, 4));
        let [Shown::Frame { size, pixels }] = shown.as_slice() else {
            panic!("expected one frame, got {shown:?}");
        };
        assert_eq!(*size, (8, 4));
        // Red, as XRGB8888.
        assert!(pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [0, 0, 255, 255]));
    }

    #[test]
    fn a_load_without_a_file_gives_the_background_colour_for_every_output() {
        let color = crate::scaling::Color {
            r: 0x10,
            g: 0x20,
            b: 0x30,
        };
        let request = LoadRequest {
            input: None,
            options: ScaleOptions {
                bg_color: color,
                ..ScaleOptions::default()
            },
            canvases: vec![Some((8, 4)), None],
        };
        let engines = load(&request).expect("no file to fail on");
        assert_eq!(engines.len(), 2);
        for mut engine in engines {
            assert!(!engine.is_dynamic());
            assert!(matches!(
                attach(engine.as_mut(), (4, 4)).as_slice(),
                [Shown::Fill(c)] if *c == color
            ));
        }
    }

    #[test]
    fn without_a_surface_nothing_is_prepared() {
        let path = temp_png("unprepared");
        let mut engine = load_one(&request(&path, None));
        // The decoded image is still what the engine holds, so it can be shown
        // on a surface of any size.
        let shown = attach(engine.as_mut(), (2, 2));
        std::fs::remove_file(&path).expect("remove the PNG");
        assert!(matches!(
            shown.as_slice(),
            [Shown::Frame { size: (2, 2), .. }]
        ));
    }

    #[test]
    fn spawn_loads_on_another_thread_and_delivers_there() {
        let path = temp_png("spawn");
        let (results, delivered) = mpsc::channel();
        spawn(request(&path, Some((4, 2))), move |loaded| {
            let thread = std::thread::current();
            let _ = results.send((loaded.is_ok(), thread.id(), thread.name().map(String::from)));
        })
        .expect("start the loader thread");

        let (ok, thread, name) = delivered
            .recv_timeout(Duration::from_secs(20))
            .expect("the loader delivers a result");
        std::fs::remove_file(&path).expect("remove the PNG");
        assert!(ok);
        assert_ne!(thread, std::thread::current().id());
        assert_eq!(name.as_deref(), Some("veil-loader"));
    }

    #[test]
    fn spawn_delivers_errors_too() {
        let path =
            std::env::temp_dir().join(format!("veil-loader-{}-gone.png", std::process::id()));
        let (results, delivered) = mpsc::channel();
        spawn(request(&path, None), move |loaded| {
            let _ = results.send(loaded.map(drop).map_err(|e| e.to_string()));
        })
        .expect("start the loader thread");

        let error = delivered
            .recv_timeout(Duration::from_secs(20))
            .expect("the loader delivers a result")
            .expect_err("the file does not exist");
        assert!(error.contains("Failed to resolve path"), "{error}");
    }

    #[test]
    fn spawn_prepare_prepares_only_the_engines_whose_surface_changed() {
        let path = temp_png("prepare-again");
        let mut request = request(&path, Some((4, 2)));
        request.canvases.push(Some((4, 2)));
        let engines = load(&request).expect("load");

        // The second output was resized to 8x4 while the wallpaper loaded.
        let (results, delivered) = mpsc::channel();
        spawn_prepare(engines, vec![None, Some((8, 4))], move |loaded| {
            let _ = results.send(loaded);
        })
        .expect("start the loader thread");
        let mut engines = delivered
            .recv_timeout(Duration::from_secs(20))
            .expect("the loader delivers")
            .expect("prepared");
        // Both first frames are ready: the file is not needed any more.
        std::fs::remove_file(&path).expect("remove the PNG");

        assert_eq!(engines.len(), 2);
        let sizes: Vec<_> = engines
            .iter_mut()
            .zip([(4, 2), (8, 4)])
            .map(
                |(engine, canvas)| match attach(engine.as_mut(), canvas).as_slice() {
                    [Shown::Frame { size, .. }] => *size,
                    other => panic!("expected one frame, got {other:?}"),
                },
            )
            .collect();
        assert_eq!(sizes, [(4, 2), (8, 4)]);
    }
}
