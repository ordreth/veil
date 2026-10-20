//! Which engine shows a file.
//!
//! The content of the file decides, read from its first bytes: a PNG that is
//! named `clip.mp4` is still a picture, and a WebM that is named `still.png`
//! is still a video. The extension is only a hint, for content that is not
//! recognised here (a container this module does not know, an image format
//! without a signature).
//!
//! Animations in picture formats:
//!
//! * **GIF:** one with a single frame is an image. One with more is played by
//!   the video engine, which loops it like any video (GStreamer needs
//!   `gst-libav` for that).
//! * **WebP:** always an image. An animated one shows its first frame:
//!   GStreamer cannot play animated WebP (checked with 1.28.7).
//! * **APNG and others:** not looked for; the image engine shows what the
//!   decoder gives for the file, which is its first (or default) picture.
//!
//! Pictures in a format that is recognised but cannot be decoded here (HEIF,
//! JPEG XL) are refused as that. Left to the rule above, a HEIC photo would be
//! an ISO media file like an MP4, and GStreamer would say that a video cannot
//! be played. AVIF is known to `image` and goes to the image engine, where the
//! decoder refuses it by name in a build without AVIF decoding (the default):
//! `ImageFormat::reading_enabled` says yes for it all the same, so it cannot
//! be refused earlier.

use crate::error::VeilError;
use image::codecs::gif::GifDecoder;
use image::{AnimationDecoder, ImageDecoder, ImageFormat};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

/// How many bytes of a file are read to tell what it is. The longest
/// signature looked for is the third sync byte of an MPEG transport stream.
const HEADER_LEN: usize = 512;

/// Extensions that mean "video" when the content does not say what it is.
/// GStreamer knows far more containers than [`sniff`] does, so such a file is
/// handed to it; whether it can be played is then GStreamer's answer.
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "ogv", "mpg", "mpeg", "ts", "m2ts", "mts", "wmv",
    "asf", "flv", "3gp", "vob",
];

/// The two kinds of wallpaper, and so the two engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
}

/// What the first bytes of a file say it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// A picture format. GIF and WebP may hold an animation.
    Image(ImageFormat),
    /// A picture format, by its name, that no decoder here reads.
    UnsupportedImage(&'static str),
    /// A video container, by its name for the log.
    Video(&'static str),
    /// Nothing this module knows.
    Unknown,
}

/// Tells from the first bytes of a file what it is. `header` may be shorter
/// than the signatures looked for (a short file); it is then just not them.
pub fn sniff(header: &[u8]) -> Content {
    // Pictures first: the signatures of the two groups do not overlap, except
    // that an AVIF picture is an ISO media file like an MP4, and `image`
    // knows which one it is.
    if let Ok(format) = image::guess_format(header) {
        return Content::Image(format);
    }
    if let Some(format) = unsupported_picture(header) {
        return Content::UnsupportedImage(format);
    }
    match video_container(header) {
        Some(container) => Content::Video(container),
        None => Content::Unknown,
    }
}

/// The picture format `header` starts, if it is one that is known here only
/// by its signature. They come before the video containers: HEIF is an ISO
/// media file, told from an MP4 by the brand that follows `ftyp`.
fn unsupported_picture(header: &[u8]) -> Option<&'static str> {
    const HEIF_BRANDS: [&[u8; 4]; 8] = [
        b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"mif1", b"msf1",
    ];
    let brand = header.get(8..12);
    if header.get(4..8) == Some(b"ftyp".as_slice())
        && HEIF_BRANDS
            .iter()
            .any(|known| brand == Some(known.as_slice()))
    {
        Some("HEIF")
    } else if header.starts_with(&[0xFF, 0x0A]) || header.starts_with(b"\0\0\0\x0cJXL \r\n\x87\n") {
        // The bare codestream, and the container.
        Some("JPEG XL")
    } else {
        None
    }
}

