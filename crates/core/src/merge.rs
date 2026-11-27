//! Merging two sidebars that both grew out of a third: what sync does when
//! two devices changed the sidebar since they last agreed (docs/spec/sync.md,
//! "When two devices changed the sidebar").
//!
//! Everything is told apart by its id. `base` is the sidebar both sides
//! started from, `local` this device's, `remote` the one that reached the
//! server first.
//!
//! - What only one side changed is taken from that side, field by field:
//!   one device renames a tab, the other moves it, and both count.
//! - Where both changed the same thing, `remote` wins: it was written
//!   first. A tab's last use is the later of the two.
//! - What one side added stays. What one side removed goes, unless the
//!   other changed it meanwhile: a change is never lost to a removal.
//! - A folder or space that still holds something stays with it, and so
//!   does a profile that something runs in.
//! - The order is that of the side that reordered; what the other side
//!   added goes next to its neighbours there.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::model::{
    FavoriteSet, Folder, Id, Profile, SidebarNode, SidebarSnapshot, Space, TabItem,
};
use crate::smart::item_key;

/// Where a tab or folder is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Place {
    /// The favorites of a profile.
    Favorites(Id),
    /// The top of a space's pinned tree, its Today list, its bookmarks.
    Pinned(Id),
    Today(Id),
    Bookmarks(Id),
    /// Inside a folder.
    Folder(Id),
}

impl Place {
    fn space(&self) -> Option<&Id> {
        match self {
            Place::Pinned(s) | Place::Today(s) | Place::Bookmarks(s) => Some(s),
            Place::Favorites(_) | Place::Folder(_) => None,
        }
    }
}

/// A tab, or a folder without its children (those have places of their own).
#[derive(Debug, Clone, PartialEq)]
enum Item {
    Tab(TabItem),
    Folder(Folder),
}

/// A sidebar taken apart: every tab and folder by id with its place, and
/// each place's ids in order.
#[derive(Default)]
struct Flat {
    items: HashMap<Id, (Item, Place)>,
    lists: HashMap<Place, Vec<Id>>,
}

impl Flat {
    fn of(snap: &SidebarSnapshot) -> Self {
        let mut flat = Self::default();
        for set in &snap.favorites {
            let place = Place::Favorites(set.profile_id.clone());
            for t in &set.items {
                flat.add(Item::Tab(t.clone()), &place);
            }
        }
        for space in &snap.spaces {
            flat.nodes(&space.pinned, &Place::Pinned(space.id.clone()));
            let today = Place::Today(space.id.clone());
            for t in &space.today {
                flat.add(Item::Tab(t.clone()), &today);
            }
            flat.nodes(&space.bookmarks, &Place::Bookmarks(space.id.clone()));
        }
        flat
    }

    fn nodes(&mut self, nodes: &[SidebarNode], place: &Place) {
        for node in nodes {
            match node {
                SidebarNode::Tab(t) => {
                    self.add(Item::Tab(t.clone()), place);
                }
                SidebarNode::Folder(f) => {
                    // Field by field: the children are not copied only to
                    // be dropped.
                    let shell = Folder {
                        id: f.id.clone(),
                        title: f.title.clone(),
                        icon: f.icon.clone(),
                        children: Vec::new(),
                        smart: f.smart.clone(),
                        other: f.other.clone(),
                    };
                    if self.add(Item::Folder(shell), place) {
                        self.nodes(&f.children, &Place::Folder(f.id.clone()));
                    }
                }
            }
        }
    }

    /// False for an id that is there already: the first one counts.
    fn add(&mut self, item: Item, place: &Place) -> bool {
        let id = match &item {
            Item::Tab(t) => &t.id,
            Item::Folder(f) => &f.id,
        };
        if self.items.contains_key(id) {
            return false;
        }
        self.lists
            .entry(place.clone())
            .or_default()
            .push(id.clone());
        self.items.insert(id.clone(), (item, place.clone()));
        true
    }

    fn list(&self, place: &Place) -> &[Id] {
        self.lists.get(place).map_or(&[], Vec::as_slice)
    }
}

/// `local` and `remote` as one value, field by field: a field only `local`
/// changed since `base` is `local`'s, every other one `remote`'s.
fn fields<T: Clone + PartialEq + Serialize + DeserializeOwned>(
    base: &T,
    local: &T,
    remote: &T,
) -> T {
    if local == base || local == remote {
        return remote.clone();
    }
    if remote == base {
        return local.clone();
    }
    let object = |v: &T| match serde_json::to_value(v) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    };
    let (Some(b), Some(l), Some(mut out)) = (object(base), object(local), object(remote)) else {
        return remote.clone();
    };
    let keys: HashSet<&String> = l.keys().chain(b.keys()).collect();
    for key in keys {
        let (was, here) = (b.get(key), l.get(key));
        if here != was && out.get(key) == was {
            match here {
                Some(v) => out.insert(key.clone(), v.clone()),
                None => out.remove(key),
            };
        }
    }
    serde_json::from_value(Value::Object(out)).unwrap_or_else(|_| remote.clone())
}

