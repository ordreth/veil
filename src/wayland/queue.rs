//! Which `veil set` request is being loaded, and which ones come next.
//!
//! Wallpapers are loaded off the event loop, one at a time, and a load cannot
//! be interrupted. Requests may arrive faster than that. Each request is for a
//! set of outputs (its targets: one output, or every output), and the rule is
//! that the latest request wins **on each output**: a new request takes its
//! outputs away from every earlier request that is not shown yet. An earlier
//! request that has no output left is replaced. Its client is told at once, and
//! it is never shown, not even for a moment; if it was still waiting, it is not
//! even loaded. One that keeps some of its outputs is still shown on those.
//!
//! This is the bookkeeping only. It is generic over the output (`T`), the job
//! (`J`, what to load) and the client (`C`, who to answer), so it holds no
//! engine, no thread and no socket, and is tested without them.

/// What to do after [`SetQueue::submit`].
#[derive(Debug, PartialEq, Eq)]
pub struct Submitted<T, J, C> {
    /// Start loading this now, for these outputs: the worker was idle.
    pub start: Option<(Vec<T>, J)>,
    /// These earlier requests have no output left and will not be shown. Tell
    /// their clients.
    pub superseded: Vec<C>,
}

/// What to do after [`SetQueue::finished`].
#[derive(Debug, PartialEq, Eq)]
pub struct Finished<T, J, C> {
    /// Who asked for what has just been loaded, and the outputs it is still
    /// wanted on (some of those it was loaded for may have been taken by newer
    /// requests). `None` means that newer requests have taken every output:
    /// the result is stale and must be dropped.
    pub wanted: Option<(C, Vec<T>)>,
    /// Start loading this now, for these outputs.
    pub next: Option<(Vec<T>, J)>,
}

#[derive(Debug)]
pub struct SetQueue<T, J, C> {
    /// `Some` while the worker is loading: the outputs it is still wanted on,
    /// and its client while it is wanted at all.
    loading: Option<(Vec<T>, Option<C>)>,
    /// Requests waiting for the worker, oldest first.
    waiting: Vec<Waiting<T, J, C>>,
}

#[derive(Debug)]
struct Waiting<T, J, C> {
    targets: Vec<T>,
    job: J,
    client: C,
}

impl<T, J, C> Default for SetQueue<T, J, C> {
    fn default() -> Self {
        Self {
            loading: None,
            waiting: Vec::new(),
        }
    }
}

impl<T: PartialEq + Clone, J, C> SetQueue<T, J, C> {
    /// A new request for `targets` (which must not be empty). It is either
    /// started at once or kept until the running load ends, and it takes its
    /// outputs from every request before it that is not shown yet.
    pub fn submit(&mut self, targets: Vec<T>, job: J, client: C) -> Submitted<T, J, C> {
        let mut superseded = Vec::new();

        let mut kept = Vec::with_capacity(self.waiting.len());
        for mut earlier in self.waiting.drain(..) {
            earlier.targets.retain(|t| !targets.contains(t));
            if earlier.targets.is_empty() {
                superseded.push(earlier.client);
            } else {
                kept.push(earlier);
            }
        }
        self.waiting = kept;

        let Some((loading_targets, loading_client)) = &mut self.loading else {
            self.loading = Some((targets.clone(), Some(client)));
            return Submitted {
                start: Some((targets, job)),
                superseded,
            };
        };
        loading_targets.retain(|t| !targets.contains(t));
        if loading_targets.is_empty() {
            superseded.extend(loading_client.take());
        }
        self.waiting.push(Waiting {
            targets,
            job,
            client,
        });
        Submitted {
            start: None,
            superseded,
        }
    }

    /// The running load has ended, with or without success.
    pub fn finished(&mut self) -> Finished<T, J, C> {
        let wanted = match self.loading.take() {
            Some((targets, Some(client))) if !targets.is_empty() => Some((client, targets)),
            _ => None,
        };
        let next = (!self.waiting.is_empty()).then(|| {
            let next = self.waiting.remove(0);
            self.loading = Some((next.targets.clone(), Some(next.client)));
            (next.targets, next.job)
        });
        Finished { wanted, next }
    }

