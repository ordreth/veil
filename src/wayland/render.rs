//! The wallpaper of one surface: which engine shows it, and at which size and
//! scale.
//!
//! This is the part of the daemon's state that decides when an engine is
//! attached, resized, replaced and detached. It holds no Wayland objects: it is
//! told about the surface (`set_size`, `set_scale`, `release`) and is handed a
//! [`RenderTarget`] when there is something to draw. That keeps the engine
//! lifecycle testable with a mock engine and no compositor.
//!
//! # Replacing the wallpaper
//!
//! A new engine never takes the place of the current one before it has
//! something to show. An engine that draws at once (an image) is swapped in by
//! [`RenderState::switch`]. One that needs time (a video) is attached and kept
//! aside, while the surface goes on showing the current wallpaper. When it
//! reports its first frame, [`RenderState::started`] swaps it in; if it reports
//! a failure, it is given up and nothing has changed on screen.
//!
//! Engines that are replaced or given up are not dropped here. They are handed
//! out through [`RenderState::take_retired`], because dropping a video pipeline
//! takes long enough to be done off the event loop.
//!
//! # Pausing
//!
//! Whether the wallpaper is to play is part of this state too
//! ([`RenderState::set_paused`]): every engine of the surface is told, the one
//! that waits to replace the wallpaper and every later one included, and a
//! paused video is not watched for frames the compositor does not take.
//!
//! # Unseen
//!
//! A video whose surface the compositor does not draw (the session is locked,
//! the output is powered off) is released: [`RenderState::release_unseen`]
//! detaches it, so its pipeline is gone, and nothing is attached until
//! [`RenderState::seen`] says that the surface is drawn again. A wallpaper set
//! meanwhile is stored, as without a surface.

use crate::engine::{Attached, EngineSignal, RenderContext, RenderTarget, ReportSink, StreamInfo};
use crate::error::VeilError;
use crate::scaling::SurfaceScale;

pub use crate::engine::Engine;

/// What [`RenderState::switch`] did with the new engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switched {
    /// It is the wallpaper now, and on screen.
    Shown,
    /// It is attached and waits for its first frame; the previous wallpaper is
    /// still shown. [`RenderState::started`] ends this, one way or the other.
    Starting,
    /// There is no surface. It is the wallpaper, and will be shown on the next
    /// one.
    Stored,
}

/// What a start report led to, see [`RenderState::started`].
#[derive(Debug)]
pub enum Started {
    /// The engine that was starting has replaced the previous wallpaper.
    Switched,
    /// The engine that was to replace the wallpaper could not start, or could
    /// not take over. The previous wallpaper is untouched.
    RolledBack(VeilError),
    /// The wallpaper (there was none before it on this surface) has its first
    /// frame.
    Shown,
    /// The wallpaper could not start, and there is no other to show.
    Failed(VeilError),
    /// The report came from an engine that is no longer waited for.
    Ignored,
}

/// Where the wallpaper of a surface is (`veil status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// There is no configured surface.
    NoSurface,
    /// It is about to be attached, or attached and waiting for its first
    /// frame.
    Starting,
    /// It is on the surface.
    Shown,
    /// It was released because the compositor does not draw the surface
    /// (`release_unseen`), until `seen`.
    Unseen,
    /// It is not on the surface: its attach failed, or it stopped for good
    /// (`lost`). The next change of the surface tries again.
    Stopped,
}

pub struct RenderState {
    engine: Engine,
    /// Surface size in logical pixels, from the last applied configure. `None`
    /// while there is no configured surface to draw on.
    size: Option<(u32, u32)>,
    /// Scale the wallpaper is rendered at: buffers are `scale` times `size`.
    scale: SurfaceScale,
    /// Whether `engine` is attached to the current surface (its `attach`
    /// succeeded there).
    attached: bool,
    /// The serial of the start report `engine` still owes: it is attached, but
    /// its first frame has not come yet.
    engine_start: Option<u64>,
    /// The serial of the signal `engine` was given when it was attached to the
    /// current surface. Its later reports come with it.
    engine_serial: Option<u64>,
    /// An engine that will replace `engine` when it has started, and the serial
    /// of its start report. It is attached to the surface, which still shows
    /// `engine`.
    incoming: Option<(Engine, u64)>,
    /// Whether the size or the scale changed since the engine was last asked to
    /// draw. The drawing waits for `draw`, which runs once per batch of events:
    /// a compositor changes an output's scale and the surface size together, and
    /// drawing in between would composite a frame at a size that is never shown.
    dirty: bool,
    /// Engines that were replaced or given up, until the caller takes them.
    retired: Vec<Engine>,
    /// Whether the wallpaper is paused, because it cannot be seen.
    paused: bool,
    /// Whether the wallpaper was released because the compositor does not draw
    /// its surface; it is attached again when it does (`seen`).
    unseen: bool,
    /// The serial of the last start signal handed to an engine.
    last_serial: u64,
    /// Where the start signals deliver.
    report: ReportSink,
}

impl RenderState {
    /// Starts with `engine` and no surface. Nothing is drawn before `set_size`.
    /// Engines that need time to start report to `report`, from a thread of
    /// their own; the caller passes each report on to [`RenderState::started`].
    pub fn new(engine: Engine, report: ReportSink) -> Self {
        Self {
            engine,
            size: None,
            scale: SurfaceScale::ONE,
            attached: false,
            engine_start: None,
            engine_serial: None,
            incoming: None,
            dirty: false,
            retired: Vec::new(),
            paused: false,
            unseen: false,
            last_serial: 0,
            report,
        }
    }

