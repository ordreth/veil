//! Dropping engines off the event loop.
//!
//! An engine that has been replaced still holds what it showed its wallpaper
//! with. For a video that is a pipeline, and shutting one down takes tens of
//! milliseconds: too long for the event loop. Such an engine is dropped on a
//! thread of its own, which ends with that.
//!
//! Two things depend on when it is gone. A client may be waiting to hear that
//! its wallpaper is on screen, and an image is only visible once the video in
//! front of it has been removed. And a video sink holds raw pointers to the
//! surface, so the surface must not be destroyed while one is shut down:
//! [`Retirements::wait_for`] is called with the surface's owner before that.
//!
//! This is generic over the owner (`K`, the output whose surface the engines
//! were attached to) and the client (`C`, who to answer), so it is tested
//! without a socket.

use std::thread::JoinHandle;

use super::render::Engine;

/// Name of the threads that drop engines, as `/proc/<pid>/task/*/comm` shows it.
pub const THREAD_NAME: &str = "veil-teardown";

/// The engines that are being dropped right now.
pub struct Retirements<K, C> {
    running: Vec<Retirement<K, C>>,
    last_id: u64,
}

struct Retirement<K, C> {
    id: u64,
    /// The surface the engines were attached to.
    owner: K,
    thread: JoinHandle<()>,
    /// Who is told when the engines are gone.
    client: Option<C>,
}

impl<K, C> Default for Retirements<K, C> {
    fn default() -> Self {
        Self {
            running: Vec::new(),
            last_id: 0,
        }
    }
}

impl<K: PartialEq, C> Retirements<K, C> {
    /// Drops `engines`, which were attached to the surface of `owner`, and
    /// returns the client that can be answered at once.
    ///
    /// If one of them is dynamic, they are dropped on a new thread. `client` is
    /// then kept, and `None` is returned: when the thread is done it calls
    /// `done` with an id, and [`Retirements::finished`] gives the client back
    /// for that id. Otherwise (static engines hold nothing that takes time)
    /// they are dropped here and `client` is returned.
    ///
    /// The thread inherits the signal mask of the caller, like every thread
    /// that is started from the event loop thread.
    pub fn retire(
        &mut self,
        owner: K,
        engines: Vec<Engine>,
        client: Option<C>,
        done: impl FnOnce(u64) + Send + 'static,
    ) -> Option<C> {
        if !engines.iter().any(|engine| engine.is_dynamic()) {
            return client;
        }

        self.last_id += 1;
        let id = self.last_id;
        let spawned = std::thread::Builder::new()
            .name(THREAD_NAME.into())
            .spawn(move || {
                drop(engines);
                done(id);
            });
        match spawned {
            Ok(thread) => {
                self.running.push(Retirement {
                    id,
                    owner,
                    thread,
                    client,
                });
                None
            }
            Err(e) => {
                // The engines went down with the closure, on this thread.
                tracing::warn!("cannot start a thread to shut an engine down: {e}");
                client
            }
        }
    }

    /// The thread that announced `id` is done. Returns the client that waited
    /// for it, if any. An id that is not (or no longer) known gives `None`.
    pub fn finished(&mut self, id: u64) -> Option<C> {
        let index = self.running.iter().position(|r| r.id == id)?;
        Self::join(self.running.remove(index))
    }

    /// Waits until every engine that was attached to the surface of `owner`
    /// is gone, and returns the clients that waited for that, oldest first.
    /// Engines of other surfaces go on being dropped meanwhile.
    pub fn wait_for(&mut self, owner: &K) -> Vec<C> {
        let (theirs, others) = std::mem::take(&mut self.running)
            .into_iter()
            .partition(|r| &r.owner == owner);
        self.running = others;
        Self::join_all(theirs)
    }

    /// Waits until every engine that is being dropped is gone, and returns the
    /// clients that waited for that, oldest first.
    pub fn wait(&mut self) -> Vec<C> {
        Self::join_all(std::mem::take(&mut self.running))
    }

    /// Whether no engine is being dropped. For the tests.
    #[cfg(test)]
    pub fn is_idle(&self) -> bool {
        self.running.is_empty()
    }

    fn join_all(mut running: Vec<Retirement<K, C>>) -> Vec<C> {
        running.sort_by_key(|r| r.id);
        running.into_iter().filter_map(Self::join).collect()
    }

