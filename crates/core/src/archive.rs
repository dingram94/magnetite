//! The archive of closed tabs, as it syncs (docs/spec/archive.md, "On other
//! devices"): its records, how many of them go to the chain, and how two
//! devices' archives become one.
//!
//! A record never changes: it is there or not. A device that knows which
//! records it and the chain last agreed on (`Key`s) can so tell what was
//! added and what was removed on each side since.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::model::{Id, Other, TabItem, UnixSeconds};

/// The most records the chain holds, the newest. Every change writes the
/// collection whole, also from a phone: 2000 records are about 1 MiB as
/// they are sent. A device may keep more for itself.
pub const SYNCED_MOST: usize = 2000;

/// A closed tab: what it was, where, and since when it is archived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedTab {
    pub tab: TabItem,
    pub space_id: Id,
    pub archived_at: UnixSeconds,
    #[serde(flatten)]
    pub other: Other,
}

/// What tells one record from another: the tab and when it was archived. A
/// tab brought back and closed again is another record.
pub type Key = (Id, UnixSeconds);

impl ArchivedTab {
    pub fn key(&self) -> Key {
        (self.tab.id.clone(), self.archived_at)
    }
}

/// The `archive` collection of a sync chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Archive {
    /// Raised by a change that an older version must not write over; a
    /// device that reads a higher one than `Archive::VERSION` does neither
    /// take nor write the archive.
    pub version: u32,
    /// Oldest first.
    #[serde(default)]
    pub entries: Vec<ArchivedTab>,
    #[serde(flatten)]
    pub other: Other,
}

impl Archive {
    pub const VERSION: u32 = 1;

    pub fn new(entries: Vec<ArchivedTab>) -> Self {
        Self {
            version: Self::VERSION,
            entries,
            other: Other::new(),
        }
    }
}

/// The keys of `entries`.
pub fn keys(entries: &[ArchivedTab]) -> Vec<Key> {
    entries.iter().map(ArchivedTab::key).collect()
}

/// `entries` oldest first, each tab once (the record archived last), and
/// of those the newest `most`.
#[must_use]
pub fn newest(mut entries: Vec<ArchivedTab>, most: usize) -> Vec<ArchivedTab> {
    // By id too: every device then writes the same records the same way.
    entries.sort_by(|a, b| (a.archived_at, &a.tab.id).cmp(&(b.archived_at, &b.tab.id)));
    let mut last: HashMap<&Id, usize> = HashMap::new();
    for (at, e) in entries.iter().enumerate() {
        last.insert(&e.tab.id, at);
    }
    let keep: HashSet<usize> = last.into_values().collect();
    let mut at = 0;
    entries.retain(|_| {
        at += 1;
        keep.contains(&(at - 1))
    });
    let cut = entries.len().saturating_sub(most);
    entries.split_off(cut)
}

