//! `THIRD-PARTY-NOTICES` is generated from `Cargo.lock` (`cargo about generate
//! about.hbs`); this fails when a dependency was changed and the file was not.
//! CI regenerates it and compares; this is the check that runs everywhere.

use std::collections::HashSet;

/// `(name, version)` of every package in a `Cargo.lock`.
fn locked(lock: &str) -> HashSet<(String, String)> {
    let mut packages = HashSet::new();
    let mut name = None;
    for line in lock.lines() {
        if let Some(value) = line.strip_prefix("name = ") {
            name = Some(value.trim_matches('"').to_string());
        } else if let Some(value) = line.strip_prefix("version = ") {
            if let Some(name) = name.take() {
                packages.insert((name, value.trim_matches('"').to_string()));
            }
        }
    }
    packages
}

#[test]
fn every_crate_in_the_notices_is_in_cargo_lock_at_that_version() {
    let locked = locked(include_str!("../Cargo.lock"));
    let notices = include_str!("../THIRD-PARTY-NOTICES");
    let mut listed = 0;
    let mut in_used_by = false;
    for line in notices.lines() {
        if line == "Used by:" {
            in_used_by = true;
            continue;
        }
        if line.is_empty() {
            in_used_by = false;
        }
        if !in_used_by {
            continue;
        }
        let Some(rest) = line.strip_prefix("  * ") else {
            continue;
        };
        let mut words = rest.split_whitespace();
        let (Some(name), Some(version)) = (words.next(), words.next()) else {
            continue;
        };
        listed += 1;
        assert!(
            locked.contains(&(name.to_string(), version.to_string())),
            "THIRD-PARTY-NOTICES lists {name} {version}, which Cargo.lock does not have: \
             run `cargo about generate about.hbs -o THIRD-PARTY-NOTICES`"
        );
    }
    assert!(listed > 100, "the notices list only {listed} crates");
}

#[test]
fn veil_itself_is_not_listed_as_a_third_party() {
    assert!(!include_str!("../THIRD-PARTY-NOTICES").contains("  * veil "));
}