    fn join(retirement: Retirement<K, C>) -> Option<C> {
        if retirement.thread.join().is_err() {
            tracing::error!("a thread that shut an engine down panicked");
        }
        retirement.client
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Attached, RenderContext, WallpaperEngine};
    use crate::error::VeilError;
    use std::sync::mpsc;
    use std::time::Duration;

    /// What a [`Slow`] engine tells about its drop.
    #[derive(Debug, PartialEq, Eq)]
    enum Event {
        /// The drop has begun, on a thread with this name.
        Dropping(&'static str, Option<String>),
        Dropped(&'static str),
    }

    /// An engine whose drop takes as long as the test wants: it waits for a
    /// message on `go` (or for the sender to be gone).
    struct Slow {
        name: &'static str,
        dynamic: bool,
        events: mpsc::Sender<Event>,
        go: Option<mpsc::Receiver<()>>,
    }

    impl WallpaperEngine for Slow {
        fn attach(&mut self, _: &mut RenderContext<'_>) -> Result<Attached, VeilError> {
            Ok(Attached::Shown)
        }
        fn resize(&mut self, _: &mut RenderContext<'_>) -> Result<(), VeilError> {
            Ok(())
        }
        fn detach(&mut self) {}
        fn is_dynamic(&self) -> bool {
            self.dynamic
        }
    }

    impl Drop for Slow {
        fn drop(&mut self) {
            let thread = std::thread::current().name().map(String::from);
            let _ = self.events.send(Event::Dropping(self.name, thread));
            if let Some(go) = &self.go {
                let _ = go.recv();
            }
            let _ = self.events.send(Event::Dropped(self.name));
        }
    }

    struct Fixture {
        retirements: Retirements<&'static str, u32>,
        events: mpsc::Receiver<Event>,
        sender: mpsc::Sender<Event>,
        /// The ids the threads announced when they were done.
        done: mpsc::Receiver<u64>,
        announce: mpsc::Sender<u64>,
    }

    impl Fixture {
        fn new() -> Self {
            let (sender, events) = mpsc::channel();
            let (announce, done) = mpsc::channel();
            Self {
                retirements: Retirements::default(),
                events,
                sender,
                done,
                announce,
            }
        }

        /// An engine, and the handle that lets its drop finish.
        fn engine(&self, name: &'static str, dynamic: bool) -> (Engine, mpsc::Sender<()>) {
            let (release, go) = mpsc::channel();
            let engine = Slow {
                name,
                dynamic,
                events: self.sender.clone(),
                go: Some(go),
            };
            (Box::new(engine), release)
        }

        fn retire(&mut self, engines: Vec<Engine>, client: Option<u32>) -> Option<u32> {
            self.retire_for("DP-3", engines, client)
        }

        /// Retires engines that were attached to the surface of `owner`.
        fn retire_for(
            &mut self,
            owner: &'static str,
            engines: Vec<Engine>,
            client: Option<u32>,
        ) -> Option<u32> {
            let announce = self.announce.clone();
            self.retirements.retire(owner, engines, client, move |id| {
                let _ = announce.send(id);
            })
        }

        fn event(&self) -> Event {
            self.events
                .recv_timeout(Duration::from_secs(20))
                .expect("an event")
        }

        fn done(&self) -> u64 {
            self.done
                .recv_timeout(Duration::from_secs(20))
                .expect("a thread announces that it is done")
        }
    }

    #[test]
    fn static_engines_are_dropped_at_once_and_their_client_is_not_kept() {
        let mut fixture = Fixture::new();
        let (image, release) = fixture.engine("image", false);
        // Nothing to wait for: the drop must not block on this thread either.
        drop(release);

        assert_eq!(fixture.retire(vec![image], Some(7)), Some(7));
        let this = std::thread::current().name().map(String::from);
        assert_eq!(fixture.event(), Event::Dropping("image", this));
        assert_eq!(fixture.event(), Event::Dropped("image"));
        assert!(fixture.retirements.is_idle());

        // No engines at all is the same.
        assert_eq!(fixture.retire(Vec::new(), Some(8)), Some(8));
        assert_eq!(fixture.retire(Vec::new(), None), None);
        assert!(fixture.retirements.is_idle());
    }

    #[test]
    fn a_dynamic_engine_is_dropped_on_its_own_thread_and_the_client_waits_for_it() {
        let mut fixture = Fixture::new();
        let (video, release) = fixture.engine("video", true);

        // The call returns while the engine is still being dropped.
        assert_eq!(fixture.retire(vec![video], Some(7)), None);
        assert_eq!(
            fixture.event(),
            Event::Dropping("video", Some(THREAD_NAME.to_string()))
        );
        assert!(!fixture.retirements.is_idle());
        assert!(fixture.done.try_recv().is_err());

        release.send(()).expect("let the drop finish");
        assert_eq!(fixture.event(), Event::Dropped("video"));
        let id = fixture.done();
        assert_eq!(fixture.retirements.finished(id), Some(7));
        assert!(fixture.retirements.is_idle());
        // Announced once, known once.
        assert_eq!(fixture.retirements.finished(id), None);
        assert_eq!(fixture.retirements.finished(id + 1), None);
    }

    #[test]
    fn engines_retired_together_go_down_on_one_thread() {
        let mut fixture = Fixture::new();
        let (image, release_image) = fixture.engine("image", false);
        let (video, release_video) = fixture.engine("video", true);
        drop((release_image, release_video));

        assert_eq!(fixture.retire(vec![image, video], None), None);
        let thread = Some(THREAD_NAME.to_string());
        assert_eq!(fixture.event(), Event::Dropping("image", thread.clone()));
        assert_eq!(fixture.event(), Event::Dropped("image"));
        assert_eq!(fixture.event(), Event::Dropping("video", thread));
        assert_eq!(fixture.event(), Event::Dropped("video"));
        // Nobody waited, but the thread is still accounted for until it is done.
        let id = fixture.done();
        assert_eq!(fixture.retirements.finished(id), None);
        assert!(fixture.retirements.is_idle());
    }

    #[test]
    fn wait_returns_when_every_engine_is_gone() {
        let mut fixture = Fixture::new();
        let (first, release_first) = fixture.engine("first", true);
        let (second, release_second) = fixture.engine("second", true);
        assert_eq!(fixture.retire(vec![first], Some(1)), None);
        assert_eq!(fixture.retire(vec![second], Some(2)), None);

        // Let the drops finish a little later, from another thread.
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let _ = release_second.send(());
            let _ = release_first.send(());
        });
        assert_eq!(fixture.retirements.wait(), [1, 2]);
        assert!(fixture.retirements.is_idle());
        releaser.join().expect("releaser");