/// This device's archive (`local`, what of it syncs) and the chain's
/// (`remote`) as one, oldest first. `agreed`: the records the two last
/// agreed on, none for a device that never did.
///
/// - What either side added since is there.
/// - What either side removed since (a tab brought back, deleted or past
///   its time) is gone.
/// - Without `agreed` nothing counts as removed: both sides' records stay.
/// - A full `remote` holds only the newest records, so one that is older
///   than all of it was not removed there, it no longer fits: this device
///   keeps it.
#[must_use]
pub fn merge(
    agreed: Option<&[Key]>,
    local: &[ArchivedTab],
    remote: &[ArchivedTab],
) -> Vec<ArchivedTab> {
    let agreed: Option<HashSet<&Key>> = agreed.map(|keys| keys.iter().collect());
    let was_agreed = |key: &Key| agreed.as_ref().is_some_and(|a| a.contains(key));
    let theirs: HashSet<Key> = remote.iter().map(ArchivedTab::key).collect();
    let mine: HashSet<Key> = local.iter().map(ArchivedTab::key).collect();
    let full_from = (remote.len() >= SYNCED_MOST)
        .then(|| remote.iter().map(|e| e.archived_at).min())
        .flatten();
    let pushed_out = |e: &ArchivedTab| full_from.is_some_and(|oldest| e.archived_at < oldest);
    let kept = local.iter().filter(|e| {
        let key = e.key();
        theirs.contains(&key) || !was_agreed(&key) || pushed_out(e)
    });
    let new = remote.iter().filter(|e| {
        let key = e.key();
        !mine.contains(&key) && !was_agreed(&key)
    });
    newest(kept.chain(new).cloned().collect(), usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, at: UnixSeconds) -> ArchivedTab {
        ArchivedTab {
            tab: TabItem::new(id, format!("https://example.org/{id}"), id),
            space_id: "s".into(),
            archived_at: at,
            other: Other::new(),
        }
    }

    fn ids(entries: &[ArchivedTab]) -> Vec<&str> {
        entries.iter().map(|e| e.tab.id.as_str()).collect()
    }

    #[test]
    fn what_both_devices_closed_is_in_the_archive_of_both() {
        let agreed = [entry("a", 1), entry("b", 2)];
        let local = [entry("a", 1), entry("b", 2), entry("here", 5)];
        let remote = [entry("a", 1), entry("b", 2), entry("there", 4)];
        let merged = merge(Some(&keys(&agreed)), &local, &remote);
        assert_eq!(ids(&merged), ["a", "b", "there", "here"]);
    }

    #[test]
    fn a_tab_brought_back_or_deleted_on_one_device_leaves_both() {
        let agreed = [entry("a", 1), entry("b", 2), entry("c", 3)];
        // Here `a` was brought back, there `c` was deleted.
        let local = [entry("b", 2), entry("c", 3)];
        let remote = [entry("a", 1), entry("b", 2)];
        let merged = merge(Some(&keys(&agreed)), &local, &remote);
        assert_eq!(ids(&merged), ["b"]);
    }

    #[test]
    fn a_tab_closed_again_is_another_record() {
        let agreed = [entry("a", 1)];
        // Brought back here and closed again later; there it never left.
        let local = [entry("a", 9)];
        let remote = [entry("a", 1)];
        let merged = merge(Some(&keys(&agreed)), &local, &remote);
        assert_eq!(keys(&merged), [("a".to_string(), 9)]);
        // Closed on both devices, without knowing of each other: once.
        let merged = merge(None, &[entry("a", 9)], &[entry("a", 7)]);
        assert_eq!(keys(&merged), [("a".to_string(), 9)]);
    }

    #[test]
    fn a_device_that_never_agreed_with_the_chain_keeps_both_archives() {
        let local = [entry("mine", 3), entry("both", 1)];
        let remote = [entry("both", 1), entry("theirs", 2)];
        assert_eq!(
            ids(&merge(None, &local, &remote)),
            ["both", "theirs", "mine"]
        );
    }

    #[test]
    fn a_record_that_no_longer_fits_the_chain_stays_on_the_device_that_has_it() {
        let many = |from: i64| -> Vec<ArchivedTab> {
            (from..)
                .take(SYNCED_MOST)
                .map(|at| entry(&format!("t{at}"), at))
                .collect()
        };
        // Agreed on records 10 and up; the chain then got ten newer ones,
        // and holds 20 and up.
        let (agreed, remote) = (many(10), many(20));
        let merged = merge(Some(&keys(&agreed)), &agreed, &remote);
        assert_eq!(merged.len(), SYNCED_MOST + 10);
        assert_eq!(merged[0].tab.id, "t10", "not removed there, only too old");
        // A newer one that is missing there was removed.
        let mut remote = remote;
        remote.retain(|e| e.tab.id != "t500");
        remote.push(entry("more", 5000));
        let merged = merge(Some(&keys(&agreed)), &agreed, &remote);
        assert!(merged.iter().all(|e| e.tab.id != "t500"));
    }

    #[test]
    fn the_chain_gets_the_newest_records_each_tab_once() {
        let entries = vec![entry("b", 5), entry("a", 1), entry("c", 3), entry("a", 4)];
        let kept = newest(entries, 2);
        assert_eq!(keys(&kept), [("a".to_string(), 4), ("b".to_string(), 5)]);
    }

    #[test]
    fn what_a_newer_version_wrote_is_kept() {
        let written = serde_json::json!({
            "version": 1, "sorted": "by day",
            "entries": [{"tab": {"id": "a", "url": "https://example.org/", "title": "A",
                "custom_title": false, "unread": false}, "space_id": "s", "archived_at": 3,
                "closed_by": "phone"}]
        });
        let read: Archive = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(read.entries[0].other["closed_by"], "phone");
        assert_eq!(serde_json::to_value(&read).unwrap(), written);
    }
}
