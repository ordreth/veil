//! What the compositor takes in dmabufs (`zwp_linux_dmabuf_v1`): the format
//! and modifier pairs a video plane can show a decoder's frames in, as they
//! are, without converting them on the CPU (`plane.rs`).
//!
//! The global is bound twice. `DmabufState` (smithay-client-toolkit) reads
//! the formats: the modifier events of version 3, or the feedback of version
//! 4 and later, whose format table it maps. veil's own binding makes the
//! planes' buffers, whose events carry user data of veil's and are handled on
//! the planes' own queues.
//!
//! A YCbCr frame (NV12, mostly) is turned into RGB by the compositor, which
//! has to know how its colours are coded: BT.709 for most HD video, BT.601 for
//! SD. `wp_color_representation_v1` tells it. A compositor without it assumes
//! BT.601 with the limited range (the default of EGL's dmabuf import), and
//! shows other videos with wrong colours: a plane then takes dmabufs coded
//! that way only.

use std::sync::{Arc, Mutex};

use smithay_client_toolkit::{
    delegate_dmabuf,
    dmabuf::{DmabufFeedback, DmabufHandler, DmabufState},
    reexports::protocols::wp::{
        color_representation::v1::client::{
            wp_color_representation_manager_v1::{self, WpColorRepresentationManagerV1},
            wp_color_representation_surface_v1::{
                Coefficients, Range, WpColorRepresentationSurfaceV1,
            },
        },
        linux_dmabuf::zv1::client::{
            zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
            zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
            zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        },
    },
};
use wayland_client::{
    delegate_noop, globals::GlobalList, protocol::wl_buffer::WlBuffer, Connection, Dispatch,
    QueueHandle, WEnum,
};

use super::state::AppState;
use crate::engine::{ColorCoding, ColorMatrix, DrmFormat};

/// `DRM_FORMAT_MOD_INVALID`: a layout only the driver knows, which a dmabuf
/// of a decoder never has (GStreamer's caps cannot even say it).
const MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// The compositor's `zwp_linux_dmabuf_v1`, if it has one.
pub struct Dmabuf {
    state: DmabufState,
    /// veil's own binding, for the planes' buffers. `None` without the global.
    global: Option<ZwpLinuxDmabufV1>,
    /// The default feedback (version 4 and later), kept for its events.
    feedback: Option<ZwpLinuxDmabufFeedbackV1>,
    /// What the latest feedback said.
    formats: Arc<[DrmFormat]>,
    /// `wp_color_representation_manager_v1`, if the compositor has it.
    colors: Option<WpColorRepresentationManagerV1>,
    /// The codings it can be told.
    codings: Vec<ColorCoding>,
    /// Formats it advertised and then refused to import: no plane offers
    /// them again.
    refused: Arc<Mutex<Vec<DrmFormat>>>,
}

/// What a video plane shows dmabufs with.
pub struct PlaneDmabuf {
    /// veil's binding of `zwp_linux_dmabuf_v1`.
    pub global: ZwpLinuxDmabufV1,
    /// The format and modifier pairs the compositor takes.
    pub formats: Arc<[DrmFormat]>,
    /// How to tell the compositor how a frame's colours are coded, and which
    /// codings it can be told; `None` if it cannot be told any.
    pub colors: Option<(WpColorRepresentationManagerV1, Arc<[ColorCoding]>)>,
    /// Formats the compositor refused to import, for every plane.
    pub refused: Arc<Mutex<Vec<DrmFormat>>>,
}

impl Dmabuf {
    /// Binds the global, version 3 to 5, if the compositor has it.
    pub fn bind(globals: &GlobalList, qh: &QueueHandle<AppState>) -> Self {
        let state = DmabufState::new(globals, qh);
        let feedback = state.get_default_feedback(qh).ok();
        let global = state
            .version()
            .and_then(|version| globals.bind(qh, 3..=version, ()).ok());
        let colors = globals.bind(qh, 1..=1, ()).ok();
        if global.is_some() && colors.is_none() {
            tracing::debug!(
                "compositor has no wp_color_representation_manager_v1; only videos coded as \
                 BT.601 go to it in dmabufs"
            );
        }
        match state.version() {
            Some(version) => {
                tracing::debug!("compositor has zwp_linux_dmabuf_v1 version {version}")
            }
            None => tracing::info!(
                "compositor has no zwp_linux_dmabuf_v1; video frames are converted on the CPU"
            ),
        }
        Self {
            state,
            global,
            feedback,
            formats: Arc::from([]),
            colors,
            codings: Vec::new(),
            refused: Arc::default(),
        }
    }

    /// What a plane made now shows dmabufs with; `None` if it cannot.
    pub fn for_plane(&self) -> Option<PlaneDmabuf> {
        let global = self.global.clone()?;
        let formats = if self.feedback.is_some() {
            Arc::clone(&self.formats)
        } else {
            usable(self.state.modifiers().iter().map(|format| DrmFormat {
                fourcc: format.format,
                modifier: format.modifier,
            }))
        };
        let colors = self
            .colors
            .clone()
            .map(|manager| (manager, Arc::from(self.codings.as_slice())));
        (!formats.is_empty()).then_some(PlaneDmabuf {
            global,
            formats,
            colors,
            refused: Arc::clone(&self.refused),
        })
    }
}