/// A tab or folder both sides have.
fn item(base: Option<&Item>, local: &Item, remote: &Item) -> Item {
    match (base, local, remote) {
        (Some(Item::Tab(b)), Item::Tab(l), Item::Tab(r)) => {
            let mut tab = fields(b, l, r);
            // Used on both devices since: the later use counts, so a tab
            // is not archived for an age it has on one of them only.
            if l.last_active_at != b.last_active_at {
                tab.last_active_at = tab.last_active_at.max(l.last_active_at);
            }
            Item::Tab(tab)
        }
        (Some(Item::Folder(b)), Item::Folder(l), Item::Folder(r)) => Item::Folder(fields(b, l, r)),
        // Added on both sides, or a tab on one and a folder on the other.
        _ => remote.clone(),
    }
}

/// One of three values: `local`'s if only `local` changed it since `base`.
fn pick<'a, T: PartialEq>(base: Option<&T>, local: &'a T, remote: &'a T) -> &'a T {
    if Some(remote) == base && Some(local) != base {
        local
    } else {
        remote
    }
}

/// `local` and `remote` lists of things with ids (profiles, spaces without
/// what is in them) as one: each thing merged by `fields`, removed only
/// where the other side left it alone and `needed` does not name it.
fn records<T: Clone + PartialEq + Serialize + DeserializeOwned>(
    base: &[T],
    local: &[T],
    remote: &[T],
    id: fn(&T) -> &Id,
    needed: &HashSet<Id>,
) -> Vec<T> {
    let find = |list: &[T], key: &Id| list.iter().find(|t| id(t) == key).cloned();
    let mut kept: HashMap<Id, T> = HashMap::new();
    for key in remote.iter().chain(local).map(id) {
        if kept.contains_key(key) {
            continue;
        }
        let merged = match (find(base, key), find(local, key), find(remote, key)) {
            (Some(b), Some(l), Some(r)) => Some(fields(&b, &l, &r)),
            (None, _, Some(r)) => Some(r),
            (None, Some(l), None) => Some(l),
            (Some(b), Some(one), None) | (Some(b), None, Some(one)) => {
                (one != b || needed.contains(key)).then_some(one)
            }
            (_, None, None) => None,
        };
        if let Some(merged) = merged {
            kept.insert(key.clone(), merged);
        }
    }
    let ids = |list: &[T]| list.iter().map(|t| id(t).clone()).collect::<Vec<_>>();
    let members: HashSet<Id> = kept.keys().cloned().collect();
    order(&ids(base), &ids(local), &ids(remote), &members)
        .iter()
        .filter_map(|key| kept.remove(key))
        .collect()
}

/// The `members` of a list in order. The side that reordered what both had
/// gives the order (`remote` if both did); what only the other side has
/// goes behind its neighbour there, or first if it has none before it.
fn order(base: &[Id], local: &[Id], remote: &[Id], members: &HashSet<Id>) -> Vec<Id> {
    let common = |list: &[Id], other: &[Id]| -> Vec<Id> {
        (list.iter().filter(|id| other.contains(id)).cloned()).collect()
    };
    let remote_moved = common(remote, base) != common(base, remote);
    let (spine, other) = if remote_moved {
        (remote, local)
    } else {
        (local, remote)
    };
    let mut out: Vec<Id> = (spine.iter().filter(|id| members.contains(*id)).cloned()).collect();
    for (at, id) in other.iter().enumerate() {
        if !members.contains(id) || out.contains(id) {
            continue;
        }
        let behind = (other[..at].iter().rev()).find_map(|n| out.iter().position(|o| o == n));
        out.insert(behind.map_or(0, |p| p + 1), id.clone());
    }
    // In neither list: cannot be, but nothing is dropped for it.
    let mut rest: Vec<&Id> = members.iter().filter(|id| !out.contains(id)).collect();
    rest.sort();
    out.extend(rest.into_iter().cloned());
    out
}

/// The tabs and folders that stay, each with its place.
struct Kept {
    items: HashMap<Id, (Item, Place)>,
}