        // Both drops are over by now.
        let dropped: Vec<Event> = fixture.events.try_iter().collect();
        assert!(dropped.contains(&Event::Dropped("first")), "{dropped:?}");
        assert!(dropped.contains(&Event::Dropped("second")), "{dropped:?}");
        // Their announcements come late, and mean nothing any more.
        for id in fixture.done.try_iter() {
            assert_eq!(fixture.retirements.finished(id), None);
        }
        assert!(fixture.retirements.wait().is_empty());
    }

    #[test]
    fn waiting_for_one_surface_leaves_the_engines_of_the_others_alone() {
        let mut fixture = Fixture::new();
        let (left, release_left) = fixture.engine("left", true);
        let (right, release_right) = fixture.engine("right", true);
        let (again, release_again) = fixture.engine("again", true);
        assert_eq!(fixture.retire_for("DP-3", vec![left], Some(1)), None);
        assert_eq!(fixture.retire_for("HDMI-A-1", vec![right], Some(2)), None);
        assert_eq!(fixture.retire_for("DP-3", vec![again], Some(3)), None);

        // The engines of DP-3 may go; the one of HDMI-A-1 is held back, and
        // waiting for DP-3 must not wait for it.
        release_left.send(()).expect("let `left` go");
        release_again.send(()).expect("let `again` go");
        assert_eq!(fixture.retirements.wait_for(&"DP-3"), [1, 3]);
        assert!(!fixture.retirements.is_idle());
        assert!(fixture.retirements.wait_for(&"DP-3").is_empty());

        release_right.send(()).expect("let `right` go");
        assert_eq!(fixture.retirements.wait_for(&"HDMI-A-1"), [2]);
        assert!(fixture.retirements.is_idle());
    }
}