/// The protocol's name of a coding.
pub fn coefficients_and_range(coding: ColorCoding) -> (Coefficients, Range) {
    let coefficients = match coding.matrix {
        ColorMatrix::Bt601 => Coefficients::Bt601,
        ColorMatrix::Bt709 => Coefficients::Bt709,
        ColorMatrix::Bt2020 => Coefficients::Bt2020,
        ColorMatrix::Smpte240m => Coefficients::Smpte240,
        ColorMatrix::Fcc => Coefficients::Fcc,
    };
    let range = if coding.full_range {
        Range::Full
    } else {
        Range::Limited
    };
    (coefficients, range)
}

/// The coding the protocol names, if it is one of a YCbCr video.
fn coding(coefficients: Coefficients, range: Range) -> Option<ColorCoding> {
    let matrix = match coefficients {
        Coefficients::Bt601 => ColorMatrix::Bt601,
        Coefficients::Bt709 => ColorMatrix::Bt709,
        Coefficients::Bt2020 => ColorMatrix::Bt2020,
        Coefficients::Smpte240 => ColorMatrix::Smpte240m,
        Coefficients::Fcc => ColorMatrix::Fcc,
        _ => return None,
    };
    let full_range = match range {
        Range::Full => true,
        Range::Limited => false,
        _ => return None,
    };
    Some(ColorCoding { matrix, full_range })
}

impl Dispatch<WpColorRepresentationManagerV1, ()> for AppState {
    fn event(
        state: &mut Self,
        _: &WpColorRepresentationManagerV1,
        event: wp_color_representation_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_color_representation_manager_v1::Event::SupportedCoefficientsAndRanges {
                coefficients: WEnum::Value(coefficients),
                range: WEnum::Value(range),
            } => {
                if let Some(coding) = coding(coefficients, range) {
                    if !state.dmabuf.codings.contains(&coding) {
                        state.dmabuf.codings.push(coding);
                    }
                }
            }
            wp_color_representation_manager_v1::Event::Done => tracing::debug!(
                "compositor can be told how the colours of video frames are coded: {:?}",
                state.dmabuf.codings
            ),
            _ => {}
        }
    }
}

/// The formats of `formats` that a decoder's dmabuf can have, each once, in
/// their order.
fn usable(formats: impl IntoIterator<Item = DrmFormat>) -> Arc<[DrmFormat]> {
    let mut usable: Vec<DrmFormat> = Vec::new();
    for format in formats {
        if format.modifier != MOD_INVALID && !usable.contains(&format) {
            usable.push(format);
        }
    }
    usable.into()
}

impl DmabufHandler for AppState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf.state
    }

    fn dmabuf_feedback(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpLinuxDmabufFeedbackV1,
        feedback: DmabufFeedback,
    ) {
        let table = feedback.format_table();
        let formats = usable(feedback.tranches().iter().flat_map(|tranche| {
            tranche.formats.iter().filter_map(|&index| {
                table.get(usize::from(index)).map(|format| DrmFormat {
                    fourcc: format.format,
                    modifier: format.modifier,
                })
            })
        }));
        tracing::debug!(
            "compositor takes dmabufs in {} formats (main device {:#x})",
            formats.len(),
            feedback.main_device()
        );
        self.dmabuf.formats = formats;
    }

    // Buffers are made with veil's own binding (`plane.rs`), never through
    // `DmabufState`: these are not called.
    fn created(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        params: &ZwpLinuxBufferParamsV1,
        buffer: WlBuffer,
    ) {
        buffer.destroy();
        params.destroy();
    }

    fn failed(&mut self, _: &Connection, _: &QueueHandle<Self>, params: &ZwpLinuxBufferParamsV1) {
        params.destroy();
    }

    fn released(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlBuffer) {}
}

delegate_dmabuf!(AppState);
// veil's own binding: its format and modifier events (version 3) are read by
// `DmabufState` from its binding already.
delegate_noop!(AppState: ignore ZwpLinuxDmabufV1);
delegate_noop!(AppState: WpColorRepresentationSurfaceV1);

#[cfg(test)]
mod tests {
    use super::*;

    fn format(fourcc: u32, modifier: u64) -> DrmFormat {
        DrmFormat { fourcc, modifier }
    }

    #[test]
    fn codings_are_named_as_the_protocol_names_them() {
        for matrix in [
            ColorMatrix::Bt601,
            ColorMatrix::Bt709,
            ColorMatrix::Bt2020,
            ColorMatrix::Smpte240m,
            ColorMatrix::Fcc,
        ] {
            for full_range in [false, true] {
                let named = ColorCoding { matrix, full_range };
                let (coefficients, range) = coefficients_and_range(named);
                assert_eq!(coding(coefficients, range), Some(named));
            }
        }
        // RGB and the BT.2100 forms are not codings of a video's YCbCr.
        assert_eq!(coding(Coefficients::Identity, Range::Full), None);
        assert_eq!(coding(Coefficients::Ictcp, Range::Limited), None);
    }

    #[test]
    fn invalid_modifiers_and_repeated_formats_are_left_out() {
        let formats = usable([
            format(1, 0),
            format(1, MOD_INVALID),
            format(2, 7),
            format(1, 0),
        ]);
        assert_eq!(*formats, [format(1, 0), format(2, 7)]);
    }
}
