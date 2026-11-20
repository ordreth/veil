//! Video frames in dmabufs, on a GPU: a VA-API decoder's frames go to a
//! headless sway that renders on an Intel or AMD GPU, and the colours on
//! screen match ffmpeg's decode of the same clip.
//!
//! - `vulkan`: sway is told how the frames' colours are coded
//!   (`wp_color_representation_v1`), so BT.709 and BT.601 videos both go to it
//!   in dmabufs.
//! - `gles2`: it is not told, and takes YUV as BT.601. BT.601 videos go in
//!   dmabufs; BT.709 ones are converted on the CPU.
//!
//! CI has no such GPU, so these tests are ignored, and they have a test binary
//! of their own: `--test headless -- --ignored` does not run them. Run them
//! where the GPU is, with the tools of the headless dev shell:
//!
//! ```text
//! nix develop .#headless -c cargo test --test gpu -- --ignored --test-threads=1
//! ```
//!
//! The render node is the first one of an Intel or AMD GPU, or the one in
//! `VEIL_TEST_RENDER_NODE`. The decoder is GStreamer's `vah264dec`, the VA-API
//! decoder of the first VA-API device; a test fails if the frames come from
//! another decoder (as no dmabufs then come).

mod sway;

use image::RgbImage;
use std::path::{Path, PathBuf};
use std::process::Command;
use sway::{arg, wait_for, Media, Renderer, Rgb, Sway};
use veil::ipc::report::State;

/// How far a colour on screen may be from ffmpeg's, in each channel: the
/// conversions round differently. Decoding with the other matrix or range is
/// off by more than 10 in a channel of some quadrant.
const TOLERANCE: u8 = 4;

/// The size of the clips and of the output: the video is shown 1:1.
const SIZE: (u32, u32) = (640, 360);

/// The colours of the clip's quadrants: top left, top right, bottom left,
/// bottom right. Saturated enough that the matrices tell them apart, inside
/// the gamut of both.
const QUADRANTS: [Rgb; 4] = [[200, 60, 40], [60, 170, 80], [50, 70, 190], [220, 200, 70]];

/// The middle of each quadrant, away from the chroma of its neighbours.
fn spots() -> [(u32, u32); 4] {
    let (width, height) = SIZE;
    [
        (width / 4, height / 4),
        (width * 3 / 4, height / 4),
        (width / 4, height * 3 / 4),
        (width * 3 / 4, height * 3 / 4),
    ]
}

/// How a clip's colours are coded, as ffmpeg names it.
#[derive(Clone, Copy, Debug)]
enum Coding {
    Bt709,
    Bt601,
}

impl Coding {
    /// The matrix of `scale`'s `out_color_matrix`.
    fn matrix(self) -> &'static str {
        match self {
            Self::Bt709 => "bt709",
            Self::Bt601 => "bt601",
        }
    }

    /// The colour space, primaries and transfer the clip is tagged with.
    fn tag(self) -> &'static str {
        match self {
            Self::Bt709 => "bt709",
            Self::Bt601 => "smpte170m",
        }
    }
}

/// Where the path the frames take shows in the daemon's log.
const IN_DMABUFS: &str = "video frames go to the compositor as they are, in dmabufs";
const NOT_TOLD: &str = "the compositor cannot be told how the video's colours are coded";
const ON_THE_CPU: &str = "video frames are converted on the CPU";

#[test]
#[ignore = "needs an Intel or AMD GPU with VA-API: nix develop .#headless -c cargo test --test gpu -- --ignored"]
fn vulkan_takes_bt709_and_bt601_frames_in_dmabufs() {
    let node = render_node();
    let sway = Sway::start_with(
        "vulkan",
        &[SIZE],
        &Renderer::Gpu {
            name: "vulkan",
            node: &node,
        },
    );
    for coding in [Coding::Bt709, Coding::Bt601] {
        let daemon = show(&sway, coding);
        let log = daemon.log();
        assert!(log.contains(IN_DMABUFS), "{coding:?} in dmabufs:\n{log}");
        assert!(
            !log.contains(ON_THE_CPU),
            "{coding:?} not on the CPU:\n{log}"
        );
        assert_eq!(daemon.stop().code(), Some(0));
    }
}

#[test]
#[ignore = "needs an Intel or AMD GPU with VA-API: nix develop .#headless -c cargo test --test gpu -- --ignored"]
fn gles2_takes_bt601_frames_in_dmabufs_and_bt709_ones_from_the_cpu() {
    let node = render_node();
    let sway = Sway::start_with(
        "gles2",
        &[SIZE],
        &Renderer::Gpu {
            name: "gles2",
            node: &node,
        },
    );

    let daemon = show(&sway, Coding::Bt601);
    let log = daemon.log();
    assert!(log.contains(IN_DMABUFS), "BT.601 in dmabufs:\n{log}");
    assert!(!log.contains(ON_THE_CPU), "BT.601 not on the CPU:\n{log}");
    assert_eq!(daemon.stop().code(), Some(0));

    // The decoder offers dmabufs first: they are refused, and the pipeline
    // starts again with frames in system memory.
    let daemon = show(&sway, Coding::Bt709);
    let log = daemon.log();
    assert!(log.contains(NOT_TOLD), "BT.709 refused in dmabufs:\n{log}");
    assert!(log.contains(ON_THE_CPU), "BT.709 on the CPU:\n{log}");
    assert_eq!(daemon.stop().code(), Some(0));
}

