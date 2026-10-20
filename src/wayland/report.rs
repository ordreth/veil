//! What `veil status` and `veil outputs` are answered with: the daemon's
//! state, as the types of `ipc::report`.

use super::render::Phase;
use super::screen::Screen;
use super::state::AppState;
use crate::ipc::report::{
    Kind, OutputInfo, OutputList, OutputStatus, Size, State, Status, VideoInfo, WallpaperInfo,
};
use crate::persist::Wallpaper;

impl AppState {
    /// The answer to `status`.
    pub(super) fn status(&self) -> Status {
        Status {
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            uptime_secs: self.started.elapsed().as_secs(),
            pause_on: self.visibility.pause_on(),
            default: self.wallpapers.default.as_ref().map(wallpaper_info),
            outputs: self
                .screens
                .iter()
                .map(|screen| self.output_status(screen))
                .collect(),
        }
    }

    fn output_status(&self, screen: &Screen) -> OutputStatus {
        let render = &screen.render;
        let connected = screen.surface.is_some();
        let dynamic = render.is_dynamic();
        let state = match (connected, render.phase()) {
            (false, _) => State::Disconnected,
            (true, Phase::NoSurface | Phase::Starting) => State::Starting,
            (true, Phase::Shown) if !dynamic => State::Shown,
            (true, Phase::Shown) if render.is_paused() => State::Paused,
            (true, Phase::Shown) => State::Playing,
            (true, Phase::Unseen) => State::Released,
            (true, Phase::Stopped) => State::Stopped,
        };
        let kind = match (dynamic, &screen.shows.input) {
            (true, _) => Kind::Video,
            (false, None) => Kind::Color,
            (false, Some(_)) => Kind::Image,
        };
        let size = render
            .size()
            .filter(|_| connected)
            .map(|(width, height)| Size { width, height });
        let video = render.stream().map(|stream| {
            let counts = render.frame_counts();
            VideoInfo {
                size: stream.size.map(|(width, height)| Size { width, height }),
                fps: stream
                    .frame_rate
                    .map(|(numer, denom)| f64::from(numer) / f64::from(denom)),
                rendered: counts.map(|counts| counts.rendered),
                dropped: counts.map(|counts| counts.dropped),
            }
        });
        OutputStatus {
            name: screen.key.name().map(String::from),
            connected,
            wallpaper: wallpaper_info(&screen.shows),
            kind,
            state,
            paused_by_user: screen.held,
            hidden: self.is_hidden(&screen.key),
            size,
            scale: size.map(|_| render.scale().as_f64()),
            video,
            next: screen
                .starting
                .as_ref()
                .map(|starting| wallpaper_info(&starting.record.wallpaper)),
        }
    }

    /// The answer to `outputs`: every output the compositor has announced,
    /// then those the daemon keeps a wallpaper for or waits for (`--output`)
    /// that are not connected.
    pub(super) fn output_list(&self) -> OutputList {
        let mut outputs: Vec<OutputInfo> = self
            .output_state
            .outputs()
            .filter_map(|output| self.output_state.info(&output))
            .map(|info| OutputInfo {
                wallpaper: self.binding.wants(info.name.as_deref()),
                name: info.name,
                description: info.description,
                connected: true,
            })
            .collect();
        let away = self
            .screens
            .iter()
            .filter_map(|screen| screen.key.name())
            .chain(self.binding.requested().iter().map(String::as_str));
        for name in away {
            if !outputs.iter().any(|o| o.name.as_deref() == Some(name)) {
                outputs.push(OutputInfo {
                    name: Some(name.to_string()),
                    description: None,
                    connected: false,
                    wallpaper: true,
                });
            }
        }
        OutputList { outputs }
    }
}

fn wallpaper_info(wallpaper: &Wallpaper) -> WallpaperInfo {
    WallpaperInfo {
        input: wallpaper.input.clone(),
        mode: wallpaper.options.mode,
        bg_color: wallpaper.options.bg_color,
    }
}