    /// The outputs the running load is still wanted on: empty if nothing is
    /// loading, or newer requests have taken all of them.
    pub fn loading_targets(&self) -> &[T] {
        match &self.loading {
            Some((targets, Some(_))) => targets,
            _ => &[],
        }
    }

    /// Whether a request that is loading or waiting is for `target`.
    pub fn is_pending_for(&self, target: &T) -> bool {
        self.loading_targets().contains(target)
            || self.waiting.iter().any(|w| w.targets.contains(target))
    }

    /// Whether nothing is loading or waiting. For the tests.
    #[cfg(test)]
    pub fn is_idle(&self) -> bool {
        self.loading.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Queue = SetQueue<&'static str, &'static str, u32>;

    const A: &str = "DP-3";
    const B: &str = "HDMI-A-1";

    fn started(
        targets: &[&'static str],
        job: &'static str,
    ) -> Submitted<&'static str, &'static str, u32> {
        Submitted {
            start: Some((targets.to_vec(), job)),
            superseded: Vec::new(),
        }
    }

    fn queued(superseded: &[u32]) -> Submitted<&'static str, &'static str, u32> {
        Submitted {
            start: None,
            superseded: superseded.to_vec(),
        }
    }

    fn finished(
        wanted: Option<(u32, &[&'static str])>,
        next: Option<(&[&'static str], &'static str)>,
    ) -> Finished<&'static str, &'static str, u32> {
        Finished {
            wanted: wanted.map(|(client, targets)| (client, targets.to_vec())),
            next: next.map(|(targets, job)| (targets.to_vec(), job)),
        }
    }

    #[test]
    fn a_single_request_is_loaded_and_answered() {
        let mut queue = Queue::default();
        assert!(queue.is_idle());

        assert_eq!(queue.submit(vec![A], "a.png", 1), started(&[A], "a.png"));
        assert!(!queue.is_idle());
        assert_eq!(queue.finished(), finished(Some((1, &[A])), None));
        assert!(queue.is_idle());
    }

    #[test]
    fn requests_that_do_not_overlap_are_each_loaded() {
        let mut queue = Queue::default();
        for (job, client) in [("a.png", 1), ("b.png", 2), ("c.png", 3)] {
            assert_eq!(queue.submit(vec![A, B], job, client), started(&[A, B], job));
            assert_eq!(queue.finished(), finished(Some((client, &[A, B])), None));
        }
    }

    #[test]
    fn a_request_during_a_load_replaces_it_and_is_loaded_next() {
        let mut queue = Queue::default();
        assert_eq!(queue.submit(vec![A], "a.png", 1), started(&[A], "a.png"));
        // The first client learns at once that its wallpaper will not be shown.
        assert_eq!(queue.submit(vec![A], "b.png", 2), queued(&[1]));

        // `a.png` ends: nobody wants it any more, and `b.png` is next.
        assert_eq!(queue.finished(), finished(None, Some((&[A], "b.png"))));
        assert!(!queue.is_idle());
        assert_eq!(queue.finished(), finished(Some((2, &[A])), None));
        assert!(queue.is_idle());
    }

    #[test]
    fn only_the_latest_of_many_requests_is_loaded() {
        let mut queue = Queue::default();
        assert_eq!(queue.submit(vec![A], "a.png", 1), started(&[A], "a.png"));
        assert_eq!(queue.submit(vec![A], "b.png", 2), queued(&[1]));
        assert_eq!(queue.submit(vec![A], "c.png", 3), queued(&[2]));
        assert_eq!(queue.submit(vec![A], "d.png", 4), queued(&[3]));

        assert_eq!(queue.finished(), finished(None, Some((&[A], "d.png"))));
        assert_eq!(queue.finished(), finished(Some((4, &[A])), None));
    }

    #[test]
    fn requests_for_different_outputs_do_not_replace_each_other() {
        let mut queue = Queue::default();
        assert_eq!(queue.submit(vec![A], "a.png", 1), started(&[A], "a.png"));
        assert_eq!(queue.submit(vec![B], "b.png", 2), queued(&[]));
        // A newer one for A replaces the load (it was for A alone), and leaves
        // B's request alone.
        assert_eq!(queue.submit(vec![A], "c.png", 3), queued(&[1]));

        assert_eq!(queue.finished(), finished(None, Some((&[B], "b.png"))));
        assert_eq!(
            queue.finished(),
            finished(Some((2, &[B])), Some((&[A], "c.png")))
        );
        assert_eq!(queue.finished(), finished(Some((3, &[A])), None));
    }

    #[test]
    fn a_request_for_one_output_takes_it_from_an_earlier_one_for_all() {
        let mut queue = Queue::default();
        assert_eq!(
            queue.submit(vec![A, B], "all.png", 1),
            started(&[A, B], "all.png")
        );
        assert_eq!(queue.loading_targets(), [A, B]);
        // The load goes on, for B only.
        assert_eq!(queue.submit(vec![A], "a.png", 2), queued(&[]));
        assert_eq!(queue.loading_targets(), [B]);

        assert_eq!(
            queue.finished(),
            finished(Some((1, &[B])), Some((&[A], "a.png")))
        );
        assert_eq!(queue.finished(), finished(Some((2, &[A])), None));
    }

    #[test]
    fn a_request_for_all_outputs_replaces_everything_before_it() {
        let mut queue = Queue::default();
        assert_eq!(queue.submit(vec![A], "a.png", 1), started(&[A], "a.png"));
        assert_eq!(queue.submit(vec![B], "b.png", 2), queued(&[]));
        assert_eq!(queue.submit(vec![A, B], "all.png", 3), queued(&[2, 1]));
        assert_eq!(queue.loading_targets(), [] as [&str; 0]);

        assert_eq!(queue.finished(), finished(None, Some((&[A, B], "all.png"))));
        assert_eq!(queue.finished(), finished(Some((3, &[A, B])), None));
    }

    #[test]
    fn a_waiting_request_keeps_the_outputs_nobody_took() {
        let mut queue = Queue::default();
        assert_eq!(
            queue.submit(vec![A], "first.png", 1),
            started(&[A], "first.png")
        );
        assert_eq!(queue.submit(vec![A, B], "all.png", 2), queued(&[1]));
        assert_eq!(queue.submit(vec![B], "b.png", 3), queued(&[]));
        assert!(queue.is_pending_for(&A));
        assert!(queue.is_pending_for(&B));

        assert_eq!(queue.finished(), finished(None, Some((&[A], "all.png"))));
        assert_eq!(
            queue.finished(),
            finished(Some((2, &[A])), Some((&[B], "b.png")))
        );
        assert_eq!(queue.finished(), finished(Some((3, &[B])), None));
        assert!(!queue.is_pending_for(&A));
    }

    #[test]
    fn a_running_load_is_wanted_until_newer_requests_take_all_its_outputs() {
        let mut queue = Queue::default();
        assert!(queue.loading_targets().is_empty());

        queue.submit(vec![A, B], "a.png", 1);
        assert_eq!(queue.loading_targets(), [A, B]);
        queue.submit(vec![B], "b.png", 2);
        assert_eq!(queue.loading_targets(), [A]);
        queue.submit(vec![A], "c.png", 3);
        assert!(queue.loading_targets().is_empty());
    }

    #[test]
    fn finished_without_a_load_changes_nothing() {
        let mut queue = Queue::default();
        assert_eq!(queue.finished(), finished(None, None));
        assert!(queue.is_idle());
        assert_eq!(queue.submit(vec![A], "a.png", 1), started(&[A], "a.png"));
    }

    /// A worker that takes one job at a time and finishes when it is told to,
    /// driven the way the daemon drives the real one.
    #[derive(Default)]
    struct FakeLoader {
        queue: Queue,
        running: Option<&'static str>,
        /// Every job the worker was given, in order.
        loaded: Vec<&'static str>,
        /// What was put on which output, in order.
        shown: Vec<(&'static str, &'static str)>,
        /// The answers: `(client, whether its wallpaper was shown somewhere)`.
        answers: Vec<(u32, bool)>,
    }

    impl FakeLoader {
        fn start(&mut self, job: &'static str) {
            assert_eq!(self.running, None, "one load at a time");
            self.running = Some(job);
            self.loaded.push(job);
        }

        fn set(&mut self, targets: Vec<&'static str>, job: &'static str, client: u32) {
            let Submitted { start, superseded } = self.queue.submit(targets, job, client);
            for client in superseded {
                self.answers.push((client, false));
            }
            if let Some((_, job)) = start {
                self.start(job);
            }
        }

        fn finish(&mut self) {
            let job = self.running.take().expect("a load is running");
            let Finished { wanted, next } = self.queue.finished();
            if let Some((client, targets)) = wanted {
                for target in targets {
                    self.shown.push((target, job));
                }
                self.answers.push((client, true));
            }
            if let Some((_, job)) = next {
                self.start(job);
            }
        }
    }

    #[test]
    fn latest_wins_with_a_slow_loader() {
        let mut daemon = FakeLoader::default();
        // Five requests arrive while the first one is still loading.
        for (client, job) in ["a", "b", "c", "d", "e"].into_iter().enumerate() {
            daemon.set(vec![A], job, client as u32);
        }
        daemon.finish();
        daemon.finish();

        // Two loads for five requests, and only the last request was shown.
        assert_eq!(daemon.loaded, ["a", "e"]);
        assert_eq!(daemon.shown, [(A, "e")]);
        assert_eq!(
            daemon.answers,
            [(0, false), (1, false), (2, false), (3, false), (4, true)]
        );
        assert!(daemon.queue.is_idle());
    }

    #[test]
    fn every_request_is_answered_once_and_each_output_ends_with_its_latest() {
        // Six requests, each for A, for B or for both, in every combination of
        // targets and of "the running load ends first".
        let choices = [vec![A], vec![B], vec![A, B]];
        for pattern in 0u32..(3u32.pow(6) * 64) {
            let (mut kinds, finishes) = (pattern / 64, pattern % 64);
            let mut daemon = FakeLoader::default();
            let mut last = [None, None];
            for client in 0..6u32 {
                if finishes & (1 << client) != 0 && daemon.running.is_some() {
                    daemon.finish();
                }
                let targets = choices[(kinds % 3) as usize].clone();
                kinds /= 3;
                for (i, output) in [A, B].into_iter().enumerate() {
                    if targets.contains(&output) {
                        last[i] = Some(client);
                    }
                }
                let job: &'static str = ["0", "1", "2", "3", "4", "5"][client as usize];
                daemon.set(targets, job, client);
            }
            while daemon.running.is_some() {
                daemon.finish();
            }

            let mut clients: Vec<u32> = daemon.answers.iter().map(|(client, _)| *client).collect();
            clients.sort_unstable();
            assert_eq!(clients, [0, 1, 2, 3, 4, 5], "pattern {pattern}");
            // What each output shows last is its latest request.
            for (i, output) in [A, B].into_iter().enumerate() {
                let shown = daemon.shown.iter().rev().find(|(o, _)| *o == output);
                let expected =
                    last[i].map(|client| ["0", "1", "2", "3", "4", "5"][client as usize]);
                assert_eq!(
                    shown.map(|(_, job)| *job),
                    expected,
                    "pattern {pattern}, {output}"
                );
            }
            assert!(daemon.queue.is_idle());
        }
    }
}
