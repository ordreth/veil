//! Which outputs get a wallpaper surface, across hotplug.
//!
//! The daemon draws on every output it is given: all of them by default, or the
//! ones named with `--output`. Each wallpaper belongs to an output **by name**
//! ([`OutputKey`]): when an output goes away its surface is released and the
//! daemon remembers what it showed there, and when an output with that name is
//! announced again a surface is created anew for it. A wallpaper never moves to
//! another output.
//!
//! This is a pure state machine (the output id is generic), so the hotplug
//! sequences can be tested without a compositor. `AppState` owns the Wayland
//! objects and follows what the methods here return.

use std::fmt;

/// What a wallpaper belongs to: the name of its output, which outlives the
/// `wl_output` object (a replugged monitor is a new object with the old name).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OutputKey {
    Named(String),
    /// An output the compositor gave no name. It cannot be recognised when it
    /// returns, so its wallpaper ends with its surface. The number only tells
    /// such outputs apart for as long as they are there.
    Unnamed(u32),
}

impl OutputKey {
    /// The name, if the output has one.
    pub fn name(&self) -> Option<&str> {
        match self {
            OutputKey::Named(name) => Some(name),
            OutputKey::Unnamed(_) => None,
        }
    }
}

impl fmt::Display for OutputKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputKey::Named(name) => f.write_str(name),
            OutputKey::Unnamed(number) => write!(f, "<unnamed output {number}>"),
        }
    }
}

/// The outputs that host a wallpaper surface, and the rule for choosing them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputBinding<Id> {
    /// Outputs requested with `--output`. Empty means every output.
    requested: Vec<String>,
    /// The outputs that host a surface right now.
    bound: Vec<(Id, OutputKey)>,
    /// Whether an output has been bound at some point.
    ever_bound: bool,
    last_unnamed: u32,
}

impl<Id: PartialEq + Clone> OutputBinding<Id> {
    pub fn new(requested: Vec<String>) -> Self {
        Self {
            requested,
            bound: Vec::new(),
            ever_bound: false,
            last_unnamed: 0,
        }
    }

    /// The outputs requested with `--output`; empty for every output.
    pub fn requested(&self) -> &[String] {
        &self.requested
    }

    /// Whether an output has been bound at some point (it may be away now).
    pub fn ever_bound(&self) -> bool {
        self.ever_bound
    }

    /// Whether an output called `name` may host a wallpaper: with `--output`
    /// only the outputs named there, otherwise every output (also one without
    /// a name).
    pub fn wants(&self, name: Option<&str>) -> bool {
        self.requested.is_empty()
            || name.is_some_and(|name| self.requested.iter().any(|r| r == name))
    }

    /// The output that hosts the surface of `key` right now, if there is one.
    pub fn output(&self, key: &OutputKey) -> Option<&Id> {
        self.bound.iter().find(|(_, k)| k == key).map(|(id, _)| id)
    }

    /// The wallpaper whose surface `id` hosts right now, if any.
    pub fn key(&self, id: &Id) -> Option<&OutputKey> {
        self.bound.iter().find(|(i, _)| i == id).map(|(_, key)| key)
    }

    /// An output was announced or changed. Returns the wallpaper a surface must
    /// now be created for on it, or `None` if it is not wanted or already has
    /// one. An output whose name another bound output already has is not
    /// bound twice.
    pub fn offer(&mut self, id: &Id, name: Option<&str>) -> Option<OutputKey> {
        if !self.wants(name) || self.key(id).is_some() {
            return None;
        }
        let key = match name {
            Some(name) => OutputKey::Named(name.to_owned()),
            None => {
                self.last_unnamed += 1;
                OutputKey::Unnamed(self.last_unnamed)
            }
        };
        if self.output(&key).is_some() {
            return None;
        }
        self.bound.push((id.clone(), key.clone()));
        self.ever_bound = true;
        Some(key)
    }

    /// An output was removed. Returns the wallpaper whose surface it hosted,
    /// which must then be released.
    pub fn remove(&mut self, id: &Id) -> Option<OutputKey> {
        let index = self.bound.iter().position(|(i, _)| i == id)?;
        Some(self.bound.swap_remove(index).1)
    }