    /// Surface size in logical pixels, or `None` while there is no configured
    /// surface.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.size
    }

    pub fn scale(&self) -> SurfaceScale {
        self.scale
    }

    /// Whether the engine is attached to the current surface. It may still be
    /// waiting for its first frame, see `is_starting`.
    pub fn is_attached(&self) -> bool {
        self.attached
    }

    /// Whether the wallpaper is attached but has not shown its first frame yet.
    pub fn is_starting(&self) -> bool {
        self.engine_start.is_some()
    }

    /// Whether an engine is waiting to replace the wallpaper. For the tests.
    #[cfg(test)]
    pub fn is_switching(&self) -> bool {
        self.incoming.is_some()
    }

    /// Whether `draw` has something to do.
    pub fn needs_draw(&self) -> bool {
        self.dirty
    }

    /// Whether a video plays on the surface: a dynamic engine is attached as
    /// the wallpaper, and is not paused. Only such a video is watched for frames
    /// the compositor does not take (`release_unseen`).
    pub fn plays_video(&self) -> bool {
        self.attached && self.engine.is_dynamic() && !self.paused
    }

    /// Whether a dynamic engine (a video) is attached to the surface: the
    /// wallpaper, or the one that waits to replace it, paused or not. A video
    /// shows its picture in a subsurface, and where that is placed takes effect
    /// only when the surface itself is committed.
    pub fn video_attached(&self) -> bool {
        (self.attached && self.engine.is_dynamic())
            || self
                .incoming
                .as_ref()
                .is_some_and(|(engine, _)| engine.is_dynamic())
    }

    /// Whether `serial` is the signal of an engine that is attached to the
    /// surface now: the wallpaper, or the one that waits to replace it. A
    /// report with another serial comes from an engine that was replaced, or
    /// from an earlier surface.
    pub fn attached_with(&self, serial: u64) -> bool {
        (self.attached && self.engine_serial == Some(serial))
            || self.incoming.as_ref().is_some_and(|(_, s)| *s == serial)
    }

    /// Whether the wallpaper changes by itself (a video), so that pausing it
    /// makes a difference.
    pub fn is_dynamic(&self) -> bool {
        self.engine.is_dynamic()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// What the compositor did with the frames of the wallpaper, while it is
    /// attached (see `WallpaperEngine::frame_counts`).
    pub fn frame_counts(&self) -> Option<crate::engine::FrameCounts> {
        self.attached.then(|| self.engine.frame_counts()).flatten()
    }

    /// Whether the wallpaper is released until its surface is drawn again.
    pub fn is_unseen(&self) -> bool {
        self.unseen
    }

    /// Where the wallpaper is.
    pub fn phase(&self) -> Phase {
        if self.size.is_none() {
            Phase::NoSurface
        } else if self.unseen {
            Phase::Unseen
        } else if self.is_starting() || (!self.attached && self.dirty) {
            Phase::Starting
        } else if self.attached {
            Phase::Shown
        } else {
            Phase::Stopped
        }
    }

    /// What the stream of the wallpaper is, while it is attached (see
    /// `WallpaperEngine::stream`).
    pub fn stream(&self) -> Option<StreamInfo> {
        self.attached.then(|| self.engine.stream()).flatten()
    }

    /// The compositor does not show the video on the surface (it takes none of
    /// its frames): a video that plays on it is detached, which ends its
    /// pipeline, and is not attached again before `seen`. Returns whether that
    /// happened. Only a video that has started is released: one that is still
    /// starting reports first, and an image costs nothing while it is shown.
    /// An engine that waits to replace it is left to start; if it takes over
    /// while the surface is not drawn, it is released the same way.
    pub fn release_unseen(&mut self) -> bool {
        if !self.attached || !self.engine.is_dynamic() || self.is_starting() {
            return false;
        }
        self.engine.detach();
        self.attached = false;
        self.engine_serial = None;
        self.unseen = true;
        true
    }

    /// The compositor draws the surface again (a frame callback of the surface
    /// was answered):
    /// a wallpaper released by `release_unseen` is attached at the next
    /// `draw`. Returns whether there was one.
    pub fn seen(&mut self) -> bool {
        if !self.unseen {
            return false;
        }
        self.unseen = false;
        self.dirty |= self.size.is_some() && !self.attached;
        true
    }

    /// Pauses the wallpaper (it cannot be seen) or lets it play again. The
    /// engine is told, and so is one that waits to replace it; engines that
    /// come later are told before they are attached. While the wallpaper is
    /// paused, `plays_video` is false, so it is not watched for frames the
    /// compositor does not take (`release_unseen`).
    pub fn set_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        tell_paused(&mut self.engine, paused);
        if let Some((engine, _)) = &mut self.incoming {
            tell_paused(engine, paused);
        }
    }

    /// The engines that were replaced or given up since the last call. Dropping
    /// them can take long (a video pipeline is shut down), and must be over
    /// before the surface they were attached to is destroyed.
    pub fn take_retired(&mut self) -> Vec<Engine> {
        std::mem::take(&mut self.retired)
    }

    /// Records the size of the surface, from a configure. The first size is what
    /// makes the surface drawable.
    pub fn set_size(&mut self, size: (u32, u32)) {
        if self.size != Some(size) {
            self.size = Some(size);
            self.dirty = true;
        }
    }

    /// Records the scale of the surface. It may be set before the size is known.
    pub fn set_scale(&mut self, scale: SurfaceScale) {
        if self.scale != scale {
            self.scale = scale;
            self.dirty |= self.size.is_some();
        }
    }

    /// Shows the wallpaper at the current size and scale: the engine is attached
    /// if it is not shown on this surface yet, and resized otherwise. An engine
    /// that waits to replace it follows the surface as well. `target` is `None`
    /// only while there is no surface, and then there is nothing to draw.
    ///
    /// A failure is not retried by itself (that would repeat on every batch of
    /// events). The next change of size or scale, or the next engine, tries again.
    pub fn draw(&mut self, target: Option<&mut dyn RenderTarget>) -> Result<(), VeilError> {
        self.dirty = false;
        let (Some(size), Some(target)) = (self.size, target) else {
            return Ok(());
        };
        let scale = self.scale;

        let incoming = match &mut self.incoming {
            Some((engine, _)) => engine.resize(&mut RenderContext {
                keep_previous: true,
                ..RenderContext::new(size, scale, &mut *target)
            }),
            None => Ok(()),
        };

        let current = if self.attached {
            self.engine
                .resize(&mut RenderContext::new(size, scale, target))
        } else if self.incoming.is_none() && !self.unseen {
            let serial = self.next_serial();
            let attached = self.engine.attach(&mut RenderContext {
                signal: EngineSignal::new(serial, self.report.clone()),
                ..RenderContext::new(size, scale, target)
            })?;
            self.attached = true;
            self.engine_start = (attached == Attached::Starting).then_some(serial);
            self.engine_serial = Some(serial);
            Ok(())
        } else {
            // The wallpaper could not be attached earlier, and its replacement
            // is on the way: there is nothing to try again for. Or it was
            // released because the surface is not drawn: it waits for `seen`.
            Ok(())
        };
        current.and(incoming)
    }

    /// Replaces the engine. With a surface (`target` is `Some`), the new engine is
    /// attached right away, at the current size and scale. If that fails, the
    /// previous engine is kept and the error is returned. If the new engine
    /// needs time to start, the previous one stays until `started` is told how
    /// that went. Without a surface (the output is unplugged, or its surface is
    /// not configured yet), the new engine is stored holding no resources, and
    /// the first `draw` on a surface shows it.
    ///
    /// An engine that was still waiting to replace the wallpaper is given up:
    /// the latest request wins.
    pub fn switch(
        &mut self,
        mut engine: Engine,
        target: Option<&mut dyn RenderTarget>,
    ) -> Result<Switched, VeilError> {
        if let Some((earlier, _)) = self.incoming.take() {
            self.retired.push(earlier);
        }
        // A new engine plays: it only needs to be told otherwise.
        if self.paused {
            engine.pause();
        }

        let (Some(size), Some(target)) = (self.size, target) else {
            engine.detach();
            self.replace_engine(engine, None);
            self.attached = false;
            self.dirty = self.size.is_some();
            return Ok(Switched::Stored);
        };
        if self.unseen {
            // Nobody would see it start: it is shown when the surface is drawn
            // again, as if there were no surface now.
            engine.detach();
            self.replace_engine(engine, None);
            self.attached = false;
            return Ok(Switched::Stored);
        }

        let serial = self.next_serial();
        let attached = engine.attach(&mut RenderContext {
            // What the surface shows must survive a start that fails.
            keep_previous: self.attached,
            signal: EngineSignal::new(serial, self.report.clone()),
            ..RenderContext::new(size, self.scale, target)
        });
        match attached {
            Ok(Attached::Shown) => {
                self.replace_engine(engine, Some(serial));
                self.attached = true;
                // Drawn at the current size and scale, so this also covers a
                // draw that was still pending.
                self.dirty = false;
                Ok(Switched::Shown)
            }
            Ok(Attached::Starting) => {
                self.incoming = Some((engine, serial));
                Ok(Switched::Starting)
            }
            Err(e) => {
                self.retired.push(engine);
                Err(e)
            }
        }
    }

    /// An engine has reported, through its start signal with `serial`, whether
    /// it has its first frame. If it is the one that waits to replace the
    /// wallpaper, it takes over now, or is given up. Reports of engines that are
    /// no longer waited for (they were replaced, or the surface is gone) are
    /// ignored.
    pub fn started(
        &mut self,
        serial: u64,
        result: Result<(), VeilError>,
        target: Option<&mut dyn RenderTarget>,
    ) -> Started {
        let (Some(size), Some(target)) = (self.size, target) else {
            return Started::Ignored;
        };
        let mut ctx = RenderContext::new(size, self.scale, target);

        if self.incoming.as_ref().is_some_and(|(_, s)| *s == serial) {
            let Some((mut engine, _)) = self.incoming.take() else {
                return Started::Ignored;
            };
            return match result.and_then(|()| engine.take_over(&mut ctx)) {
                Ok(()) => {
                    self.replace_engine(engine, Some(serial));
                    self.attached = true;
                    Started::Switched
                }
                Err(e) => {
                    self.retired.push(engine);
                    Started::RolledBack(e)
                }
            };
        }

        if self.engine_start == Some(serial) {
            self.engine_start = None;
            return match result.and_then(|()| self.engine.take_over(&mut ctx)) {
                Ok(()) => Started::Shown,
                Err(e) => {
                    // It holds a pipeline that shows nothing. A later change of
                    // the surface attaches it again, like after a failed attach.
                    self.engine.detach();
                    self.attached = false;
                    self.engine_serial = None;
                    Started::Failed(e)
                }
            };
        }

        Started::Ignored
    }

    /// An engine has reported, through its signal with `serial`, that it has
    /// stopped showing its wallpaper for good (a video that failed and could
    /// not be restarted). If it is the wallpaper of the surface, it is detached:
    /// it holds nothing any more, the surface keeps what the engine left under
    /// itself (the background colour of a video), and a later change of the
    /// surface attaches it again, like after a failed attach. Returns whether
    /// that happened; a report of an engine that is no longer the wallpaper is
    /// ignored.
    pub fn lost(&mut self, serial: u64) -> bool {
        if !self.attached || self.engine_serial != Some(serial) {
            return false;
        }
        self.engine.detach();
        self.attached = false;
        self.engine_start = None;
        self.engine_serial = None;
        true
    }

    /// The surface is going away (its output is gone, or the compositor closed
    /// it): every engine lets go of it, and nothing is drawn until the next
    /// surface reports its size. Must be called before the surface is destroyed.
    ///
    /// An engine that was waiting to replace the wallpaper becomes the
    /// wallpaper, as if it had been set while there was no surface. Returns
    /// whether that happened.
    pub fn release(&mut self) -> bool {
        self.engine.detach();
        let promoted = match self.incoming.take() {
            Some((mut engine, _)) => {
                engine.detach();
                self.replace_engine(engine, None);
                true
            }
            None => false,
        };
        self.attached = false;
        self.engine_start = None;
        self.engine_serial = None;
        self.size = None;
        self.dirty = false;
        // The next surface starts from scratch.
        self.unseen = false;
        promoted
    }

    /// Makes `engine` the wallpaper and retires the one before it.
    /// `serial` is that of the signal it was attached with, if it is attached.
    fn replace_engine(&mut self, engine: Engine, serial: Option<u64>) {
        let previous = std::mem::replace(&mut self.engine, engine);
        self.retired.push(previous);
        self.engine_start = None;
        self.engine_serial = serial;
    }

    fn next_serial(&mut self) -> u64 {
        self.last_serial += 1;
        self.last_serial
    }
}

