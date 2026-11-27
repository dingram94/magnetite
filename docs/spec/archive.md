# Archive and auto-archive

Closing a Today tab does not delete it: the tab moves to the archive, a
list of closed tabs that can be searched and restored. Today tabs that are
no longer used move there on their own. With sync on, the devices of a chain
share the archive (below, "On other devices").

## What is archived

- Closing a Today tab: `Cmd+W`, the close button of the row, middle-click,
  or Archive in the context menu.
- **Clear** on the divider above Today, or the command "Archive all Today
  tabs": every Today tab of the current space.
- Auto-archive (below).
- Deleting a space: its pinned and Today tabs. A pinned one comes back as a
  Today tab.
- Removing a pinned tab: the minus on its row or a middle click (a tab
  without a loaded page, at its pinned page, and not a smart folder's own
  item), or "Remove from pinned tabs" in its menu (`sidebar.md`). It is
  archived at its pinned page and comes back as a Today tab.

Not archived:

- Favorites and pinned tabs. Closing one unloads its page and keeps the
  item.
- Tabs of a private profile. They close without a record (`spaces.md`).
  They leave no history either (`history.md`).
- Magnetite's own pages (Settings, the welcome page). They just close.
- A closed peek (`peek.md`).

An archived tab leaves its split view, and its page is closed. After
closing the active tab, the next tab of the space becomes active, or the
previous one if it was the last.

## Auto-archive

Once a minute Magnetite archives every Today tab that has been off screen
for longer than the auto-archive period. A tab that was never shown counts
from its creation, an imported tab from the import. The pages on screen (all
of a split's) and the tab each other space returns to are never archived.

| Setting | Options | Default |
|---|---|---|
| Settings → Tabs and archive → "Archive Today tabs after" | 12 hours, 24 hours, 7 days, 30 days, Never | 12 hours |

With "Never" no tab is archived by itself; Today tabs stay until they are
closed. Their pages are still unloaded (`sidebar.md`). Auto-archive skips the active tab of each space,
tabs with media, playing or paused, and tabs kept active (`sidebar.md`).
The period is one of the settings that sync (`sync.md`).

## Finding and restoring

- `Cmd+Shift+T`, History → Reopen Closed Tab, or the command "Reopen last
  archived tab" restores the tab archived last on this device. A tab
  closed on another device comes back from the archive view or the command
  bar.
- The archive view replaces the sidebar. It opens from the archive and
  downloads button in the footer or with the command "Open archive". It
  lists the newest 300 archived tabs by day, with their space, and has a
  search field that matches title and address. A click restores a tab;
  the × on its row removes it from the archive for good.
- The command bar finds archived tabs among its results (`command-bar.md`).
- Settings → Tabs and archive shows the number of archived tabs.

A restored tab returns to the top of Today in the space it came from, or in
the current space if that space is gone. It becomes the active tab and its
record leaves the archive.

The footer button is shared with the bookmarks, the history and the
downloads. A click opens the view chosen in Settings → Tabs and archive (the
archive by default), or the downloads while a download runs; resting on the
button offers all four and, below a line, Settings.

The downloads have their own spec: [downloads.md](downloads.md).

## Data

The archive is the `archive` list in `<data-dir>/state.json`, oldest first.
Each record holds the tab (address, title, a custom name, times of creation
and last use, home page and profile if it had them), the id of its space and
the time it was archived. Icons are not part of it; the view uses the icon
cache (`sidebar.md`). Next to it, `closed_here` names the last 100 records
this device archived (tab id and time), for "Reopen Closed Tab". The file is
readable by its owner only.

## How long records stay

A record leaves the archive when its tab is restored or removed by hand, or
once it is older than the retention period: the minute tick deletes those.
With "Forever" there is no period, and records only leave by hand.

| Setting | Options | Default |
|---|---|---|
| Settings → Tabs and archive → "Keep archived tabs for" | 30, 60 or 90 days, a number of days from 1 to 3650, or Forever | 90 days |

A typed number is saved on Enter or when the field is left. A shorter period
deletes older records within a minute, and they cannot be brought back. The
period is one of the settings that sync (`sync.md`): a record deleted for
its age on one device is deleted on the others, so they keep records
equally long. Settings written by a version from before the archive synced
do not name a period, and leave this device's as it is.

## On other devices

With sync on (`sync.md`), a tab closed on one device can be brought back on
any other of the chain.

- The archive is a collection of its own in the chain, encrypted like the
  rest. It is written with the round after it changed, and taken when the
  chain's is newer.
- Two devices' archives are merged, never one replaced by the other. Each
  device keeps which records it and the chain last agreed on. What was
  closed on either device since is in the archive of both; a tab brought
  back or removed from the archive on one device leaves it on the others.
  A device that just joined keeps its own records and gets the chain's.
- A tab brought back returns to Today on the device that did it, and
  reaches the others with the sidebar.
- The chain holds the newest 2000 records, about 1 MiB as they are sent:
  every change writes all of them. A device may hold more (with "Forever",
  or a long period); the older ones stay on it, and are not deleted for no
  longer being in the chain. Once records were left out for room, the
  chain says from which record on it is complete (by time, then tab), and
  every later write keeps saying so: a record before that is not taken as
  removed for missing there.
- Not sent, as with the sidebar: tabs of a space kept on this device, also
  after the space was deleted, and tabs that ran in a profile kept on it.
  They stay in this device's archive, and such records from another device
  are not taken. Tabs of private profiles are never archived.
- Records that arrive are checked like tabs that arrive: an address
  Magnetite does not open from stored data is dropped, times are no later
  than tomorrow, and each comes back as a plain Today tab. A record names
  its space by id; brought back where that space is not, it opens in the
  current one.
- Fields a newer version wrote are kept, and an archive in a newer format
  than this version knows is neither taken nor written over, as with the
  sidebar (`sync.md`, "Versions of Magnetite in one chain").

Code: `crates/core/src/archive.rs` (the record, merging two archives),
`crates/app/src/state/adopt.rs` (`archive_for_sync`, `adopt_archive`),
`crates/app/src/state/tabs.rs` (`archive_tab`, `delete_space`,
`prune_archive`), `crates/app/src/app/tabs.rs` (`close_tab`, `housekeeping`,
`reopen_closed`), `crates/ui/web/src/ArchiveView.tsx`,
`crates/ui/web/src/LibraryButton.tsx`.