/// The video container `header` starts, if it is one of the common ones.
fn video_container(header: &[u8]) -> Option<&'static str> {
    let at = |offset: usize, signature: &[u8]| {
        header.get(offset..offset.saturating_add(signature.len())) == Some(signature)
    };
    // A box (or atom) of four letters follows its four-byte length.
    let starts_with_box = |names: &[&[u8; 4]]| names.iter().any(|name| at(4, &name[..]));
    // Packets of 188 bytes that each start with the sync byte; three in a row
    // are not a coincidence. `shift` is 4 for the M2TS variant, whose packets
    // carry a timestamp in front.
    let transport_stream =
        |shift: usize, packet: usize| (0..3).all(|n| at(shift + n * packet, &[0x47]));

    if at(0, &[0x1A, 0x45, 0xDF, 0xA3]) {
        Some("Matroska or WebM")
    } else if starts_with_box(&[b"ftyp"]) {
        Some("MP4 or QuickTime")
    } else if starts_with_box(&[b"moov", b"mdat", b"free", b"skip", b"wide", b"pnot"]) {
        // QuickTime files from before the `ftyp` box.
        Some("QuickTime")
    } else if at(0, b"RIFF") && at(8, b"AVI ") {
        Some("AVI")
    } else if at(0, b"OggS") {
        Some("Ogg")
    } else if at(0, b"FLV\x01") {
        Some("Flash Video")
    } else if at(0, &[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11]) {
        Some("ASF or WMV")
    } else if at(0, &[0x00, 0x00, 0x01, 0xBA]) {
        Some("MPEG program stream")
    } else if transport_stream(0, 188) || transport_stream(4, 192) {
        Some("MPEG transport stream")
    } else {
        None
    }
}

/// What the extension of `path` suggests. Used only when the content does not
/// say what the file is.
pub fn extension_hint(path: &Path) -> Option<MediaKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        Some(MediaKind::Video)
    } else if ImageFormat::from_extension(&extension).is_some() {
        // Also the way in for a format without a signature (TGA).
        Some(MediaKind::Image)
    } else {
        None
    }
}

/// Whether the header of a WebP file says that it holds an animation: the
/// animation bit in the flags of its `VP8X` chunk.
fn webp_is_animated(header: &[u8]) -> bool {
    const ANIMATION: u8 = 0x02;
    header.get(12..16) == Some(b"VP8X".as_slice())
        && header.get(20).is_some_and(|flags| flags & ANIMATION != 0)
}

/// Whether the GIF at `path` has more than one frame. This decodes its first
/// two frames, within the limits that hold for every image. A GIF that cannot
/// be decoded that far is not an animation: the image engine shows its first
/// frame, or reports what is wrong with it.
fn gif_is_animated(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(mut decoder) = GifDecoder::new(BufReader::new(file)) else {
        return false;
    };
    if decoder.set_limits(super::image::limits()).is_err() {
        return false;
    }
    decoder.into_frames().take(2).filter(Result::is_ok).count() == 2
}