impl Kept {
    fn of(base: &Flat, local: &Flat, remote: &Flat) -> Self {
        let mut items = HashMap::new();
        for id in local.items.keys().chain(remote.items.keys()) {
            if items.contains_key(id) {
                continue;
            }
            let kept = match (
                base.items.get(id),
                local.items.get(id),
                remote.items.get(id),
            ) {
                (b, Some(l), Some(r)) => Some((
                    item(b.map(|b| &b.0), &l.0, &r.0),
                    pick(b.map(|b| &b.1), &l.1, &r.1).clone(),
                )),
                (None, Some(one), None) | (None, None, Some(one)) => Some(one.clone()),
                // Removed on one side: gone, unless the other changed it.
                (Some(b), Some(one), None) | (Some(b), None, Some(one)) => {
                    (one != b).then(|| one.clone())
                }
                (_, None, None) => None,
            };
            if let Some(kept) = kept {
                items.insert(id.clone(), kept);
            }
        }
        let mut kept = Self { items };
        // Each step keeps more or settles a place for good, so this ends.
        while kept.keep_folders(local, remote) | kept.undo_loops(local, remote) {}
        kept
    }

    /// A folder that was removed on one side stays if something in it
    /// stays: nothing is lost with it. True if one came back.
    fn keep_folders(&mut self, local: &Flat, remote: &Flat) -> bool {
        let mut back = Vec::new();
        for (_, place) in self.items.values() {
            let mut place = place;
            while let Place::Folder(folder) = place {
                if self.items.contains_key(folder) || back.iter().any(|(id, _)| id == folder) {
                    break;
                }
                let Some(found) = (local.items.get(folder)).or_else(|| remote.items.get(folder))
                else {
                    break;
                };
                back.push((folder.clone(), found.clone()));
                place = &found.1;
            }
        }
        let any = !back.is_empty();
        self.items.extend(back);
        any
    }

    /// Two folders moved into each other, one on each side, would hold
    /// each other and hang nowhere: such folders go where `remote` has
    /// them (`local` for one only it has). True if there were any.
    fn undo_loops(&mut self, local: &Flat, remote: &Flat) -> bool {
        let mut looping: Vec<Id> = Vec::new();
        for id in self.items.keys() {
            let mut at = id;
            let mut steps = 0;
            while let Some((_, Place::Folder(up))) = self.items.get(at) {
                steps += 1;
                if up == id {
                    looping.push(id.clone());
                    break;
                }
                if steps > self.items.len() {
                    break;
                }
                at = up;
            }
        }
        let mut any = false;
        for id in looping {
            let side = (remote.items.get(&id)).or_else(|| local.items.get(&id));
            if let (Some((_, place)), Some(kept)) = (side, self.items.get_mut(&id))
                && kept.1 != *place
            {
                kept.1 = place.clone();
                any = true;
            }
        }
        any
    }

    /// The ids in `place`.
    fn members(&self, place: &Place) -> HashSet<Id> {
        (self.items.iter())
            .filter(|(_, (_, at))| at == place)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

/// Puts the sidebar together again.
struct Build<'a> {
    kept: Kept,
    base: &'a Flat,
    local: &'a Flat,
    remote: &'a Flat,
}

impl Build<'_> {
    fn ids(&self, place: &Place) -> Vec<Id> {
        order(
            self.base.list(place),
            self.local.list(place),
            self.remote.list(place),
            &self.kept.members(place),
        )
    }

    fn tabs(&mut self, place: &Place) -> Vec<TabItem> {
        (self.ids(place).iter())
            .filter_map(|id| match self.kept.items.remove(id) {
                Some((Item::Tab(t), _)) => Some(t),
                _ => None,
            })
            .collect()
    }

    fn nodes(&mut self, place: &Place) -> Vec<SidebarNode> {
        let mut nodes = Vec::new();
        for id in self.ids(place) {
            // Taken out as it is placed: nothing is placed twice.
            match self.kept.items.remove(&id) {
                Some((Item::Tab(t), _)) => nodes.push(SidebarNode::Tab(t)),
                Some((Item::Folder(mut f), _)) => {
                    f.children = self.nodes(&Place::Folder(id));
                    one_tab_per_item(&mut f, self.remote);
                    nodes.push(SidebarNode::Folder(f));
                }
                None => {}
            }
        }
        nodes
    }
}

/// Two devices that fill the same smart folder each make a tab for an item
/// the source lists, with ids of their own: one of them stays, `remote`'s
/// if it has one.
fn one_tab_per_item(folder: &mut Folder, remote: &Flat) {
    if folder.smart.is_none() {
        return;
    }
    let mut best: HashMap<String, Id> = HashMap::new();
    for node in &folder.children {
        if let SidebarNode::Tab(t) = node {
            let key = item_key(t).to_string();
            let theirs = remote.items.contains_key(&t.id);
            match best.get(&key) {
                Some(have) if remote.items.contains_key(have) || !theirs => {}
                _ => {
                    best.insert(key, t.id.clone());
                }
            }
        }
    }
    folder.children.retain(|node| match node {
        SidebarNode::Tab(t) => best.get(item_key(t)) == Some(&t.id),
        SidebarNode::Folder(_) => true,
    });
}

