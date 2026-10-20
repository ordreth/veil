//! Video engines that are built and thrown away must leave nothing behind.
//!
//! This is a binary of its own with a single test, because it compares the
//! thread and file-descriptor counts of the whole process, which other tests
//! running in parallel would change.
//!
//! No compositor is needed: the engines are built, as a loader thread builds
//! them, but never attached. That is the pipeline `VideoEngine::new` builds to
//! validate a file, the one of a `veil set` that a newer request replaced, and
//! the one of a video that is set while its output is unplugged. What a pipeline
//! that plays leaves behind is tested in `src/engine/video.rs`.

use gstreamer as gst;
use veil::engine::video::VideoEngine;
use veil::engine::WallpaperEngine;
use veil::scaling::ScaleOptions;

/// Threads of this process, from `/proc/self/status`.
fn threads() -> usize {
    let status = std::fs::read_to_string("/proc/self/status").expect("read the process status");
    status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))
        .and_then(|count| count.trim().parse().ok())
        .expect("a thread count")
}

/// Open file descriptors of this process (plus the one that lists them).
fn file_descriptors() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .expect("list the file descriptors")
        .count()
}

#[test]
fn a_hundred_engines_that_are_never_shown_leave_no_thread_or_descriptor_behind() {
    gst::init().expect("gstreamer init");
    for factory in ["playbin", "videoconvert", "videocrop", "waylandsink"] {
        if gst::ElementFactory::find(factory).is_none() {
            eprintln!("skipping: GStreamer element '{factory}' is not installed");
            return;
        }
    }

    // Only the path has to exist: the pipelines are built but never started.
    let path = std::env::temp_dir().join(format!("veil-threads-{}.webm", std::process::id()));
    std::fs::write(&path, b"").expect("write test file");
    let build = || {
        VideoEngine::new(
            path.to_str().expect("UTF-8 temp path"),
            ScaleOptions::default(),
        )
        .expect("build the pipeline")
    };

    // What GStreamer sets up once, with the first pipeline, is not a leak.
    drop(build());
    let before = (threads(), file_descriptors());

    for round in 0..100 {
        let mut engine = build();
        // Both ways an unused engine goes: detached first (set while there is
        // no surface), or dropped as it is (replaced by a newer request).
        if round % 2 == 0 {
            engine.detach();
        }
        drop(engine);
    }
    let after = (threads(), file_descriptors());
    std::fs::remove_file(&path).expect("remove test file");

    assert_eq!(after, before, "(threads, file descriptors)");
}