/// Reads the first bytes of the file at `path`. This is where a path that
/// cannot be a wallpaper is refused, before any decoder sees it, so the error
/// names the real problem and not what a decoder makes of it: a path that does
/// not exist, one that is not a regular file (a directory, a device, a pipe,
/// which would block), an empty file, and a file that cannot be read.
fn read_header(path: &Path) -> Result<Vec<u8>, VeilError> {
    let shown = || path.display().to_string();
    let metadata = std::fs::metadata(path).map_err(|source| VeilError::PathResolution {
        path: shown(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(VeilError::NotAFile(shown()));
    }
    if metadata.len() == 0 {
        return Err(VeilError::EmptyFile(shown()));
    }

    let unreadable = |source| VeilError::Unreadable {
        path: shown(),
        source,
    };
    let file = File::open(path).map_err(unreadable)?;
    let mut header = Vec::with_capacity(HEADER_LEN);
    file.take(HEADER_LEN as u64)
        .read_to_end(&mut header)
        .map_err(unreadable)?;
    Ok(header)
}

/// Decides which engine shows the file at `path`: by its content, and by its
/// extension only if the content is not recognised. A file that is neither is
/// refused.
///
/// This reads the start of the file (and two frames of a GIF), so it belongs
/// on a loader thread like the rest of loading a wallpaper.
pub fn detect(path: &Path) -> Result<MediaKind, VeilError> {
    let header = read_header(path)?;
    let hint = extension_hint(path);
    let name = path.display();

    let kind = match sniff(&header) {
        Content::Image(ImageFormat::Gif) if gif_is_animated(path) => {
            tracing::debug!("'{name}' is an animated GIF; it is played as a video");
            return Ok(MediaKind::Video);
        }
        Content::Image(ImageFormat::WebP) if webp_is_animated(&header) => {
            tracing::warn!(
                "'{name}' is an animated WebP, which cannot be played; showing its first frame"
            );
            MediaKind::Image
        }
        Content::Image(format) if !format.reading_enabled() => {
            // The name people know it by: its usual extension, in capitals.
            let format = format
                .extensions_str()
                .first()
                .map_or_else(|| format!("{format:?}"), |ext| ext.to_ascii_uppercase());
            return Err(VeilError::UnsupportedImage {
                path: name.to_string(),
                format,
            });
        }
        Content::UnsupportedImage(format) => {
            return Err(VeilError::UnsupportedImage {
                path: name.to_string(),
                format: format.to_string(),
            });
        }
        Content::Image(format) => {
            tracing::debug!("'{name}' is an image ({format:?}) by its content");
            MediaKind::Image
        }
        Content::Video(container) => {
            tracing::debug!("'{name}' is a video ({container}) by its content");
            MediaKind::Video
        }
        Content::Unknown => {
            let Some(kind) = hint else {
                return Err(VeilError::UnknownMedia(name.to_string()));
            };
            tracing::debug!(
                "the content of '{name}' is not recognised; going by its extension: {kind:?}"
            );
            return Ok(kind);
        }
    };

    if hint.is_some_and(|hint| hint != kind) {
        let (is, named) = match kind {
            MediaKind::Image => ("an image", "a video"),
            MediaKind::Video => ("a video", "an image"),
        };
        tracing::info!("'{name}' is {is}, although it is named like {named}");
    }
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::{Frame, Rgba, RgbaImage};
    use std::path::PathBuf;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F'];
    const WEBP_STILL: &[u8] = b"RIFF\xf4\x01\0\0WEBPVP8 \xe8\x01\0\0";
    const WEBP_ANIMATED: &[u8] = b"RIFF\x56\x08\0\0WEBPVP8X\x0a\0\0\0\x02\0\0\0";
    const WEBP_EXTENDED_STILL: &[u8] = b"RIFF\x56\x08\0\0WEBPVP8X\x0a\0\0\0\x10\0\0\0";
    const MATROSKA: &[u8] = &[0x1A, 0x45, 0xDF, 0xA3, 0x01, 0, 0, 0, 0, 0, 0, 0x23];
    const MP4: &[u8] = b"\0\0\0\x20ftypisom\0\0\x02\0isomiso2avc1mp41";
    const QUICKTIME_OLD: &[u8] = b"\0\0\x0f\xa0moov\0\0\0\x6cmvhd";
    const AVI: &[u8] = b"RIFF\x24\x10\0\0AVI LIST";
    const OGG: &[u8] = b"OggS\0\x02\0\0\0\0\0\0\0\0";
    const FLV: &[u8] = b"FLV\x01\x01\0\0\0\x09";
    const ASF: &[u8] = &[
        0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA,
    ];
    const MPEG_PS: &[u8] = &[0x00, 0x00, 0x01, 0xBA, 0x44, 0x00, 0x04, 0x00];
    const TEXT: &[u8] = b"just some notes\n";

    /// Packets of `packet` bytes that each start with the sync byte, after
    /// `shift` bytes.
    fn transport_stream(shift: usize, packet: usize) -> Vec<u8> {
        let mut bytes = vec![0xFFu8; shift + 3 * packet];
        for n in 0..3 {
            bytes[shift + n * packet] = 0x47;
        }
        bytes
    }

    #[test]
    fn the_first_bytes_tell_what_a_file_is() {
        let mpeg_ts = transport_stream(0, 188);
        let m2ts = transport_stream(4, 192);
        let cases: &[(&str, &[u8], Content)] = &[
            ("png", PNG, Content::Image(ImageFormat::Png)),
            ("jpeg", JPEG, Content::Image(ImageFormat::Jpeg)),
            (
                "gif 89a",
                b"GIF89a\x80\x02",
                Content::Image(ImageFormat::Gif),
            ),
            (
                "gif 87a",
                b"GIF87a\x80\x02",
                Content::Image(ImageFormat::Gif),
            ),
            ("webp", WEBP_STILL, Content::Image(ImageFormat::WebP)),
            (
                "animated webp",
                WEBP_ANIMATED,
                Content::Image(ImageFormat::WebP),
            ),
            ("bmp", b"BM\x8a\x8c\x0a\0", Content::Image(ImageFormat::Bmp)),
            (
                "tiff",
                b"II*\0\x08\0\0\0",
                Content::Image(ImageFormat::Tiff),
            ),
            // An ISO media file like an MP4, but a picture.
            (
                "avif",
                b"\0\0\0\x1cftypavif\0\0\0\0",
                Content::Image(ImageFormat::Avif),
            ),
            // Pictures no decoder here reads. HEIF is an ISO media file too.
            (
                "heic",
                b"\0\0\0\x1cftypheix\0\0\0\0",
                Content::UnsupportedImage("HEIF"),
            ),
            (
                "heif",
                b"\0\0\0\x18ftypmif1\0\0\0\0",
                Content::UnsupportedImage("HEIF"),
            ),
            (
                "jpeg xl",
                &[0xFF, 0x0A, 0x38, 0x5B],
                Content::UnsupportedImage("JPEG XL"),
            ),
            (
                "jpeg xl in its container",
                b"\0\0\0\x0cJXL \r\n\x87\n\0\0\0\x14ftypjxl ",
                Content::UnsupportedImage("JPEG XL"),
            ),
            ("matroska", MATROSKA, Content::Video("Matroska or WebM")),
            ("mp4", MP4, Content::Video("MP4 or QuickTime")),
            (
                "mov",
                b"\0\0\0\x14ftypqt  \0\0\0\0",
                Content::Video("MP4 or QuickTime"),
            ),
            ("old mov", QUICKTIME_OLD, Content::Video("QuickTime")),
            ("avi", AVI, Content::Video("AVI")),
            ("ogg", OGG, Content::Video("Ogg")),
            ("flv", FLV, Content::Video("Flash Video")),
            ("asf", ASF, Content::Video("ASF or WMV")),
            ("mpeg ps", MPEG_PS, Content::Video("MPEG program stream")),
            ("mpeg ts", &mpeg_ts, Content::Video("MPEG transport stream")),
            ("m2ts", &m2ts, Content::Video("MPEG transport stream")),
            ("text", TEXT, Content::Unknown),
            ("nothing", b"", Content::Unknown),
            ("one byte", b"\x1a", Content::Unknown),
            // Signatures that are cut short, or only look alike.
            ("half a matroska signature", &[0x1A, 0x45], Content::Unknown),
            (
                "riff of another kind",
                b"RIFF\x24\0\0\0WAVEfmt ",
                Content::Unknown,
            ),
            (
                "a box that is not media",
                b"\0\0\0\x10junk\0\0\0\0",
                Content::Unknown,
            ),
            ("one sync byte", &mpeg_ts[..188], Content::Unknown),
            ("two sync bytes", &mpeg_ts[..376], Content::Unknown),
        ];
        for (name, header, expected) in cases {
            assert_eq!(sniff(header), *expected, "{name}");
        }
    }

    #[test]
    fn the_extension_is_a_hint_for_video_image_or_nothing() {
        let cases: &[(&str, Option<MediaKind>)] = &[
            ("clip.mp4", Some(MediaKind::Video)),
            ("clip.MKV", Some(MediaKind::Video)),
            ("clip.WebM", Some(MediaKind::Video)),
            ("my.holiday.2024.mov", Some(MediaKind::Video)),
            ("/srv/media.d/loop.m2ts", Some(MediaKind::Video)),
            ("still.png", Some(MediaKind::Image)),
            ("still.JPG", Some(MediaKind::Image)),
            ("still.tga", Some(MediaKind::Image)),
            // A GIF is a picture format; whether it moves is in its content.
            ("anim.gif", Some(MediaKind::Image)),
            ("clip.mp4.png", Some(MediaKind::Image)),
            ("archive.png.webm", Some(MediaKind::Video)),
            ("notes.txt", None),
            ("wallpaper", None),
            ("wallpaper.", None),
            ("", None),
            // `Path::extension` takes a leading dot as part of the name.
            (".mp4", None),
            ("/home/user/.mkv", None),
            ("/srv/videos.mp4/still", None),
        ];
        for (path, expected) in cases {
            assert_eq!(extension_hint(Path::new(path)), *expected, "{path:?}");
        }
        for extension in VIDEO_EXTENSIONS {
            assert_eq!(
                extension_hint(Path::new(&format!("clip.{extension}"))),
                Some(MediaKind::Video),
                "{extension}"
            );
        }
    }

    #[test]
    fn the_animation_bit_of_a_webp_is_read_from_its_header() {
        assert!(webp_is_animated(WEBP_ANIMATED));
        assert!(!webp_is_animated(WEBP_EXTENDED_STILL));
        assert!(!webp_is_animated(WEBP_STILL));
        assert!(!webp_is_animated(&WEBP_ANIMATED[..20]));
        assert!(!webp_is_animated(b""));
    }

    /// A unique path in the temp directory; the caller removes the file.
    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("veil-media-{}-{name}", std::process::id()))
    }

    /// Writes `content` to a file called `name` and returns what `detect` says.
    fn detect_file(name: &str, content: &[u8]) -> Result<MediaKind, VeilError> {
        let path = temp_path(name);
        std::fs::write(&path, content).expect("write the test file");
        let detected = detect(&path);
        std::fs::remove_file(&path).expect("remove the test file");
        detected
    }

    /// A GIF of `frames` frames, each of one colour.
    fn gif(frames: u8) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            for n in 0..frames {
                let picture = RgbaImage::from_pixel(4, 4, Rgba([n.wrapping_mul(60), 0, 0, 255]));
                encoder
                    .encode_frame(Frame::new(picture))
                    .expect("encode a GIF frame");
            }
        }
        bytes
    }

    #[test]
    fn the_content_decides_and_not_the_name() {
        let still = gif(1);
        let animated = gif(3);
        let cases: &[(&str, &[u8], MediaKind)] = &[
            ("a.png", PNG, MediaKind::Image),
            ("a.mkv", MATROSKA, MediaKind::Video),
            // The content decides, not the extension, and its mirror image.
            ("png-named.mp4", PNG, MediaKind::Image),
            ("jpeg-named.mkv", JPEG, MediaKind::Image),
            ("matroska-named.png", MATROSKA, MediaKind::Video),
            ("mp4-named.jpg", MP4, MediaKind::Video),
            ("png-without-extension", PNG, MediaKind::Image),
            ("mp4-without-extension", MP4, MediaKind::Video),
            ("png-named.txt", PNG, MediaKind::Image),
            // A GIF is an image, unless it moves.
            ("still.gif", &still, MediaKind::Image),
            ("animated.gif", &animated, MediaKind::Video),
            ("animated-gif-named.png", &animated, MediaKind::Video),
            ("still-gif-named.mp4", &still, MediaKind::Image),
            // WebP is shown by the image engine either way.
            ("still.webp", WEBP_STILL, MediaKind::Image),
            ("animated.webp", WEBP_ANIMATED, MediaKind::Image),
            // Content that is not recognised: the extension is asked.
            ("unknown.mp4", TEXT, MediaKind::Video),
            ("unknown.webm", TEXT, MediaKind::Video),
            ("unknown.png", TEXT, MediaKind::Image),
            ("unknown.tga", &[0, 0, 2, 0, 0, 0, 0, 0], MediaKind::Image),
        ];
        for (name, content, expected) in cases {
            let detected = detect_file(name, content);
            assert!(
                matches!(detected, Ok(kind) if kind == *expected),
                "{name}: {detected:?}, expected {expected:?}"
            );
        }
    }

    #[test]
    fn a_gif_that_breaks_off_after_its_first_frame_is_an_image() {
        let animated = gif(3);
        // The second frame starts where the still one ends (minus its trailer).
        let first_frame_ends = gif(1).len() - 1;
        let cut = &animated[..first_frame_ends + 8];
        let detected = detect_file("cut.gif", cut);
        assert!(matches!(detected, Ok(MediaKind::Image)), "{detected:?}");
        // And one that has nothing but its signature.
        let detected = detect_file("signature-only.gif", b"GIF89a");
        assert!(matches!(detected, Ok(MediaKind::Image)), "{detected:?}");
    }

    #[test]
    fn a_file_that_is_neither_is_refused_with_its_name() {
        for name in ["notes.txt", "notes", "notes.pdf"] {
            let detected = detect_file(name, TEXT);
            let Err(VeilError::UnknownMedia(path)) = &detected else {
                panic!("{name}: {detected:?}");
            };
            assert!(path.ends_with(name), "{path}");
        }
    }

    #[test]
    fn a_picture_that_cannot_be_decoded_is_refused_as_that_and_not_as_a_video() {
        let cases: &[(&str, &[u8], &str)] = &[
            ("photo.heic", b"\0\0\0\x1cftypheix\0\0\0\0", "HEIF"),
            // Its name does not make it a video.
            ("photo.mp4", b"\0\0\0\x1cftypheic\0\0\0\0", "HEIF"),
            ("photo.jxl", &[0xFF, 0x0A, 0x38, 0x5B], "JPEG XL"),
            ("photo", &[0xFF, 0x0A, 0x38, 0x5B], "JPEG XL"),
        ];
        for (name, content, expected) in cases {
            let detected = detect_file(name, content);
            let Err(VeilError::UnsupportedImage { path, format }) = &detected else {
                panic!("{name}: {detected:?}");
            };
            assert!(path.ends_with(name), "{path}");
            assert_eq!(format, expected, "{name}");
        }

        // AVIF is known to `image`, which says whether it can read it.
        let detected = detect_file("photo.avif", b"\0\0\0\x1cftypavif\0\0\0\0");
        if ImageFormat::Avif.reading_enabled() {
            assert!(matches!(detected, Ok(MediaKind::Image)), "{detected:?}");
        } else {
            assert!(
                matches!(&detected, Err(VeilError::UnsupportedImage { format, .. }) if format == "AVIF"),
                "{detected:?}"
            );
        }
    }

    #[test]
    fn an_empty_file_is_refused_whatever_its_name() {
        for name in ["empty.png", "empty.mp4", "empty"] {
            let detected = detect_file(name, b"");
            assert!(
                matches!(detected, Err(VeilError::EmptyFile(_))),
                "{name}: {detected:?}"
            );
        }
    }

    #[test]
    fn a_file_that_cannot_be_read_is_refused_with_the_reason() {
        use std::os::unix::fs::PermissionsExt;

        for (name, content) in [("locked.png", PNG), ("locked.mkv", MATROSKA)] {
            let path = temp_path(name);
            std::fs::write(&path, content).expect("write the test file");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
                .expect("take the permissions away");
            // Root reads the file all the same; there is nothing to test then.
            let readable_anyway = File::open(&path).is_ok();
            let detected = detect(&path);
            std::fs::remove_file(&path).expect("remove the test file");
            if readable_anyway {
                eprintln!("skipping: this user can read a file without permissions");
                return;
            }

            let Err(VeilError::Unreadable { path, source }) = &detected else {
                panic!("{name}: {detected:?}");
            };
            assert!(path.ends_with(name), "{path}");
            assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
        }
    }

    #[test]
    fn a_missing_path_and_a_directory_are_refused_before_anything_is_read() {
        let missing = detect(&temp_path("missing.png"));
        assert!(
            matches!(missing, Err(VeilError::PathResolution { .. })),
            "{missing:?}"
        );

        let directory = temp_path("directory.mp4");
        std::fs::create_dir_all(&directory).expect("create the directory");
        let detected = detect(&directory);
        std::fs::remove_dir(&directory).expect("remove the directory");
        assert!(
            matches!(detected, Err(VeilError::NotAFile(_))),
            "{detected:?}"
        );
    }

    #[test]
    fn a_file_shorter_than_the_header_is_read_whole() {
        // Three bytes are a complete JPEG signature.
        let detected = detect_file("tiny.bin", &[0xFF, 0xD8, 0xFF]);
        assert!(matches!(detected, Ok(MediaKind::Image)), "{detected:?}");
        // One byte is nothing, and has no extension to go by.
        let detected = detect_file("one-byte", &[0x1A]);
        assert!(
            matches!(detected, Err(VeilError::UnknownMedia(_))),
            "{detected:?}"
        );
    }
}