/// Starts a daemon with the clip coded with `coding` and waits until the
/// output shows ffmpeg's colours.
fn show(sway: &Sway, coding: Coding) -> sway::Daemon {
    let clip = clip(coding);
    let expected = reference(&clip);
    let daemon = sway.daemon_with(&["--input", arg(&clip), "--mode", "fill"], |command| {
        // The VA-API decoder before any other, NVIDIA's included.
        command.env("GST_PLUGIN_FEATURE_RANK", "vah264dec:512,nvh264dec:0");
    });
    sway.wait_for_state("HEADLESS-1", State::Playing);
    wait_for(&format!("ffmpeg's colours of the {coding:?} clip"), || {
        let shot = sway.shot("HEADLESS-1");
        let found: Vec<Rgb> = spots()
            .iter()
            .map(|&(x, y)| shot.get_pixel(x, y).0)
            .collect();
        if found.iter().zip(&expected).all(|(a, b)| near(*a, *b)) {
            Ok(())
        } else {
            Err(format!(
                "found {found:?}, ffmpeg has {expected:?}; daemon log:\n{}",
                daemon.log()
            ))
        }
    });
    daemon
}

fn near(a: Rgb, b: Rgb) -> bool {
    a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= TOLERANCE)
}

/// The render node of the first Intel or AMD GPU, or `VEIL_TEST_RENDER_NODE`.
fn render_node() -> PathBuf {
    if let Some(node) = std::env::var_os("VEIL_TEST_RENDER_NODE") {
        return node.into();
    }
    let mut nodes: Vec<String> = std::fs::read_dir("/sys/class/drm")
        .expect("read /sys/class/drm")
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            name.starts_with("renderD").then_some(name)
        })
        .collect();
    nodes.sort();
    nodes
        .into_iter()
        .find(|name| {
            let vendor = std::fs::read_to_string(format!("/sys/class/drm/{name}/device/vendor"))
                .unwrap_or_default();
            // Intel, AMD.
            matches!(vendor.trim(), "0x8086" | "0x1002")
        })
        .map(|name| Path::new("/dev/dri").join(name))
        .expect("no render node of an Intel or AMD GPU; set VEIL_TEST_RENDER_NODE")
}

/// An H.264 clip of 2 s of the four quadrants, its colours coded and tagged
/// with `coding` in the limited range.
fn clip(coding: Coding) -> PathBuf {
    let path = Media::dir().join(format!("quadrants-{}.mp4", coding.matrix()));
    if path.exists() {
        return path;
    }
    let (width, height) = SIZE;
    let picture_path = Media::dir().join("quadrants-source.png");
    let picture = RgbImage::from_fn(width, height, |x, y| {
        let index = usize::from(x >= width / 2) + 2 * usize::from(y >= height / 2);
        image::Rgb(QUADRANTS[index])
    });
    picture
        .save_with_format(&picture_path, image::ImageFormat::Png)
        .expect("write the picture of the clip");
    let partial = path.with_extension(format!("{}.part", std::process::id()));
    let tag = coding.tag();
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-loop", "1", "-framerate", "30", "-i"])
        .arg(&picture_path)
        .args(["-frames:v", "60", "-vf"])
        .arg(format!(
            "scale=out_color_matrix={}:out_range=tv,format=yuv420p",
            coding.matrix()
        ))
        // The High profile, which every VA-API H.264 decoder takes.
        .args(["-c:v", "libx264", "-profile:v", "high", "-qp", "8"])
        .args([
            "-colorspace",
            tag,
            "-color_primaries",
            tag,
            "-color_trc",
            tag,
        ])
        .args(["-color_range", "tv", "-f", "mp4"])
        .arg(&partial)
        .output()
        .expect("run ffmpeg");
    assert!(
        output.status.success(),
        "make the clip {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::rename(&partial, &path).expect("move the clip into place");
    path
}

/// The colours of the quadrants in ffmpeg's decode of the first frame of
/// `clip`, in full-range RGB.
fn reference(clip: &Path) -> Vec<Rgb> {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(clip)
        .args(["-frames:v", "1", "-vf", "scale=out_range=pc,format=rgb24"])
        .args(["-f", "image2pipe", "-c:v", "ppm", "-"])
        .output()
        .expect("run ffmpeg");
    assert!(
        output.status.success(),
        "decode {}: {}",
        clip.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let frame = image::load_from_memory(&output.stdout)
        .expect("ffmpeg's frame")
        .to_rgb8();
    let colours: Vec<Rgb> = spots()
        .iter()
        .map(|&(x, y)| frame.get_pixel(x, y).0)
        .collect();
    // ffmpeg reads the clip's tags: its colours are the picture's.
    for (decoded, source) in colours.iter().zip(QUADRANTS) {
        assert!(
            near(*decoded, source),
            "ffmpeg decodes {} as {colours:?}, the picture is {QUADRANTS:?}",
            clip.display()
        );
    }
    colours
}
