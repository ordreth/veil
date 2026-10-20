//! One answer for a `veil set` that changes several outputs.
//!
//! A `set` without `--output` puts a wallpaper on every output, and each output
//! switches on its own time: an image at once, a video when its first frame is
//! there, either one only after the wallpaper it replaced is gone. The client
//! gets one answer, when the last of them is done. Each output's share of the
//! request is a [`Ticket`], and [`Answers`] collects how each one ended.
//!
//! This is generic over the client (`C`, who to answer), so it is tested
//! without a socket.

/// One output's part of a request. It is not `Clone`: every part is reported
/// exactly once, through [`Answers::done`].
#[derive(Debug, PartialEq, Eq)]
pub struct Ticket {
    id: u64,
    /// The output, as the client's answer names it.
    output: String,
}

/// How one output's part ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// The wallpaper is on the output (or stored for it while it is away).
    Done,
    /// A newer request for the output replaced it before it was shown.
    Superseded,
    /// It could not be shown there; the message says why.
    Failed(String),
}

/// The answer for the whole request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// Every part was replaced by newer requests.
    Superseded,
    /// At least one part failed. With a single output this is its message;
    /// otherwise each failure is named after its output, unless they all failed
    /// for the same reason.
    Failed(String),
}

pub struct Answers<C> {
    open: Vec<Open<C>>,
    last_id: u64,
}

struct Open<C> {
    id: u64,
    client: C,
    parts: usize,
    remaining: usize,
    superseded: usize,
    /// `(output, message)`, in the order they came.
    failures: Vec<(String, String)>,
}

impl<C> Default for Answers<C> {
    fn default() -> Self {
        Self {
            open: Vec::new(),
            last_id: 0,
        }
    }
}

impl<C> Answers<C> {
    /// Splits the request of `client` into one ticket per output. With no
    /// output there is nothing to wait for, and the client is handed back.
    pub fn open(&mut self, client: C, outputs: Vec<String>) -> Result<Vec<Ticket>, C> {
        if outputs.is_empty() {
            return Err(client);
        }
        self.last_id += 1;
        let id = self.last_id;
        self.open.push(Open {
            id,
            client,
            parts: outputs.len(),
            remaining: outputs.len(),
            superseded: 0,
            failures: Vec::new(),
        });
        Ok(outputs
            .into_iter()
            .map(|output| Ticket { id, output })
            .collect())
    }

    /// One part has ended. When it is the last part of its request, the client
    /// is returned with the answer for the whole request.
    pub fn done(&mut self, ticket: Ticket, part: Part) -> Option<(C, Verdict)> {
        let index = self.open.iter().position(|open| open.id == ticket.id)?;
        let open = &mut self.open[index];
        open.remaining = open.remaining.saturating_sub(1);
        match part {
            Part::Done => {}
            Part::Superseded => open.superseded += 1,
            Part::Failed(message) => open.failures.push((ticket.output, message)),
        }
        if open.remaining > 0 {
            return None;
        }
        let open = self.open.swap_remove(index);
        let verdict = verdict(open.parts, open.superseded, open.failures);
        Some((open.client, verdict))
    }

    /// Whether no request waits for its parts. For the tests.
    #[cfg(test)]
    pub fn is_idle(&self) -> bool {
        self.open.is_empty()
    }
}