    /// The compositor closed the surface of `key`. Returns `true` if it had
    /// one. The output itself may still exist; its next [`Self::offer`] binds
    /// it again.
    pub fn surface_closed(&mut self, key: &OutputKey) -> bool {
        let before = self.bound.len();
        self.bound.retain(|(_, k)| k != key);
        self.bound.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake output ids: a compositor gives a replugged output a new object.
    type Binding = OutputBinding<u32>;

    fn named(name: &str) -> Option<OutputKey> {
        Some(OutputKey::Named(name.into()))
    }

    #[test]
    fn requested_outputs_must_match_by_name() {
        let binding = Binding::new(vec!["HDMI-A-1".into(), "DP-3".into()]);
        assert!(binding.wants(Some("HDMI-A-1")));
        assert!(binding.wants(Some("DP-3")));
        assert!(!binding.wants(Some("DP-1")));
        assert!(!binding.wants(Some("hdmi-a-1")));
        assert!(!binding.wants(None));
    }

    #[test]
    fn without_a_request_every_output_is_wanted() {
        let binding = Binding::new(Vec::new());
        assert!(binding.wants(Some("DP-3")));
        assert!(binding.wants(Some("eDP-1")));
        assert!(binding.wants(None));
    }

    #[test]
    fn every_announced_output_gets_a_surface() {
        let mut binding = Binding::new(Vec::new());
        assert!(!binding.ever_bound());

        assert_eq!(binding.offer(&1, Some("HDMI-A-1")), named("HDMI-A-1"));
        assert_eq!(binding.offer(&2, Some("DP-3")), named("DP-3"));
        assert!(binding.ever_bound());
        assert_eq!(binding.output(&OutputKey::Named("DP-3".into())), Some(&2));
        assert_eq!(binding.key(&1), Some(&OutputKey::Named("HDMI-A-1".into())));

        // Updates of bound outputs change nothing.
        assert_eq!(binding.offer(&1, Some("HDMI-A-1")), None);
        assert_eq!(binding.offer(&2, Some("DP-3")), None);
    }

    #[test]
    fn only_requested_outputs_are_bound_and_the_others_are_ignored() {
        let mut binding = Binding::new(vec!["DP-3".into(), "eDP-1".into()]);
        assert_eq!(binding.offer(&1, Some("HDMI-A-1")), None);
        assert!(!binding.ever_bound());

        assert_eq!(binding.offer(&2, Some("DP-3")), named("DP-3"));
        assert_eq!(binding.key(&1), None);
        // The other requested output comes later.
        assert_eq!(binding.offer(&3, Some("eDP-1")), named("eDP-1"));
        assert_eq!(binding.requested(), ["DP-3", "eDP-1"]);
    }

    #[test]
    fn unplugging_one_output_leaves_the_others_alone() {
        let mut binding = Binding::new(Vec::new());
        binding.offer(&1, Some("HDMI-A-1"));
        binding.offer(&2, Some("DP-3"));

        assert_eq!(binding.remove(&1), named("HDMI-A-1"));
        assert_eq!(binding.output(&OutputKey::Named("DP-3".into())), Some(&2));
        assert_eq!(binding.output(&OutputKey::Named("HDMI-A-1".into())), None);
        // Removed once, released once.
        assert_eq!(binding.remove(&1), None);

        // Replug: same name, new object.
        assert_eq!(binding.offer(&4, Some("HDMI-A-1")), named("HDMI-A-1"));
        assert_eq!(binding.key(&4), Some(&OutputKey::Named("HDMI-A-1".into())));
        assert_eq!(binding.key(&2), Some(&OutputKey::Named("DP-3".into())));
    }

    #[test]
    fn repeated_hotplug_cycles_keep_working() {
        let mut binding = Binding::new(vec!["eDP-1".into()]);
        for id in 0..5 {
            assert_eq!(binding.offer(&id, Some("eDP-1")), named("eDP-1"), "{id}");
            assert_eq!(binding.offer(&id, Some("eDP-1")), None, "{id}");
            assert_eq!(binding.remove(&id), named("eDP-1"), "{id}");
            assert_eq!(binding.remove(&id), None, "{id}");
        }
        assert_eq!(binding.output(&OutputKey::Named("eDP-1".into())), None);
    }

    #[test]
    fn a_closed_surface_is_created_again_on_the_next_offer_of_its_output() {
        let mut binding = Binding::new(Vec::new());
        binding.offer(&1, Some("HDMI-A-1"));
        binding.offer(&2, Some("DP-3"));
        let hdmi = OutputKey::Named("HDMI-A-1".into());

        // The compositor closes the surface but the output object stays.
        assert!(binding.surface_closed(&hdmi));
        assert!(!binding.surface_closed(&hdmi));
        assert_eq!(binding.key(&1), None);
        // Its removal afterwards releases nothing more.
        let mut gone = binding.clone();
        assert_eq!(gone.remove(&1), None);

        // An update of the same output (or its re-announcement) binds again.
        assert_eq!(binding.offer(&1, Some("HDMI-A-1")), Some(hdmi));
        assert_eq!(binding.key(&2), Some(&OutputKey::Named("DP-3".into())));
    }

    #[test]
    fn unnamed_outputs_are_told_apart_while_they_are_there() {
        let mut binding = Binding::new(Vec::new());
        let first = binding.offer(&1, None).expect("bound");
        let second = binding.offer(&2, None).expect("bound");
        assert_ne!(first, second);
        assert_eq!(first.name(), None);
        assert_eq!(binding.offer(&1, None), None);

        // Back after an unplug, it cannot be recognised: a new wallpaper.
        assert_eq!(binding.remove(&1), Some(first.clone()));
        let again = binding.offer(&3, None).expect("bound");
        assert_ne!(again, first);
        assert_ne!(again, second);
    }

    #[test]
    fn two_outputs_with_the_same_name_get_one_surface() {
        let mut binding = Binding::new(Vec::new());
        assert_eq!(binding.offer(&1, Some("DP-3")), named("DP-3"));
        assert_eq!(binding.offer(&2, Some("DP-3")), None);
        // Once the first is gone, the second may have it.
        binding.remove(&1);
        assert_eq!(binding.offer(&2, Some("DP-3")), named("DP-3"));
    }

    #[test]
    fn output_keys_read_as_their_names() {
        assert_eq!(OutputKey::Named("DP-3".into()).to_string(), "DP-3");
        assert_eq!(OutputKey::Unnamed(2).to_string(), "<unnamed output 2>");
        assert_eq!(OutputKey::Named("DP-3".into()).name(), Some("DP-3"));
    }
}