fn tell_paused(engine: &mut Engine, paused: bool) {
    if paused {
        engine.pause();
    } else {
        engine.resume();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::image::ImageEngine;
    use crate::engine::testing::{FakeTarget, Shown};
    use crate::engine::{EngineReport, WallpaperEngine};
    use crate::scaling::Color;
    use std::sync::{Arc, Mutex};

    const HD: (u32, u32) = (1920, 1080);
    const ONE: SurfaceScale = SurfaceScale::ONE;

    /// A lifecycle call received by a [`MockEngine`], with the engine's name.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Call {
        Attach(&'static str, (u32, u32), SurfaceScale),
        /// The `attach` just recorded answered `Starting`. The flag is the
        /// `keep_previous` it was given.
        Starting(&'static str, bool),
        TakeOver(&'static str),
        Resize(&'static str, (u32, u32), SurfaceScale),
        Detach(&'static str),
        Pause(&'static str),
        Resume(&'static str),
        Drop(&'static str),
    }

    #[derive(Default)]
    struct Recorded {
        calls: Vec<Call>,
        /// The start signals the engines were given, by engine name.
        signals: Vec<(&'static str, EngineSignal)>,
        /// What reached the state's `ReportSink`.
        reports: Vec<(u64, EngineReport)>,
    }

    /// What the engines of a test did, in order, and their start reports.
    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Recorded>>);

    impl Log {
        fn recorded(&self) -> std::sync::MutexGuard<'_, Recorded> {
            self.0.lock().expect("log")
        }

        /// The calls since the last `take`.
        fn take(&self) -> Vec<Call> {
            std::mem::take(&mut self.recorded().calls)
        }

        /// A state whose first engine is `engine`, and whose start reports are
        /// collected here.
        fn state(&self, engine: Engine) -> RenderState {
            let log = self.clone();
            RenderState::new(
                engine,
                Arc::new(move |serial, report| log.recorded().reports.push((serial, report))),
            )
        }

        /// The last signal the engine `name` was given.
        fn signal(&self, name: &'static str) -> EngineSignal {
            let recorded = self.recorded();
            let (_, signal) = recorded
                .signals
                .iter()
                .rev()
                .find(|(engine, _)| *engine == name)
                .expect("the engine was given a signal");
            signal.clone()
        }

        /// The engine `name` reports how its start went, and the report is
        /// passed on to the state, as the daemon does.
        fn report(
            &self,
            state: &mut RenderState,
            name: &'static str,
            started: Result<(), VeilError>,
            target: &mut FakeTarget,
        ) -> Started {
            self.signal(name).started(started);
            // Taken out first: the state calls engines, which write to this log.
            let report = self.recorded().reports.pop().expect("a report");
            match report {
                (serial, EngineReport::Started(started)) => {
                    state.started(serial, started, Some(target))
                }
                (_, other) => panic!("expected a start report, got {other:?}"),
            }
        }

        /// The engine `name` reports that it has moved its picture. Returns the
        /// serial the report comes with.
        fn moved(&self, name: &'static str) -> u64 {
            self.signal(name).moved();
            match self.recorded().reports.pop() {
                Some((serial, EngineReport::Moved)) => serial,
                other => panic!("expected a move report, got {other:?}"),
            }
        }

        /// The engine `name` reports that it has stopped for good, and the
        /// report is passed on to the state. Returns what the state says.
        fn lose(&self, state: &mut RenderState, name: &'static str) -> bool {
            self.signal(name)
                .lost(VeilError::Gstreamer("it stopped".into()));
            // Taken out first: the state calls engines, which write to this log.
            let report = self.recorded().reports.pop().expect("a report");
            match report {
                (serial, EngineReport::Lost(_)) => state.lost(serial),
                (_, other) => panic!("expected a lost report, got {other:?}"),
            }
        }
    }

    /// An engine that records its lifecycle calls and paints its colour on the
    /// target, so a test can see which engine drew last.
    struct MockEngine {
        name: &'static str,
        log: Log,
        color: Color,
        dynamic: bool,
        /// How many of the next `attach` calls fail.
        failing_attaches: u32,
        /// Whether `attach` answers `Starting`, like a video.
        starts_later: bool,
        /// How many of the next `take_over` calls fail.
        failing_take_overs: u32,
    }

    impl MockEngine {
        fn new(name: &'static str, log: &Log) -> Self {
            Self {
                name,
                log: log.clone(),
                color: Color::default(),
                dynamic: false,
                failing_attaches: 0,
                starts_later: false,
                failing_take_overs: 0,
            }
        }

        fn boxed(name: &'static str, log: &Log) -> Engine {
            Box::new(Self::new(name, log))
        }

        /// An engine that starts later and paints `shade` once it has taken over.
        fn later(name: &'static str, log: &Log, shade: u8) -> Self {
            let mut engine = Self::new(name, log);
            engine.starts_later = true;
            engine.color = grey(shade);
            engine
        }

        fn record(&self, call: Call) {
            self.log.recorded().calls.push(call);
        }

        fn failure(&self) -> VeilError {
            VeilError::BufferCreation(format!("{} failed", self.name))
        }
    }

    impl WallpaperEngine for MockEngine {
        fn attach(&mut self, ctx: &mut RenderContext<'_>) -> Result<Attached, VeilError> {
            self.record(Call::Attach(self.name, ctx.size, ctx.scale));
            if self.failing_attaches > 0 {
                self.failing_attaches -= 1;
                return Err(self.failure());
            }
            self.log
                .recorded()
                .signals
                .push((self.name, ctx.signal.clone()));
            if !self.starts_later {
                ctx.target.fill(self.color)?;
                return Ok(Attached::Shown);
            }
            self.record(Call::Starting(self.name, ctx.keep_previous));
            // Like a video: a surface with nothing on it gets a background.
            if !ctx.keep_previous {
                ctx.target.fill(Color::default())?;
            }
            Ok(Attached::Starting)
        }

        fn take_over(&mut self, ctx: &mut RenderContext<'_>) -> Result<(), VeilError> {
            self.record(Call::TakeOver(self.name));
            if self.failing_take_overs > 0 {
                self.failing_take_overs -= 1;
                return Err(self.failure());
            }
            ctx.target.fill(self.color)
        }

        fn resize(&mut self, ctx: &mut RenderContext<'_>) -> Result<(), VeilError> {
            self.record(Call::Resize(self.name, ctx.size, ctx.scale));
            // An engine that still waits to take over must not draw.
            if ctx.keep_previous {
                return Ok(());
            }
            ctx.target.fill(self.color)
        }

        fn detach(&mut self) {
            self.record(Call::Detach(self.name));
        }

        fn pause(&mut self) {
            self.record(Call::Pause(self.name));
        }

        fn resume(&mut self) {
            self.record(Call::Resume(self.name));
        }

        fn is_dynamic(&self) -> bool {
            self.dynamic
        }
    }

    impl Drop for MockEngine {
        fn drop(&mut self) {
            self.record(Call::Drop(self.name));
        }
    }

    fn target() -> FakeTarget {
        FakeTarget::new(HD)
    }

    fn grey(shade: u8) -> Color {
        Color {
            r: shade,
            g: shade,
            b: shade,
        }
    }

    /// A state whose engine `a` is shown on a 1920x1080 surface at scale 1.
    fn shown(log: &Log) -> RenderState {
        let mut state = log.state(MockEngine::boxed("a", log));
        state.set_size(HD);
        state.draw(Some(&mut target())).expect("first draw");
        assert_eq!(log.take(), [Call::Attach("a", HD, ONE)]);
        state
    }

    /// Drops the engines the state has retired, and says which they were.
    fn dropped(state: &mut RenderState, log: &Log) -> Vec<Call> {
        let before = log.take();
        assert!(before.is_empty(), "calls before the drop: {before:?}");
        drop(state.take_retired());
        log.take()
    }

    #[test]
    fn nothing_is_drawn_before_the_surface_has_a_size() {
        let log = Log::default();
        let mut state = log.state(MockEngine::boxed("a", &log));
        assert_eq!(state.size(), None);
        assert!(!state.needs_draw());

        // The compositor may send the scale ahead of the first configure.
        state.set_scale(SurfaceScale::from_120ths(180));
        assert!(!state.needs_draw());
        state.draw(Some(&mut target())).expect("nothing to draw");
        assert!(log.take().is_empty());
        assert!(!state.is_attached());
    }

    #[test]
    fn first_size_attaches_once_at_the_scale_known_by_then() {
        let log = Log::default();
        let mut state = log.state(MockEngine::boxed("a", &log));
        let scale = SurfaceScale::from_120ths(180);
        state.set_scale(scale);
        state.set_size((1280, 720));
        assert!(state.needs_draw());

        let mut target = target();
        state.draw(Some(&mut target)).expect("draw");
        assert_eq!(log.take(), [Call::Attach("a", (1280, 720), scale)]);
        assert!(state.is_attached());
        assert!(!state.is_starting());
        assert!(!state.needs_draw());
        assert_eq!(target.shown.len(), 1);
    }

    #[test]
    fn size_and_scale_changing_together_are_drawn_once() {
        let log = Log::default();
        let mut state = shown(&log);

        // What a compositor sends when an output goes from 1x to 2x.
        let two = SurfaceScale::from_integer(2);
        state.set_scale(two);
        state.set_size((960, 540));
        assert!(state.needs_draw());
        state.draw(Some(&mut target())).expect("draw");
        assert_eq!(log.take(), [Call::Resize("a", (960, 540), two)]);
        assert!(!state.needs_draw());
    }

    #[test]
    fn unchanged_size_and_scale_need_no_draw() {
        let log = Log::default();
        let mut state = shown(&log);
        state.set_size(HD);
        state.set_scale(ONE);
        assert!(!state.needs_draw());
    }

    #[test]
    fn release_detaches_and_the_next_surface_attaches_again() {
        let log = Log::default();
        let mut state = shown(&log);
        state.set_scale(SurfaceScale::from_integer(2));

        assert!(!state.release());
        assert_eq!(log.take(), [Call::Detach("a")]);
        assert_eq!(state.size(), None);
        assert!(!state.is_attached());
        // The pending redraw died with the surface.
        assert!(!state.needs_draw());

        // The output is back, with the same size as before: that is still a new
        // surface, so the engine is attached, not resized.
        state.set_scale(ONE);
        state.set_size(HD);
        state.draw(Some(&mut target())).expect("draw");
        assert_eq!(log.take(), [Call::Attach("a", HD, ONE)]);
    }

    #[test]
    fn switch_attaches_the_new_engine_and_retires_the_old_one() {
        let log = Log::default();
        let mut state = shown(&log);
        // A redraw was still pending; the new engine is drawn at the new scale.
        let two = SurfaceScale::from_integer(2);
        state.set_scale(two);

        let mut target = target();
        let mut b = MockEngine::new("b", &log);
        b.color = grey(9);
        let switched = state.switch(Box::new(b), Some(&mut target));
        assert_eq!(switched.expect("switch"), Switched::Shown);
        // The old engine is handed out, not dropped: that is the caller's.
        assert_eq!(log.take(), [Call::Attach("b", HD, two)]);
        assert_eq!(dropped(&mut state, &log), [Call::Drop("a")]);
        assert_eq!(target.shown, [Shown::Fill(grey(9))]);
        assert!(state.is_attached());
        assert!(!state.is_switching());
        assert!(!state.needs_draw());

        // Later changes go to the new engine.
        state.set_size((1280, 720));
        state.draw(Some(&mut target)).expect("draw");
        assert_eq!(log.take(), [Call::Resize("b", (1280, 720), two)]);
    }

    #[test]
    fn an_engine_that_fails_to_attach_does_not_replace_the_wallpaper() {
        let log = Log::default();
        let mut state = shown(&log);

        let mut b = MockEngine::new("b", &log);
        b.failing_attaches = 1;
        let mut target = target();
        let err = state
            .switch(Box::new(b), Some(&mut target))
            .expect_err("b fails to attach");
        assert_eq!(err.to_string(), "Failed to create SHM buffer: b failed");
        assert_eq!(log.take(), [Call::Attach("b", HD, ONE)]);
        assert_eq!(dropped(&mut state, &log), [Call::Drop("b")]);
        assert!(target.shown.is_empty());

        // `a` is still the wallpaper and still attached.
        assert!(state.is_attached());
        state.set_size((1280, 720));
        state.draw(Some(&mut target)).expect("draw");
        assert_eq!(log.take(), [Call::Resize("a", (1280, 720), ONE)]);
    }

    #[test]
    fn an_engine_that_starts_later_replaces_the_wallpaper_when_it_has_started() {
        let log = Log::default();
        let mut state = shown(&log);
        let mut target = target();

        let b = MockEngine::later("b", &log, 9);
        let switched = state.switch(Box::new(b), Some(&mut target));
        assert_eq!(switched.expect("switch"), Switched::Starting);
        // It is told to leave the surface alone, and does.
        assert_eq!(
            log.take(),
            [Call::Attach("b", HD, ONE), Call::Starting("b", true)]
        );
        assert!(target.shown.is_empty());
        assert!(state.is_switching());
        assert!(state.take_retired().is_empty());

        // Meanwhile `a` is the wallpaper, and both follow the surface. Only `a`
        // draws.
        state.set_size((1280, 720));
        state.draw(Some(&mut target)).expect("draw");
        assert_eq!(
            log.take(),
            [
                Call::Resize("b", (1280, 720), ONE),
                Call::Resize("a", (1280, 720), ONE)
            ]
        );
        assert_eq!(target.shown, [Shown::Fill(Color::default())]);
        target.shown.clear();

        // The first frame of `b` is there.
        let started = log.report(&mut state, "b", Ok(()), &mut target);
        assert!(matches!(started, Started::Switched), "{started:?}");
        assert_eq!(log.take(), [Call::TakeOver("b")]);
        assert_eq!(target.shown, [Shown::Fill(grey(9))]);
        assert_eq!(dropped(&mut state, &log), [Call::Drop("a")]);
        assert!(!state.is_switching());
        assert!(state.is_attached());

        state.set_size(HD);
        state.draw(Some(&mut target)).expect("draw");
        assert_eq!(log.take(), [Call::Resize("b", HD, ONE)]);
    }

    #[test]
    fn an_engine_that_fails_to_start_does_not_replace_the_wallpaper() {
        let log = Log::default();
        let mut state = shown(&log);
        let mut target = target();

        let b = MockEngine::later("b", &log, 9);
        state
            .switch(Box::new(b), Some(&mut target))
            .expect("switch");
        log.take();

        let broken = Err(VeilError::Gstreamer("no video in this file".into()));
        let started = log.report(&mut state, "b", broken, &mut target);
        let Started::RolledBack(error) = started else {
            panic!("expected a rollback, got {started:?}");
        };
        assert_eq!(
            error.to_string(),
            "GStreamer pipeline error: no video in this file"
        );
        // It never took over, and nothing was drawn at any point.
        assert!(target.shown.is_empty());
        assert_eq!(dropped(&mut state, &log), [Call::Drop("b")]);
        assert!(!state.is_switching());

        // `a` is still the wallpaper and still attached.
        assert!(state.is_attached());
        state.set_size((1280, 720));
        state.draw(Some(&mut target)).expect("draw");
        assert_eq!(log.take(), [Call::Resize("a", (1280, 720), ONE)]);
    }

    #[test]
    fn an_engine_that_fails_to_take_over_does_not_replace_the_wallpaper() {
        let log = Log::default();
        let mut state = shown(&log);
        let mut target = target();

        let mut b = MockEngine::later("b", &log, 9);
        b.failing_take_overs = 1;
        state
            .switch(Box::new(b), Some(&mut target))
            .expect("switch");
        log.take();

        let started = log.report(&mut state, "b", Ok(()), &mut target);
        assert!(matches!(started, Started::RolledBack(_)), "{started:?}");
        assert_eq!(log.take(), [Call::TakeOver("b")]);
        assert!(target.shown.is_empty());
        assert_eq!(dropped(&mut state, &log), [Call::Drop("b")]);
        assert!(state.is_attached());
        assert!(!state.is_switching());
    }

    #[test]
    fn a_newer_switch_replaces_one_that_is_still_starting() {
        let log = Log::default();
        let mut state = shown(&log);
        let mut target = target();

        state
            .switch(Box::new(MockEngine::later("b", &log, 1)), Some(&mut target))
            .expect("switch to b");
        let switched = state.switch(Box::new(MockEngine::later("c", &log, 2)), Some(&mut target));
        assert_eq!(switched.expect("switch to c"), Switched::Starting);
        log.take();
        // `b` is given up before it was ever shown; `a` is still on screen.
        assert_eq!(dropped(&mut state, &log), [Call::Drop("b")]);

        // A report of `b` that was already on its way changes nothing.
        let late = log.report(&mut state, "b", Ok(()), &mut target);
        assert!(matches!(late, Started::Ignored), "{late:?}");
        assert!(log.take().is_empty());

        let started = log.report(&mut state, "c", Ok(()), &mut target);
        assert!(matches!(started, Started::Switched), "{started:?}");
        assert_eq!(target.shown, [Shown::Fill(grey(2))]);
        log.take();
        assert_eq!(dropped(&mut state, &log), [Call::Drop("a")]);

        // An engine that is shown at once replaces a starting one as well.
        state
            .switch(Box::new(MockEngine::later("d", &log, 3)), Some(&mut target))
            .expect("switch to d");
        let switched = state.switch(MockEngine::boxed("e", &log), Some(&mut target));
        assert_eq!(switched.expect("switch to e"), Switched::Shown);
        log.take();
        assert_eq!(
            dropped(&mut state, &log),
            [Call::Drop("d"), Call::Drop("c")]
        );
        assert!(!state.is_switching());
    }

    #[test]
    fn the_latest_switch_wins_even_if_it_fails() {
        let log = Log::default();
        let mut state = shown(&log);
        let mut target = target();

        state
            .switch(Box::new(MockEngine::later("b", &log, 1)), Some(&mut target))
            .expect("switch to b");
        let mut c = MockEngine::new("c", &log);
        c.failing_attaches = 1;
        state
            .switch(Box::new(c), Some(&mut target))
            .expect_err("c fails to attach");
        log.take();

        // `b` was replaced by `c` when `c` arrived, like a request that is
        // replaced while it waits to be loaded. What stays is `a`.
        assert_eq!(
            dropped(&mut state, &log),
            [Call::Drop("b"), Call::Drop("c")]
        );
        assert!(!state.is_switching());
        assert!(state.is_attached());
        assert!(target.shown.is_empty());
    }

    #[test]
    fn releasing_the_surface_makes_a_starting_engine_the_wallpaper() {
        let log = Log::default();
        let mut state = shown(&log);
        let mut target = target();
        state
            .switch(Box::new(MockEngine::later("b", &log, 9)), Some(&mut target))
            .expect("switch");
        log.take();

        // The output is unplugged before `b` has its first frame. Both let go
        // of the surface, and `b` is what the next surface will show.
        assert!(state.release());
        assert_eq!(log.take(), [Call::Detach("a"), Call::Detach("b")]);
        assert_eq!(dropped(&mut state, &log), [Call::Drop("a")]);
        assert!(!state.is_switching());
        assert!(!state.is_attached());

        // Its report belongs to a surface that is gone.
        let late = log.report(&mut state, "b", Ok(()), &mut target);
        assert!(matches!(late, Started::Ignored), "{late:?}");

        state.set_size(HD);
        state.draw(Some(&mut target)).expect("draw");
        assert_eq!(
            log.take(),
            [Call::Attach("b", HD, ONE), Call::Starting("b", false)]
        );
    }

    #[test]
    fn the_first_wallpaper_of_a_surface_may_start_later() {
        let log = Log::default();
        let mut state = log.state(Box::new(MockEngine::later("a", &log, 9)));
        let mut target = target();
        state.set_size(HD);
        state.draw(Some(&mut target)).expect("draw");

        // There is nothing on the surface to keep, so it may draw at once.
        assert_eq!(
            log.take(),
            [Call::Attach("a", HD, ONE), Call::Starting("a", false)]
        );
        assert_eq!(target.shown, [Shown::Fill(Color::default())]);
        assert!(state.is_attached());
        assert!(state.is_starting());
        assert!(!state.needs_draw());

        // It follows the surface while it starts.
        state.set_size((1280, 720));
        state.draw(Some(&mut target)).expect("draw");
        assert_eq!(log.take(), [Call::Resize("a", (1280, 720), ONE)]);

        let started = log.report(&mut state, "a", Ok(()), &mut target);
        assert!(matches!(started, Started::Shown), "{started:?}");
        assert_eq!(log.take(), [Call::TakeOver("a")]);
        assert!(!state.is_starting());
        assert!(state.take_retired().is_empty());

        // A second report of the same start is not waited for.
        let again = log.report(&mut state, "a", Ok(()), &mut target);
        assert!(matches!(again, Started::Ignored), "{again:?}");
        assert!(log.take().is_empty());
    }

    #[test]
    fn the_first_wallpaper_that_cannot_start_is_detached_and_tried_again_later() {
        let log = Log::default();
        let mut a = MockEngine::later("a", &log, 9);
        a.dynamic = true;
        let mut state = log.state(Box::new(a));
        let mut target = target();
        state.set_size(HD);
        state.draw(Some(&mut target)).expect("draw");
        assert!(state.plays_video());
        log.take();

        let broken = Err(VeilError::Gstreamer("no video in this file".into()));
        let started = log.report(&mut state, "a", broken, &mut target);
        assert!(matches!(started, Started::Failed(_)), "{started:?}");
        // It gives up what it holds, and is not watched any more.
        assert_eq!(log.take(), [Call::Detach("a")]);
        assert!(!state.is_attached());
        assert!(!state.is_starting());
        assert!(!state.plays_video());
        // Not on every batch of events.
        assert!(!state.needs_draw());

        // The next change of the surface tries again, with an attach.
        state.set_size((1280, 720));
        state.draw(Some(&mut target)).expect("draw");
        assert_eq!(
            log.take(),
            [
                Call::Attach("a", (1280, 720), ONE),
                Call::Starting("a", false)
            ]
        );
        assert!(state.is_starting());
    }

    #[test]
    fn a_wallpaper_that_is_still_starting_can_be_replaced() {
        let log = Log::default();
        let mut state = log.state(Box::new(MockEngine::later("a", &log, 1)));
        let mut target = target();
        state.set_size(HD);
        state.draw(Some(&mut target)).expect("draw");
        log.take();

        // `veil set` right after the daemon started with a video.
        let switched = state.switch(MockEngine::boxed("b", &log), Some(&mut target));
        assert_eq!(switched.expect("switch"), Switched::Shown);
        assert_eq!(log.take(), [Call::Attach("b", HD, ONE)]);
        assert_eq!(dropped(&mut state, &log), [Call::Drop("a")]);
        assert!(!state.is_starting());

        let late = log.report(&mut state, "a", Ok(()), &mut target);
        assert!(matches!(late, Started::Ignored), "{late:?}");
        assert!(log.take().is_empty());
    }

    #[test]
    fn a_report_with_an_unknown_serial_is_ignored() {
        let log = Log::default();
        let mut state = shown(&log);
        let mut target = target();
        let started = state.started(77, Ok(()), Some(&mut target));
        assert!(matches!(started, Started::Ignored), "{started:?}");
        // So is any report while there is no surface.
        state.release();
        let started = state.started(1, Ok(()), None);
        assert!(matches!(started, Started::Ignored), "{started:?}");
    }

    #[test]
    fn a_wallpaper_that_stops_for_good_is_detached_and_tried_again_later() {
        let log = Log::default();
        let mut a = MockEngine::later("a", &log, 9);
        a.dynamic = true;
        let mut state = log.state(Box::new(a));
        let mut target = target();
        state.set_size(HD);
        state.draw(Some(&mut target)).expect("draw");
        log.report(&mut state, "a", Ok(()), &mut target);
        assert!(state.plays_video());
        log.take();

        // It played, failed, and its restarts failed too.
        assert!(log.lose(&mut state, "a"));
        // It gives up what it holds, and is not watched any more.
        assert_eq!(log.take(), [Call::Detach("a")]);
        assert!(!state.is_attached());
        assert!(!state.plays_video());
        // Nothing is tried again by itself, and nothing is retired: it is still
        // the wallpaper that was asked for.
        assert!(!state.needs_draw());
        assert!(state.take_retired().is_empty());

        // The same report again means nothing any more.
        assert!(!log.lose(&mut state, "a"));
        assert!(log.take().is_empty());

        // The next change of the surface tries again, with an attach.
        state.set_size((1280, 720));
        state.draw(Some(&mut target)).expect("draw");
        assert_eq!(
            log.take(),
            [
                Call::Attach("a", (1280, 720), ONE),
                Call::Starting("a", false)
            ]
        );
    }

    #[test]
    fn a_lost_report_of_an_engine_that_is_not_the_wallpaper_is_ignored() {
        let log = Log::default();
        let mut state = shown(&log);
        let mut target = target();

        // `a` was replaced; what its threads still report is of no interest.
        state
            .switch(MockEngine::boxed("b", &log), Some(&mut target))
            .expect("switch");
        log.take();
        assert!(!log.lose(&mut state, "a"));
        assert!(log.take().is_empty());
        assert!(state.is_attached());

        // An engine that still waits to take over is not the wallpaper either.
        state
            .switch(Box::new(MockEngine::later("c", &log, 9)), Some(&mut target))
            .expect("switch");
        log.take();
        assert!(!log.lose(&mut state, "c"));
        assert!(log.take().is_empty());
        assert!(state.is_switching());

        // And after the surface is gone nothing is attached that could be lost.
        state.release();
        log.take();
        assert!(!log.lose(&mut state, "c"));
        assert!(log.take().is_empty());
    }

    #[test]
    fn an_engine_that_was_shown_at_once_can_be_lost_too() {
        let log = Log::default();
        let mut state = shown(&log);
        assert!(log.lose(&mut state, "a"));
        assert_eq!(log.take(), [Call::Detach("a")]);
        assert!(!state.is_attached());
    }

    #[test]
    fn switch_without_a_surface_stores_the_engine_detached() {
        let log = Log::default();
        let mut state = shown(&log);
        state.release();
        log.take();

        // `veil set` while the output is unplugged.
        let switched = state.switch(MockEngine::boxed("b", &log), None);
        assert_eq!(switched.expect("accepted"), Switched::Stored);
        assert_eq!(log.take(), [Call::Detach("b")]);
        assert_eq!(dropped(&mut state, &log), [Call::Drop("a")]);
        assert!(!state.is_attached());
        assert!(!state.needs_draw());

        // It is shown when the output returns.
        state.set_size(HD);
        state.draw(Some(&mut target())).expect("draw");
        assert_eq!(log.take(), [Call::Attach("b", HD, ONE)]);
    }

    #[test]
    fn switch_before_the_first_configure_stores_the_engine_detached() {
        let log = Log::default();
        let mut state = log.state(MockEngine::boxed("a", &log));

        // There is a surface, but it has no size yet, so nothing can be drawn.
        let switched = state.switch(MockEngine::boxed("b", &log), Some(&mut target()));
        assert_eq!(switched.expect("accepted"), Switched::Stored);
        assert_eq!(log.take(), [Call::Detach("b")]);
        assert_eq!(dropped(&mut state, &log), [Call::Drop("a")]);

        state.set_size(HD);
        state.draw(Some(&mut target())).expect("draw");
        assert_eq!(log.take(), [Call::Attach("b", HD, ONE)]);
    }

    #[test]
    fn failed_attach_is_not_repeated_until_something_changes() {
        let log = Log::default();
        let mut a = MockEngine::new("a", &log);
        a.failing_attaches = 1;
        let mut state = log.state(Box::new(a));
        state.set_size(HD);

        state
            .draw(Some(&mut target()))
            .expect_err("the first attach fails");
        assert_eq!(log.take(), [Call::Attach("a", HD, ONE)]);
        assert!(!state.is_attached());
        // No retry on every batch of events.
        assert!(!state.needs_draw());

        // The next change tries again, and attaches: the engine was never shown
        // on this surface, so it must not get a `resize`.
        state.set_size((1280, 720));
        state.draw(Some(&mut target())).expect("second attempt");
        assert_eq!(log.take(), [Call::Attach("a", (1280, 720), ONE)]);
        assert!(state.is_attached());
    }

    #[test]
    fn only_a_shown_dynamic_engine_plays_video() {
        let log = Log::default();
        let mut video = MockEngine::new("video", &log);
        video.dynamic = true;
        let mut state = log.state(Box::new(video));
        // Not attached yet: there is nothing to watch.
        assert!(!state.plays_video());

        state.set_size(HD);
        state.draw(Some(&mut target())).expect("draw");
        assert!(state.plays_video());

        // An image plays nothing; a video plays again.
        state
            .switch(MockEngine::boxed("image", &log), Some(&mut target()))
            .expect("switch");
        assert!(!state.plays_video());
        let mut video = MockEngine::later("video", &log, 9);
        video.dynamic = true;
        let mut target = target();
        state
            .switch(Box::new(video), Some(&mut target))
            .expect("switch");
        // Not while the image is still what is shown.
        assert!(!state.plays_video());
        log.report(&mut state, "video", Ok(()), &mut target);
        assert!(state.plays_video());

        state.release();
        assert!(!state.plays_video());
    }

    #[test]
    fn a_dynamic_engine_that_failed_to_attach_plays_no_video() {
        let log = Log::default();
        let mut video = MockEngine::new("video", &log);
        video.dynamic = true;
        video.failing_attaches = 1;
        let mut state = log.state(Box::new(video));
        state.set_size(HD);
        state.draw(Some(&mut target())).expect_err("attach fails");
        assert!(!state.plays_video());
    }

    #[test]
    fn a_move_counts_for_the_videos_attached_to_the_surface() {
        let log = Log::default();
        let mut state = log.state(video("a", &log));
        state.set_size(HD);
        let mut target = target();
        state.draw(Some(&mut target)).expect("draw");
        let a = log.moved("a");
        assert!(state.video_attached());
        assert!(state.attached_with(a));

        // A video that starts later is attached next to it, and counts too.
        let mut b = MockEngine::later("b", &log, 9);
        b.dynamic = true;
        state
            .switch(Box::new(b), Some(&mut target))
            .expect("switch");
        let b = log.moved("b");
        assert!(state.attached_with(a));
        assert!(state.attached_with(b));

        // Once it has taken over, the one it replaced does not.
        log.report(&mut state, "b", Ok(()), &mut target);
        assert!(!state.attached_with(a));
        assert!(state.attached_with(b));

        // Paused, it is still on the surface, but it does not play.
        state.set_paused(true);
        assert!(state.video_attached());
        assert!(!state.plays_video());

        // An image draws on the surface itself.
        state
            .switch(MockEngine::boxed("c", &log), Some(&mut target))
            .expect("switch");
        assert!(!state.video_attached());
        assert!(!state.attached_with(b));

        // A video that starts over an image is attached already.
        let mut d = MockEngine::later("d", &log, 9);
        d.dynamic = true;
        state
            .switch(Box::new(d), Some(&mut target))
            .expect("switch");
        let d = log.moved("d");
        assert!(state.video_attached());
        assert!(state.attached_with(d));

        // Without a surface nothing is attached.
        state.release();
        assert!(!state.video_attached());
        assert!(!state.attached_with(d));
    }

    /// A video-like engine that is shown at once.
    fn video(name: &'static str, log: &Log) -> Engine {
        let mut engine = MockEngine::new(name, log);
        engine.dynamic = true;
        Box::new(engine)
    }

    #[test]
    fn a_paused_wallpaper_is_told_once_and_plays_no_video() {
        let log = Log::default();
        let mut state = log.state(video("a", &log));
        state.set_size(HD);
        state.draw(Some(&mut target())).expect("draw");
        log.take();
        assert!(state.plays_video());

        state.set_paused(true);
        assert_eq!(log.take(), [Call::Pause("a")]);
        assert!(state.is_paused());
        assert!(!state.plays_video());
        // Nothing to say twice.
        state.set_paused(true);
        assert!(log.take().is_empty());

        // A paused wallpaper still follows its surface.
        state.set_size((1280, 720));
        state.draw(Some(&mut target())).expect("draw");
        assert_eq!(log.take(), [Call::Resize("a", (1280, 720), ONE)]);

        state.set_paused(false);
        assert_eq!(log.take(), [Call::Resume("a")]);
        assert!(state.plays_video());
    }

    #[test]
    fn engines_that_come_while_paused_are_told_before_they_are_attached() {
        let log = Log::default();
        let mut state = shown(&log);
        state.set_paused(true);
        assert_eq!(log.take(), [Call::Pause("a")]);

        // Shown at once.
        state
            .switch(video("b", &log), Some(&mut target()))
            .expect("switch");
        assert_eq!(log.take(), [Call::Pause("b"), Call::Attach("b", HD, ONE)]);
        assert!(!state.plays_video());

        // Starting later.
        let mut c = MockEngine::later("c", &log, 9);
        c.dynamic = true;
        let mut target = target();
        state
            .switch(Box::new(c), Some(&mut target))
            .expect("switch");
        assert_eq!(
            log.take(),
            [
                Call::Pause("c"),
                Call::Attach("c", HD, ONE),
                Call::Starting("c", true)
            ]
        );
        log.report(&mut state, "c", Ok(()), &mut target);
        assert_eq!(log.take(), [Call::TakeOver("c")]);
        assert!(!state.plays_video());

        // Without a surface: stored, and told all the same.
        state.release();
        log.take();
        state.switch(video("d", &log), None).expect("stored");
        assert_eq!(log.take(), [Call::Pause("d"), Call::Detach("d")]);
        drop(state.take_retired());
        log.take();

        // A new surface: the stored engine is attached, paused.
        state.set_size(HD);
        state.draw(Some(&mut target)).expect("draw");
        assert_eq!(log.take(), [Call::Attach("d", HD, ONE)]);
        assert!(state.is_paused());
        assert!(!state.plays_video());
        state.set_paused(false);
        assert_eq!(log.take(), [Call::Resume("d")]);
        assert!(state.plays_video());
    }

    #[test]
    fn an_engine_that_waits_to_take_over_is_paused_and_resumed_too() {
        let log = Log::default();
        let mut state = shown(&log);
        let mut b = MockEngine::later("b", &log, 9);
        b.dynamic = true;
        let mut target = target();
        state
            .switch(Box::new(b), Some(&mut target))
            .expect("switch");
        log.take();

        state.set_paused(true);
        assert_eq!(log.take(), [Call::Pause("a"), Call::Pause("b")]);
        log.report(&mut state, "b", Ok(()), &mut target);
        assert_eq!(log.take(), [Call::TakeOver("b")]);
        assert!(!state.plays_video());

        state.set_paused(false);
        assert_eq!(log.take(), [Call::Resume("b")]);
        assert!(state.plays_video());
    }

    #[test]
    fn the_phase_follows_the_wallpaper() {
        let log = Log::default();
        let mut a = MockEngine::later("a", &log, 9);
        a.dynamic = true;
        let mut state = log.state(Box::new(a));
        let mut target = target();
        assert_eq!(state.phase(), Phase::NoSurface);
        state.set_size(HD);
        // Drawn with the next batch of events.
        assert_eq!(state.phase(), Phase::Starting);
        state.draw(Some(&mut target)).expect("draw");
        assert_eq!(state.phase(), Phase::Starting);
        log.report(&mut state, "a", Ok(()), &mut target);
        assert_eq!(state.phase(), Phase::Shown);
        assert!(state.release_unseen());
        assert_eq!(state.phase(), Phase::Unseen);
        assert!(state.seen());
        assert_eq!(state.phase(), Phase::Starting);
        state.draw(Some(&mut target)).expect("draw");
        log.report(&mut state, "a", Ok(()), &mut target);
        assert_eq!(state.phase(), Phase::Shown);
        assert!(log.lose(&mut state, "a"));
        assert_eq!(state.phase(), Phase::Stopped);
        state.release();
        assert_eq!(state.phase(), Phase::NoSurface);
        // A mock engine has no stream to report.
        assert_eq!(state.stream(), None);
    }

    #[test]
    fn an_unseen_video_is_released_and_attached_again_when_seen() {
        let log = Log::default();
        let mut state = log.state(video("a", &log));
        state.set_size(HD);
        state.draw(Some(&mut target())).expect("draw");
        log.take();

        assert!(state.release_unseen());
        assert_eq!(log.take(), [Call::Detach("a")]);
        assert!(state.is_unseen());
        assert!(!state.is_attached());
        assert!(!state.plays_video());
        // Once.
        assert!(!state.release_unseen());

        // A configure meanwhile does not attach it: nobody would see it.
        state.set_size((1280, 720));
        state.draw(Some(&mut target())).expect("draw");
        assert!(log.take().is_empty());

        assert!(state.seen());
        assert!(!state.is_unseen());
        assert!(state.needs_draw());
        state.draw(Some(&mut target())).expect("draw");
        assert_eq!(log.take(), [Call::Attach("a", (1280, 720), ONE)]);
        assert!(state.plays_video());
        // Seen again: nothing more to do.
        assert!(!state.seen());
        assert!(!state.needs_draw());
    }

    #[test]
    fn only_a_video_that_has_started_is_released() {
        let log = Log::default();
        // An image costs nothing while it is not drawn.
        let mut state = shown(&log);
        assert!(!state.release_unseen());
        assert!(log.take().is_empty());
        assert!(!state.is_unseen());

        // A video that still waits for its first frame reports first.
        let mut a = MockEngine::later("v", &log, 9);
        a.dynamic = true;
        let mut state = log.state(Box::new(a));
        let mut target = target();
        state.set_size(HD);
        state.draw(Some(&mut target)).expect("draw");
        log.take();
        assert!(state.is_starting());
        assert!(!state.release_unseen());
        log.report(&mut state, "v", Ok(()), &mut target);
        log.take();
        assert!(state.release_unseen());
        assert_eq!(log.take(), [Call::Detach("v")]);

        // Not attached at all: nothing to release.
        let mut state = log.state(video("w", &log));
        assert!(!state.release_unseen());
    }

    #[test]
    fn a_wallpaper_set_while_unseen_is_stored_until_seen() {
        let log = Log::default();
        let mut state = log.state(video("a", &log));
        state.set_size(HD);
        state.draw(Some(&mut target())).expect("draw");
        state.release_unseen();
        log.take();

        let switched = state.switch(video("b", &log), Some(&mut target()));
        assert_eq!(switched.expect("stored"), Switched::Stored);
        assert_eq!(log.take(), [Call::Detach("b")]);
        assert_eq!(dropped(&mut state, &log), [Call::Drop("a")]);
        assert!(!state.needs_draw());

        state.seen();
        state.draw(Some(&mut target())).expect("draw");
        assert_eq!(log.take(), [Call::Attach("b", HD, ONE)]);
    }

    #[test]
    fn an_engine_that_takes_over_while_unseen_is_watched_like_any_other() {
        let log = Log::default();
        let mut state = log.state(video("a", &log));
        let mut target = target();
        state.set_size(HD);
        state.draw(Some(&mut target)).expect("draw");
        // A video starts to replace it, and then the screen is locked.
        let mut b = MockEngine::later("b", &log, 9);
        b.dynamic = true;
        state
            .switch(Box::new(b), Some(&mut target))
            .expect("switch");
        assert!(state.release_unseen());
        log.take();

        log.report(&mut state, "b", Ok(()), &mut target);
        assert_eq!(log.take(), [Call::TakeOver("b")]);
        assert_eq!(dropped(&mut state, &log), [Call::Drop("a")]);
        // It plays where nobody sees it, until the next look.
        assert!(state.plays_video());
        assert!(state.release_unseen());
        assert_eq!(log.take(), [Call::Detach("b")]);
        state.seen();
        state.draw(Some(&mut target)).expect("draw");
        // Its surface shows nothing worth keeping: it may draw at once.
        assert_eq!(
            log.take(),
            [Call::Attach("b", HD, ONE), Call::Starting("b", false)]
        );
    }

    #[test]
    fn a_new_surface_forgets_that_the_old_one_was_unseen() {
        let log = Log::default();
        let mut state = log.state(video("a", &log));
        state.set_size(HD);
        state.draw(Some(&mut target())).expect("draw");
        state.release_unseen();
        state.release();
        assert!(!state.is_unseen());
        log.take();
        state.set_size(HD);
        state.draw(Some(&mut target())).expect("draw");
        assert_eq!(log.take(), [Call::Attach("a", HD, ONE)]);
    }

    #[test]
    fn a_new_state_plays() {
        let log = Log::default();
        let mut state = log.state(video("a", &log));
        assert!(!state.is_paused());
        state.set_paused(false);
        assert!(
            log.take().is_empty(),
            "a new engine plays without being told"
        );
    }

    #[test]
    fn real_image_engine_goes_through_the_same_lifecycle() {
        // The solid background (`veil daemon` without `--input`) from the first
        // configure, over a scale change, to an unplugged output and back.
        let color = Color { r: 1, g: 2, b: 3 };
        let log = Log::default();
        let mut state = log.state(Box::new(ImageEngine::solid(color)));
        let mut target = target();

        state.set_size(HD);
        state.draw(Some(&mut target)).expect("first draw");
        assert!(!state.is_starting());
        state.set_scale(SurfaceScale::from_integer(2));
        state.draw(Some(&mut target)).expect("redraw");
        state.release();
        state.set_size(HD);
        state
            .draw(Some(&mut target))
            .expect("draw on the new surface");

        assert_eq!(target.shown, vec![Shown::Fill(color); 3]);
        assert!(!state.plays_video());
    }
}