/// `local` and `remote` as one sidebar, both having started from `base`
/// (the module's text has the rules). Ids are taken as unique within a
/// sidebar; of two things with one id the first counts.
#[must_use]
pub fn merge(
    base: &SidebarSnapshot,
    local: &SidebarSnapshot,
    remote: &SidebarSnapshot,
) -> SidebarSnapshot {
    let flat = (Flat::of(base), Flat::of(local), Flat::of(remote));
    let kept = Kept::of(&flat.0, &flat.1, &flat.2);
    // A space that still holds something stays.
    let needed: HashSet<Id> = (kept.items.values())
        .filter_map(|(_, place)| place.space().cloned())
        .collect();
    // Spaces without what is in them, field by field: their lists are not
    // copied only to be dropped.
    let shell = |spaces: &[Space]| -> Vec<Space> {
        let empty = |sp: &Space| Space {
            id: sp.id.clone(),
            title: sp.title.clone(),
            icon: sp.icon.clone(),
            icon_image: sp.icon_image.clone(),
            accent: sp.accent,
            profile_id: sp.profile_id.clone(),
            shortcut: sp.shortcut.clone(),
            pinned: Vec::new(),
            today: Vec::new(),
            bookmarks: Vec::new(),
            other: sp.other.clone(),
        };
        spaces.iter().map(empty).collect()
    };
    let mut spaces = records(
        &shell(&base.spaces),
        &shell(&local.spaces),
        &shell(&remote.spaces),
        |sp| &sp.id,
        &needed,
    );
    let mut build = Build {
        kept,
        base: &flat.0,
        local: &flat.1,
        remote: &flat.2,
    };
    for space in &mut spaces {
        space.pinned = build.nodes(&Place::Pinned(space.id.clone()));
        space.today = build.tabs(&Place::Today(space.id.clone()));
        space.bookmarks = build.nodes(&Place::Bookmarks(space.id.clone()));
    }
    // What a newer version wrote on a favorites list, merged like a field.
    let list_other = |snap: &SidebarSnapshot, profile: &Id| {
        (snap.favorites.iter().find(|f| f.profile_id == *profile))
            .map(|f| f.other.clone())
            .unwrap_or_default()
    };
    let mut favorites: Vec<FavoriteSet> = Vec::new();
    for set in remote.favorites.iter().chain(&local.favorites) {
        let id = &set.profile_id;
        if favorites.iter().all(|f| f.profile_id != *id) {
            favorites.push(FavoriteSet {
                profile_id: id.clone(),
                items: build.tabs(&Place::Favorites(id.clone())),
                other: fields(
                    &list_other(base, id),
                    &list_other(local, id),
                    &list_other(remote, id),
                ),
            });
        }
    }
    // Written in this version's format, whichever version wrote the sides.
    let mut merged = SidebarSnapshot {
        favorites,
        spaces,
        other: fields(&base.other, &local.other, &remote.other),
        ..SidebarSnapshot::new()
    };
    // A profile that something still runs in stays, as a space does.
    let needed: HashSet<Id> = merged.profiles_in_use().cloned().collect();
    let profiles: fn(&Profile) -> &Id = |p| &p.id;
    merged.profiles = records(
        &base.profiles,
        &local.profiles,
        &remote.profiles,
        profiles,
        &needed,
    );
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smart::{SmartFolder, SmartSource};

    fn tab(id: &str) -> TabItem {
        TabItem::new(id, format!("https://example.org/{id}"), id)
    }

    fn pinned(id: &str) -> SidebarNode {
        SidebarNode::Tab(TabItem {
            home_url: Some(format!("https://example.org/{id}")),
            ..tab(id)
        })
    }

    fn folder(id: &str, children: Vec<SidebarNode>) -> SidebarNode {
        SidebarNode::Folder(Folder {
            id: id.into(),
            title: id.into(),
            icon: None,
            children,
            smart: None,
            other: crate::Other::new(),
        })
    }

    fn space(id: &str, pinned: Vec<SidebarNode>, today: &[&str]) -> Space {
        Space {
            id: id.into(),
            title: id.into(),
            icon: None,
            icon_image: None,
            accent: None,
            profile_id: "p".into(),
            shortcut: None,
            pinned,
            today: today.iter().map(|t| tab(t)).collect(),
            bookmarks: Vec::new(),
            other: crate::Other::new(),
        }
    }

    fn sidebar(spaces: Vec<Space>) -> SidebarSnapshot {
        SidebarSnapshot {
            profiles: vec![Profile {
                id: "p".into(),
                name: "Default".into(),
                is_default: true,
                ephemeral: false,
                color: None,
                other: crate::Other::new(),
            }],
            favorites: vec![FavoriteSet {
                profile_id: "p".into(),
                items: vec![tab("fav")],
                other: crate::Other::new(),
            }],
            spaces,
            ..SidebarSnapshot::new()
        }
    }

    /// One space: a pinned tab, a folder with one, and two Today tabs.
    fn start() -> SidebarSnapshot {
        sidebar(vec![space(
            "s",
            vec![pinned("p1"), folder("f", vec![pinned("p2")])],
            &["t1", "t2"],
        )])
    }

    fn today(snap: &SidebarSnapshot, space: usize) -> Vec<&str> {
        (snap.spaces[space].today.iter())
            .map(|t| t.id.as_str())
            .collect()
    }

    /// The ids of a tree, a folder's children in brackets behind it.
    fn tree(nodes: &[SidebarNode]) -> String {
        let one = |n: &SidebarNode| match n {
            SidebarNode::Tab(t) => t.id.clone(),
            SidebarNode::Folder(f) => format!("{}[{}]", f.id, tree(&f.children)),
        };
        nodes.iter().map(one).collect::<Vec<_>>().join(" ")
    }

    fn find<'a>(snap: &'a SidebarSnapshot, id: &str) -> &'a TabItem {
        (snap.all_tabs().find(|t| t.id == id)).unwrap_or_else(|| panic!("no tab {id}"))
    }

    fn find_mut<'a>(snap: &'a mut SidebarSnapshot, id: &str) -> &'a mut TabItem {
        fn among<'a>(nodes: &'a mut [SidebarNode], id: &str) -> Option<&'a mut TabItem> {
            nodes.iter_mut().find_map(|n| match n {
                SidebarNode::Tab(t) => (t.id == id).then_some(t),
                SidebarNode::Folder(f) => among(&mut f.children, id),
            })
        }
        let favorites = snap.favorites.iter_mut().flat_map(|f| f.items.iter_mut());
        if let Some(t) = favorites.into_iter().find(|t| t.id == id) {
            return t;
        }
        for space in &mut snap.spaces {
            if let Some(at) = space.today.iter().position(|t| t.id == id) {
                return &mut space.today[at];
            }
            if let Some(t) = among(&mut space.pinned, id) {
                return t;
            }
        }
        panic!("no tab {id}")
    }

    #[test]
    fn a_side_that_changed_nothing_takes_the_other() {
        let base = start();
        let mut other = start();
        other.spaces[0].today.remove(0);
        other.spaces[0].title = "Work".into();
        assert_eq!(merge(&base, &base, &other), other);
        assert_eq!(merge(&base, &other, &base), other);
        assert_eq!(merge(&base, &other, &other), other);
    }

    #[test]
    fn tabs_opened_on_both_devices_all_stay() {
        let base = start();
        let (mut local, mut remote) = (start(), start());
        local.spaces[0].today.insert(0, tab("here"));
        remote.spaces[0].today.insert(0, tab("there"));
        remote.spaces[0].pinned.push(pinned("p3"));
        let merged = merge(&base, &local, &remote);
        assert_eq!(today(&merged, 0), ["there", "here", "t1", "t2"]);
        assert_eq!(tree(&merged.spaces[0].pinned), "p1 f[p2] p3");
    }

    #[test]
    fn a_tab_closed_on_one_device_is_closed() {
        let base = start();
        let (mut local, mut remote) = (start(), start());
        local.spaces[0].today.retain(|t| t.id != "t1");
        remote.spaces[0].pinned.remove(0);
        let merged = merge(&base, &local, &remote);
        assert_eq!(today(&merged, 0), ["t2"]);
        assert_eq!(tree(&merged.spaces[0].pinned), "f[p2]");
    }

    #[test]
    fn a_tab_closed_here_and_used_there_stays() {
        let base = start();
        let (mut local, mut remote) = (start(), start());
        local.spaces[0].today.retain(|t| t.id != "t1");
        find_mut(&mut remote, "t1").last_active_at = Some(50);
        let merged = merge(&base, &local, &remote);
        assert_eq!(today(&merged, 0), ["t1", "t2"]);
        // And the other way round.
        let merged = merge(&base, &remote, &local);
        assert_eq!(today(&merged, 0), ["t1", "t2"]);
        assert_eq!(find(&merged, "t1").last_active_at, Some(50));
    }

    #[test]
    fn each_field_is_taken_from_the_side_that_changed_it() {
        let base = start();
        let (mut local, mut remote) = (start(), start());
        let here = find_mut(&mut local, "p1");
        here.title = "Mine".into();
        here.custom_title = true;
        find_mut(&mut remote, "p1").url = "https://example.org/elsewhere".into();
        local.spaces[0].title = "Work".into();
        remote.spaces[0].icon = Some("🏠".into());
        let merged = merge(&base, &local, &remote);
        let p1 = find(&merged, "p1");
        assert_eq!((p1.title.as_str(), p1.custom_title), ("Mine", true));
        assert_eq!(p1.url, "https://example.org/elsewhere");
        let space = &merged.spaces[0];
        assert_eq!(
            (space.title.as_str(), space.icon.as_deref()),
            ("Work", Some("🏠"))
        );
    }

    #[test]
    fn where_both_changed_the_same_the_one_written_first_wins() {
        let base = start();
        let (mut local, mut remote) = (start(), start());
        find_mut(&mut local, "t1").title = "Here".into();
        find_mut(&mut local, "t1").last_active_at = Some(90);
        find_mut(&mut remote, "t1").title = "There".into();
        find_mut(&mut remote, "t1").last_active_at = Some(70);
        let merged = merge(&base, &local, &remote);
        assert_eq!(find(&merged, "t1").title, "There");
        // Its last use is the later one, whoever wrote first.
        assert_eq!(find(&merged, "t1").last_active_at, Some(90));
    }

    #[test]
    fn a_field_set_back_to_nothing_is_merged_too() {
        let mut base = start();
        find_mut(&mut base, "fav").profile_id = Some("work".into());
        find_mut(&mut base, "fav").panel = true;
        let (mut local, mut remote) = (base.clone(), base.clone());
        let here = find_mut(&mut local, "fav");
        here.profile_id = None;
        here.panel = false;
        find_mut(&mut remote, "fav").title = "Renamed".into();
        let merged = merge(&base, &local, &remote);
        let fav = find(&merged, "fav");
        assert_eq!((fav.profile_id.as_deref(), fav.panel), (None, false));
        assert_eq!(fav.title, "Renamed");
    }

    #[test]
    fn a_tab_moved_on_one_device_and_renamed_on_the_other_is_both() {
        let base = start();
        let (mut local, mut remote) = (start(), start());
        // Here: p1 goes into the folder. There: it gets a name.
        let p1 = local.spaces[0].pinned.remove(0);
        let SidebarNode::Folder(f) = &mut local.spaces[0].pinned[0] else {
            panic!("folder expected")
        };
        f.children.push(p1);
        find_mut(&mut remote, "p1").title = "Named".into();
        let merged = merge(&base, &local, &remote);
        assert_eq!(tree(&merged.spaces[0].pinned), "f[p2 p1]");
        assert_eq!(find(&merged, "p1").title, "Named");
    }

    #[test]
    fn a_tab_pinned_here_and_one_opened_there() {
        let base = start();
        let (mut local, mut remote) = (start(), start());
        assert!(local.spaces[0].pin("t2"));
        remote.spaces[0].today.push(tab("t3"));
        let merged = merge(&base, &local, &remote);
        assert_eq!(tree(&merged.spaces[0].pinned), "t2 p1 f[p2]");
        assert_eq!(today(&merged, 0), ["t1", "t3"]);
        assert!(find(&merged, "t2").home_url.is_some());
    }

    #[test]
    fn a_tab_moved_to_different_places_goes_where_the_first_writer_put_it() {
        let base = sidebar(vec![
            space("s", vec![pinned("p1")], &[]),
            space("a", Vec::new(), &[]),
            space("b", Vec::new(), &[]),
        ]);
        let (mut local, mut remote) = (base.clone(), base.clone());
        let p1 = local.spaces[0].pinned.remove(0);
        local.spaces[1].pinned.push(p1);
        let p1 = remote.spaces[0].pinned.remove(0);
        remote.spaces[2].pinned.push(p1);
        let merged = merge(&base, &local, &remote);
        let trees: Vec<String> = merged.spaces.iter().map(|s| tree(&s.pinned)).collect();
        assert_eq!(trees, ["", "", "p1"]);
    }

    #[test]
    fn the_order_is_that_of_the_device_that_reordered() {
        let base = sidebar(vec![space("s", Vec::new(), &["a", "b", "c", "d"])]);
        let reordered = |extra: Option<(usize, &str)>| {
            let mut snap = base.clone();
            snap.spaces[0].today.swap(0, 3);
            if let Some((at, id)) = extra {
                snap.spaces[0].today.insert(at, tab(id));
            }
            snap
        };
        let mut added = base.clone();
        added.spaces[0].today.insert(2, tab("new"));
        // Reordered here, a tab added there: behind the tab it follows there.
        let merged = merge(&base, &reordered(None), &added);
        assert_eq!(today(&merged, 0), ["d", "b", "new", "c", "a"]);
        // And the other way round.
        let merged = merge(&base, &added, &reordered(None));
        assert_eq!(today(&merged, 0), ["d", "b", "new", "c", "a"]);
        // Both reordered: the one written first, with what is new here.
        let mut local = base.clone();
        local.spaces[0].today.reverse();
        local.spaces[0].today.push(tab("mine"));
        let merged = merge(&base, &local, &reordered(Some((1, "theirs"))));
        assert_eq!(today(&merged, 0), ["d", "theirs", "b", "c", "a", "mine"]);
    }

    #[test]
    fn spaces_added_removed_and_reordered() {
        let base = sidebar(vec![
            space("a", Vec::new(), &["a1"]),
            space("b", Vec::new(), &["b1"]),
            space("c", Vec::new(), &["c1"]),
        ]);
        let (mut local, mut remote) = (base.clone(), base.clone());
        local.spaces.push(space("mine", Vec::new(), &["m1"]));
        local.spaces.remove(1);
        remote.spaces.swap(0, 2);
        remote.spaces.insert(0, space("theirs", Vec::new(), &[]));
        let merged = merge(&base, &local, &remote);
        let ids: Vec<&str> = merged.spaces.iter().map(|s| s.id.as_str()).collect();
        // The new one here follows `c`, as it does here.
        assert_eq!(ids, ["theirs", "c", "mine", "a"]);
        assert_eq!(today(&merged, 2), ["m1"]);
    }

    #[test]
    fn a_removed_space_stays_with_what_was_put_into_it_meanwhile() {
        let base = sidebar(vec![
            space("a", Vec::new(), &["a1"]),
            space("b", vec![pinned("b1")], &["b2"]),
        ]);
        let (mut local, mut remote) = (base.clone(), base.clone());
        local.spaces[1].today.insert(0, tab("new"));
        remote.spaces.remove(1);
        let merged = merge(&base, &local, &remote);
        assert_eq!(merged.spaces.len(), 2);
        // What was in it and unchanged went with it.
        assert_eq!(today(&merged, 1), ["new"]);
        assert_eq!(tree(&merged.spaces[1].pinned), "");
        // Nothing new in it: it goes.
        let merged = merge(&base, &base, &remote);
        assert_eq!(merged.spaces.len(), 1);
    }

    #[test]
    fn a_removed_folder_stays_with_what_was_put_into_it_meanwhile() {
        let base = start();
        let (mut local, mut remote) = (start(), start());
        // There: the folder goes, with p2. Here: p1 moved into it.
        remote.spaces[0].pinned.remove(1);
        let p1 = local.spaces[0].pinned.remove(0);
        let SidebarNode::Folder(f) = &mut local.spaces[0].pinned[0] else {
            panic!("folder expected")
        };
        f.children.insert(0, p1);
        let merged = merge(&base, &local, &remote);
        assert_eq!(tree(&merged.spaces[0].pinned), "f[p1]");
        let merged = merge(&base, &remote, &local);
        assert_eq!(tree(&merged.spaces[0].pinned), "f[p1]");
    }

    #[test]
    fn a_folder_taken_apart_here_keeps_what_was_added_to_it_there() {
        let base = start();
        let (mut local, mut remote) = (start(), start());
        crate::sidebar::dissolve_folder(&mut local.spaces[0].pinned, "f");
        let SidebarNode::Folder(f) = &mut remote.spaces[0].pinned[1] else {
            panic!("folder expected")
        };
        f.children.push(pinned("p3"));
        let merged = merge(&base, &local, &remote);
        assert_eq!(tree(&merged.spaces[0].pinned), "p1 f[p3] p2");
    }

    #[test]
    fn folders_moved_into_each_other_end_where_the_first_writer_has_them() {
        let base = sidebar(vec![space(
            "s",
            vec![folder("f", vec![pinned("p1")]), folder("g", Vec::new())],
            &[],
        )]);
        let into = |outer: &str, inner: &str| {
            let mut snap = base.clone();
            let moved = crate::sidebar::remove_from_tree(&mut snap.spaces[0].pinned, inner);
            let SidebarNode::Folder(f) = &mut snap.spaces[0].pinned[0] else {
                panic!("folder expected")
            };
            assert_eq!(f.id, outer);
            f.children.extend(moved);
            snap
        };
        let merged = merge(&base, &into("g", "f"), &into("f", "g"));
        assert_eq!(tree(&merged.spaces[0].pinned), "f[p1 g[]]");
    }

    #[test]
    fn favorites_and_bookmarks_merge_like_tabs() {
        let mut base = start();
        base.spaces[0].bookmarks = vec![folder("bf", vec![SidebarNode::Tab(tab("b1"))])];
        let (mut local, mut remote) = (base.clone(), base.clone());
        local.favorites[0].items.push(tab("fav2"));
        local.spaces[0].bookmarks.push(SidebarNode::Tab(tab("b2")));
        remote.favorites[0].items.insert(0, tab("fav0"));
        remote.favorites.push(FavoriteSet {
            profile_id: "work".into(),
            items: vec![tab("wfav")],
            other: crate::Other::new(),
        });
        let SidebarNode::Folder(f) = &mut remote.spaces[0].bookmarks[0] else {
            panic!("folder expected")
        };
        f.children.clear();
        let merged = merge(&base, &local, &remote);
        let ids = |set: &FavoriteSet| set.items.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&merged.favorites[0]), ["fav0", "fav", "fav2"]);
        assert_eq!(ids(&merged.favorites[1]), ["wfav"]);
        assert_eq!(tree(&merged.spaces[0].bookmarks), "bf[] b2");
    }

    #[test]
    fn profiles_merge_by_id() {
        let profile = |id: &str, name: &str| Profile {
            id: id.into(),
            name: name.into(),
            is_default: false,
            ephemeral: false,
            color: None,
            other: crate::Other::new(),
        };
        let mut base = start();
        base.profiles.push(profile("work", "Work"));
        base.profiles.push(profile("old", "Old"));
        let (mut local, mut remote) = (base.clone(), base.clone());
        local.profiles[1].name = "Job".into();
        local.profiles.push(profile("mine", "Mine"));
        remote.profiles[1].color = Some(crate::Color::rgb(1, 2, 3));
        remote.profiles.remove(2);
        let merged = merge(&base, &local, &remote);
        let names: Vec<&str> = merged.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Default", "Job", "Mine"]);
        assert_eq!(merged.profiles[1].color, Some(crate::Color::rgb(1, 2, 3)));

        // Removed on one device, put to use on the other: it stays. One
        // nothing runs in still goes.
        let (mut local, mut remote) = (base.clone(), base.clone());
        local.profiles.retain(|p| p.id == "p");
        remote.spaces[0].profile_id = "work".into();
        let merged = merge(&base, &local, &remote);
        assert_eq!(merged.spaces[0].profile_id, "work");
        let ids: Vec<&str> = merged.profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["p", "work"]);
    }

    #[test]
    fn a_smart_folder_filled_on_two_devices_has_one_tab_per_item() {
        let smart = |children: Vec<SidebarNode>| {
            SidebarNode::Folder(Folder {
                id: "smart".into(),
                title: "Reviews".into(),
                icon: None,
                children,
                smart: Some(SmartFolder::new(SmartSource::YouTrack {
                    url: "https://example.org".into(),
                    query: "for: me".into(),
                })),
                other: crate::Other::new(),
            })
        };
        let item = |id: &str, url: &str| {
            SidebarNode::Tab(TabItem {
                home_url: Some(url.into()),
                ..TabItem::new(id, url, id)
            })
        };
        let base = sidebar(vec![space("s", vec![smart(Vec::new())], &[])]);
        let with = |children| sidebar(vec![space("s", vec![smart(children)], &[])]);
        let local = with(vec![
            item("here-1", "https://example.org/1"),
            item("here-2", "https://example.org/2"),
        ]);
        let remote = with(vec![item("there-1", "https://example.org/1")]);
        let merged = merge(&base, &local, &remote);
        assert_eq!(tree(&merged.spaces[0].pinned), "smart[there-1 here-2]");
    }

    #[test]
    fn fields_this_version_does_not_know_are_merged_like_the_others() {
        let mut base = start();
        find_mut(&mut base, "t1")
            .other
            .insert("note".into(), "old".into());
        let (mut local, mut remote) = (base.clone(), base.clone());
        // There, a newer version changed its field; here, the tab got a name.
        find_mut(&mut remote, "t1")
            .other
            .insert("note".into(), "new".into());
        remote.spaces[0]
            .other
            .insert("wallpaper".into(), "hills".into());
        let SidebarNode::Folder(f) = &mut remote.spaces[0].pinned[1] else {
            panic!("folder expected")
        };
        f.other.insert("stripe".into(), "red".into());
        remote.favorites[0].other.insert("shelf".into(), 2.into());
        remote.other.insert("mood".into(), "calm".into());
        find_mut(&mut local, "t1").title = "Named".into();
        let merged = merge(&base, &local, &remote);
        let t1 = find(&merged, "t1");
        assert_eq!(
            (t1.title.as_str(), &t1.other["note"]),
            ("Named", &"new".into())
        );
        assert_eq!(merged.spaces[0].other["wallpaper"], "hills");
        let SidebarNode::Folder(f) = &merged.spaces[0].pinned[1] else {
            panic!("folder expected")
        };
        assert_eq!(f.other["stripe"], "red");
        assert_eq!(merged.favorites[0].other["shelf"], 2);
        assert_eq!(merged.other["mood"], "calm");
    }

    #[test]
    fn an_id_that_is_there_twice_counts_once() {
        let base = start();
        let mut remote = start();
        remote.spaces[0].today.push(tab("p1"));
        remote.spaces[0].today.push(tab("t1"));
        let merged = merge(&base, &base, &remote);
        assert_eq!(today(&merged, 0), ["t1", "t2"]);
        assert_eq!(tree(&merged.spaces[0].pinned), "p1 f[p2]");
    }
}