fn verdict(parts: usize, superseded: usize, failures: Vec<(String, String)>) -> Verdict {
    let Some((_, first)) = failures.first() else {
        return if superseded == parts {
            Verdict::Superseded
        } else {
            Verdict::Ok
        };
    };
    let all_alike = failures.len() == parts && failures.iter().all(|(_, m)| m == first);
    if parts == 1 || all_alike {
        return Verdict::Failed(first.clone());
    }
    let named: Vec<String> = failures
        .iter()
        .map(|(output, message)| format!("{output}: {message}"))
        .collect();
    Verdict::Failed(named.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outputs(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn tickets(answers: &mut Answers<u32>, client: u32, names: &[&str]) -> Vec<Ticket> {
        answers
            .open(client, outputs(names))
            .expect("there are outputs")
    }

    #[test]
    fn a_single_output_is_answered_with_its_own_result() {
        let mut answers = Answers::default();
        for (part, verdict) in [
            (Part::Done, Verdict::Ok),
            (Part::Superseded, Verdict::Superseded),
            (Part::Failed("boom".into()), Verdict::Failed("boom".into())),
        ] {
            let [ticket] =
                <[Ticket; 1]>::try_from(tickets(&mut answers, 7, &["DP-3"])).expect("one ticket");
            assert_eq!(answers.done(ticket, part), Some((7, verdict)));
            assert!(answers.is_idle());
        }
    }

    #[test]
    fn several_outputs_are_answered_when_the_last_one_is_done() {
        let mut answers = Answers::default();
        let mut parts = tickets(&mut answers, 1, &["DP-3", "HDMI-A-1", "eDP-1"]).into_iter();
        let (a, b, c) = (
            parts.next().expect("a"),
            parts.next().expect("b"),
            parts.next().expect("c"),
        );
        // In any order.
        assert_eq!(answers.done(b, Part::Done), None);
        assert_eq!(answers.done(c, Part::Superseded), None);
        assert!(!answers.is_idle());
        assert_eq!(answers.done(a, Part::Done), Some((1, Verdict::Ok)));
        assert!(answers.is_idle());
    }

    #[test]
    fn requests_are_kept_apart() {
        let mut answers = Answers::default();
        let first = tickets(&mut answers, 1, &["DP-3", "HDMI-A-1"]);
        let second = tickets(&mut answers, 2, &["DP-3"]);
        let mut second = second.into_iter();
        assert_eq!(
            answers.done(second.next().expect("ticket"), Part::Done),
            Some((2, Verdict::Ok))
        );
        let mut first = first.into_iter();
        assert_eq!(
            answers.done(first.next().expect("ticket"), Part::Done),
            None
        );
        assert_eq!(
            answers.done(first.next().expect("ticket"), Part::Failed("x".into())),
            Some((1, Verdict::Failed("HDMI-A-1: x".into())))
        );
    }

    #[test]
    fn failures_are_named_after_their_outputs_unless_they_are_all_alike() {
        let mut answers = Answers::default();
        let mut parts = tickets(&mut answers, 1, &["DP-3", "HDMI-A-1", "eDP-1"]).into_iter();
        answers.done(parts.next().expect("a"), Part::Failed("no video".into()));
        answers.done(parts.next().expect("b"), Part::Done);
        assert_eq!(
            answers.done(parts.next().expect("c"), Part::Failed("busy".into())),
            Some((1, Verdict::Failed("DP-3: no video; eDP-1: busy".into())))
        );

        // A file that is no video fails the same way everywhere.
        let mut parts = tickets(&mut answers, 2, &["DP-3", "HDMI-A-1"]).into_iter();
        answers.done(parts.next().expect("a"), Part::Failed("no video".into()));
        assert_eq!(
            answers.done(parts.next().expect("b"), Part::Failed("no video".into())),
            Some((2, Verdict::Failed("no video".into())))
        );
    }

    #[test]
    fn a_request_is_superseded_only_if_every_part_is() {
        let mut answers = Answers::default();
        let mut parts = tickets(&mut answers, 1, &["DP-3", "HDMI-A-1"]).into_iter();
        answers.done(parts.next().expect("a"), Part::Superseded);
        assert_eq!(
            answers.done(parts.next().expect("b"), Part::Superseded),
            Some((1, Verdict::Superseded))
        );

        let mut parts = tickets(&mut answers, 2, &["DP-3", "HDMI-A-1"]).into_iter();
        answers.done(parts.next().expect("a"), Part::Superseded);
        assert_eq!(
            answers.done(parts.next().expect("b"), Part::Done),
            Some((2, Verdict::Ok))
        );
    }

    #[test]
    fn a_request_without_outputs_is_handed_back() {
        let mut answers: Answers<u32> = Answers::default();
        assert_eq!(answers.open(5, Vec::new()), Err(5));
        assert!(answers.is_idle());
    }
}
