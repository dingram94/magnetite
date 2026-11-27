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

/// A place in `newest`'s order: the time a record was archived, then its
/// tab's id (many tabs are archived in the same second, as with a space
/// deleted).
pub type Mark = (UnixSeconds, Id);

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
    /// Records before this place (`Mark`) may be missing here only because
    /// they did not fit (`SYNCED_MOST`), not because they were removed. Set
    /// by a device that sent the newest records only, and carried on by
    /// every later write (`to_write`), so it never goes back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_out_before: Option<Mark>,
    #[serde(flatten)]
    pub other: Other,
}

impl Archive {
    pub const VERSION: u32 = 1;

    pub fn new(entries: Vec<ArchivedTab>) -> Self {
        Self {
            version: Self::VERSION,
            entries,
            left_out_before: None,
            other: Other::new(),
        }
    }

    /// What a device writes over `chain` (the chain's copy, none before the
    /// first write): its own records (`mine`, as `newest` gives them), with
    /// what the chain's copy says beyond them kept, fields of a newer
    /// version included. `mine` at the limit may have left records out.
    #[must_use]
    pub fn to_write(chain: Option<Self>, mine: Vec<ArchivedTab>) -> Self {
        let cut = (mine.len() >= SYNCED_MOST)
            .then(|| mine.first().map(|e| (e.archived_at, e.tab.id.clone())))
            .flatten();
        let mut a = chain.unwrap_or_else(|| Self::new(Vec::new()));
        a.version = Self::VERSION;
        a.left_out_before = a.left_out_before.max(cut);
        a.entries = mine;
        a
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
/// - The chain holds only the newest records: one before `left_out_before`
///   (`Archive::left_out_before`) that is missing there was not removed,
///   it did not fit. This device keeps it.
#[must_use]
pub fn merge(
    agreed: Option<&[Key]>,
    local: &[ArchivedTab],
    remote: &[ArchivedTab],
    left_out_before: Option<&Mark>,
) -> Vec<ArchivedTab> {
    let agreed: Option<HashSet<&Key>> = agreed.map(|keys| keys.iter().collect());
    let was_agreed = |key: &Key| agreed.as_ref().is_some_and(|a| a.contains(key));
    let theirs: HashSet<Key> = remote.iter().map(ArchivedTab::key).collect();
    let mine: HashSet<Key> = local.iter().map(ArchivedTab::key).collect();
    let pushed_out = |e: &ArchivedTab| {
        left_out_before.is_some_and(|(at, id)| (e.archived_at, &e.tab.id) < (*at, id))
    };
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
        let merged = merge(Some(&keys(&agreed)), &local, &remote, None);
        assert_eq!(ids(&merged), ["a", "b", "there", "here"]);
    }

    #[test]
    fn a_tab_brought_back_or_deleted_on_one_device_leaves_both() {
        let agreed = [entry("a", 1), entry("b", 2), entry("c", 3)];
        // Here `a` was brought back, there `c` was deleted.
        let local = [entry("b", 2), entry("c", 3)];
        let remote = [entry("a", 1), entry("b", 2)];
        let merged = merge(Some(&keys(&agreed)), &local, &remote, None);
        assert_eq!(ids(&merged), ["b"]);
    }

    #[test]
    fn a_tab_closed_again_is_another_record() {
        let agreed = [entry("a", 1)];
        // Brought back here and closed again later; there it never left.
        let local = [entry("a", 9)];
        let remote = [entry("a", 1)];
        let merged = merge(Some(&keys(&agreed)), &local, &remote, None);
        assert_eq!(keys(&merged), [("a".to_string(), 9)]);
        // Closed on both devices, without knowing of each other: once.
        let merged = merge(None, &[entry("a", 9)], &[entry("a", 7)], None);
        assert_eq!(keys(&merged), [("a".to_string(), 9)]);
    }

    #[test]
    fn a_device_that_never_agreed_with_the_chain_keeps_both_archives() {
        let local = [entry("mine", 3), entry("both", 1)];
        let remote = [entry("both", 1), entry("theirs", 2)];
        assert_eq!(
            ids(&merge(None, &local, &remote, None)),
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
        let agreed = many(10);
        let chain = Archive::to_write(None, many(20));
        assert_eq!(chain.left_out_before, Some((20, "t20".into())));
        let merged = merge(
            Some(&keys(&agreed)),
            &agreed,
            &chain.entries,
            chain.left_out_before.as_ref(),
        );
        assert_eq!(merged.len(), SYNCED_MOST + 10);
        assert_eq!(merged[0].tab.id, "t10", "not removed there, only too old");
        // A device that joined after that holds only the chain's records,
        // and brings one back: the chain is no longer full, and still says
        // what it left out.
        let mut joined = chain.entries.clone();
        joined.retain(|e| e.tab.id != "t500");
        let chain = Archive::to_write(Some(chain), joined);
        assert_eq!(chain.entries.len(), SYNCED_MOST - 1);
        let merged = merge(
            Some(&keys(&agreed)),
            &agreed,
            &chain.entries,
            chain.left_out_before.as_ref(),
        );
        assert_eq!(merged[0].tab.id, "t10", "still only too old");
        // A newer one that is missing there was removed.
        assert!(merged.iter().all(|e| e.tab.id != "t500"));
    }

    #[test]
    fn a_record_left_out_in_a_second_full_of_them_is_not_taken_as_removed() {
        // A space of 2000 tabs deleted in one second, then one more closed
        // in it: the chain leaves out the first of them by id.
        let agreed: Vec<_> = (0..SYNCED_MOST)
            .map(|n| entry(&format!("t{n:04}"), 5))
            .collect();
        let mut all = agreed.clone();
        all.push(entry("u", 5));
        let mut chain = Archive::to_write(None, newest(all, SYNCED_MOST));
        assert_eq!(chain.left_out_before, Some((5, "t0001".into())));
        chain.entries.retain(|e| e.tab.id != "t0500");
        let merged = merge(
            Some(&keys(&agreed)),
            &agreed,
            &chain.entries,
            chain.left_out_before.as_ref(),
        );
        assert_eq!(merged[0].tab.id, "t0000", "left out, not removed");
        assert!(merged.iter().all(|e| e.tab.id != "t0500"), "removed");
        assert_eq!(merged.len(), SYNCED_MOST);
    }

    #[test]
    fn a_write_keeps_what_the_chain_says_beyond_its_records() {
        let mut chain = Archive::new(vec![entry("a", 1)]);
        chain.left_out_before = Some((1, "a".into()));
        chain.other.insert("sorted".into(), "by day".into());
        let written = Archive::to_write(Some(chain), vec![entry("b", 2)]);
        assert_eq!(ids(&written.entries), ["b"]);
        assert_eq!(written.left_out_before, Some((1, "a".into())));
        assert_eq!(written.other["sorted"], "by day");
        // Never further back than a mark there was.
        let mut chain = written;
        chain.left_out_before = Some((50, "x".into()));
        let many: Vec<_> = (0..)
            .take(SYNCED_MOST)
            .map(|at| entry(&format!("t{at}"), at))
            .collect();
        assert_eq!(
            Archive::to_write(Some(chain), many).left_out_before,
            Some((50, "x".into()))
        );
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
