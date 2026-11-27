# Sync

Sync keeps the sidebar (profiles, favorites, spaces with their pinned and
Today tabs) and a few settings the same on several devices. It is optional
and off until you turn it on. There is no account: a 24-word sync code is
the only key, and the devices that hold it form a sync chain. The data is
encrypted on the device, so the server sees only an anonymous key and padded
ciphertext. The server is one you can host yourself; Magnetite also runs a
free one at `sync.magnetitebrowser.org`.

This page covers the browser's side. The server (one binary and a Postgres
database), its configuration, limits and API are documented in its own
repository, <https://github.com/magnetite-browser/magnetite-sync>
(`README.md`, `docs/design.md`). Background: `docs/adr/0005-sync.md`,
`docs/adr/0008-public-sync-server.md`, `docs/design/security.md`.

## Setup (Settings → Sync)

1. Server: "Magnetite Sync" (`https://sync.magnetitebrowser.org`,
   preselected, no sign-up) or "Your own server" with its address. It needs
   `https://`; plain `http://` is accepted only for `localhost`, `127.0.0.1`
   and `::1`. A path prefix is fine; a user name or a query string is not.
2. First device: "Start syncing" (with your own server: plus an invite code
   if it asks for one). Magnetite creates the sync code, shows its 24 words
   and asks you to keep them. Nobody can recover them for you.
3. Other devices: paste the 24 words and "Join"; case and spacing do not
   matter. A device that joins takes the chain's sidebar in place of its
   own (see "Taking another device's data"). While the chain holds nothing
   yet, it waits and says so: only the device that started the chain writes
   first.
   A device that is in a chain cannot start or join another before it
   leaves.
4. While syncing, the page shows the server, the time of the last sync and
   the last error, with "Sync now" and "Show sync code".
   The shown code has a QR code beside its words, which holds the code and
   the server for Magnetite's phone apps (see "The code as a QR code").
5. "Leave on this device" forgets the code here; the chain and the other
   devices stay as they are. "Delete sync chain…" (click twice) also erases
   the chain on the server for every device.

## The code as a QR code

Beside the 24 words, the page draws a QR code that holds the link

    magnetite://sync?server=<address of the server>&words=<the 24 words>

with both values percent-encoded and the words separated by `%20`. It is
there for Magnetite's phone apps (`magnetite-ios`, `magnetite-android`):
their "Scan QR Code" step, where a chain is joined, reads the link and joins
with it, so neither the words nor the address of an own server are typed.
The browser itself does not read the link, and a phone app without that
step joins with the words, as a computer does. The QR code is as secret as
the words and shows only while they show. It is made on the device; a link is
used and not plain text, so a phone's camera app does not offer to search
the web for the words.

## When and how it syncs

- Shortly after start, then every 5 minutes, on "Sync now" and right after
  starting or joining a chain.
- Each round handles the two collections in turn. It asks the server first:
  a newer version there is taken. Otherwise the device writes its own copy
  as the next version, but only when it has something new: not when the
  server holds the same, and not when the sidebar is still as the device
  and the chain last agreed on it (apart from what a device keeps of its
  own, see below). So a device left running does not write over what
  another one changed while offline.
- What is sent is the saved sidebar without the tabs other devices would
  not take: Magnetite's own pages and local files.
- A sidebar that arrives while this device has changes of its own is
  merged with them (next section), and the device writes the result right
  after, without waiting for the next round. When another device wrote in
  between, it reads again first.
- The settings are replaced as a whole: of two devices that changed them
  between syncs, the one that syncs second takes the other's.
- Versions only go up. A write counts once the server confirms exactly the
  version written, and a server copy older than what the device has seen is
  refused, so a server cannot roll a device back.

## When two devices changed the sidebar

Each device keeps the sidebar it and the chain last agreed on: the one it
took, or the one it wrote. What differs from that is what the device
changed since, also before a restart. It is saved like the device's own
sidebar: nothing of a profile that is private here goes to disk with it. When a newer sidebar arrives, those
changes go into it, and nothing is lost for having synced second:

- Everything is told apart by its id: spaces, folders, tabs, favorites,
  bookmarks, profiles. What only one device changed is taken from it, field
  by field. A tab renamed on one device and moved on the other is renamed
  and moved.
- Where both changed the same thing (the same tab's title, the place of the
  same tab), the device that synced first wins. A tab's last use is the
  later of the two, so a tab is not archived for an age it has on one
  device only.
- What one device added stays: tabs opened on both are all there.
- What one device removed goes, unless the other changed it meanwhile: a
  tab closed on one device and used on the other stays open.
- A folder or space removed on one device stays if the other put something
  into it or changed something in it meanwhile, with just that. So does a
  profile removed on one device that the other put to use.
- Each device removed the spaces the other kept: a sidebar cannot be without
  one, so the chain's sidebar is taken as it arrives, as where both changed
  the same thing.
- The order of a list is that of the device that reordered it (of the one
  that synced first, if both did). What the other device added goes behind
  the tab it follows there, or first.
- Two folders moved into each other, one on each device, end where the
  device that synced first has them.
- Two devices that fill the same smart folder each make a tab for an item
  the source lists: one of the two stays.

The merged sidebar is taken like any that arrives ("Taking another device's
data"). A device that never agreed with the chain, because it just joined,
has nothing to tell its own changes by and takes the chain's sidebar.

## What syncs

| Collection | Content |
|---|---|
| `sidebar` | profiles (name, colour), favorites, spaces (name, glyph or own icon, colour) with their pinned trees (folders, smart folder definitions), Today tabs and bookmarks |
| `settings` | when Today tabs are archived, when inactive tabs are suspended, where links from other apps open (a tab, a peek or a mini window; a version that does not know a choice uses a new tab), the shortcuts with other keys or none, who gets their keys first, and the sites that get them all first (`shortcuts.md`) |
| `devices` | per device: the tabs open on it that it lists for the others (phones do), which tabs of other devices it took over, a phone's changes to the sidebar, and which of those a desktop made |

The server never sees these names, only ids derived from the sync code.

## Tabs of other devices

A phone reads the sidebar and does not write it: it knows only part of
what a desktop keeps there, and what it wrote would be without the rest.
What a phone has open travels in `devices` instead, where every device changes only its own entry and writes the
collection as the version after the one it read; when another device wrote
first, it reads again. Devices cannot overwrite each other there.

- A tab open on another device shows in the Today list of the space it is
  open in there (in the space in front if this device does not have that
  one), below this device's tabs, with the device's icon on the right, which
  gives way to "Open here" on hover. It is not a tab here: it has no page and
  is not in the sidebar that syncs.
- A click takes it over: it opens here in front, as the last Today tab, where
  its row was, and this device notes its id as taken. The other device sees
  that at its next sync, closes the tab and stops listing it; this device
  then forgets the id.
- A tab another device took is no longer shown here.
- A tab the other device sent here ("Send to Desktop") is taken without a
  click: it opens in the background, marked unread. With several desktops
  in the chain it opens on one: a desktop opens it only after its note of
  having taken it is stored, and the others see that note.
- A desktop lists no tabs of its own: its Today tabs are in the sidebar
  already. Its entry holds only what it took, and is gone when that is
  nothing. It names itself by its kind ("Mac", "Linux PC"), never by the
  computer's name.
- Entries not written for 90 days are dropped with the next write. Fields a
  newer version wrote are kept.
- Never in the private space, and not in a space kept on this device
  alone: the tab then opens in the space in front, or, if that is such a
  space too, in the first space that syncs. Only a device none of whose
  spaces sync opens it in the space in front regardless.
- A tab with an address Magnetite does not open from stored data (a local
  file, for example) is not shown and not taken: it stays open on its
  device.

### Changes from phones

A phone cannot write the sidebar either, so what it changes there travels
in `devices` too: it lists its changes in its entry, and a desktop makes
them. The phone shows them at once, marked as waiting, until the sidebar it
reads has them.

- A phone can pin a page in a space (last in its pinned tree), make a page
  a favorite of a space's profile (last in its list), add a space (after
  the others, with the default profile and colour, as "+" → New Space
  does), rename a space, folder, pinned tab or favorite, and remove a
  pinned tab or favorite (it goes to the archive, as when closed here).
- A desktop makes each change once, at its next sync, and then notes the
  change's id in its entry and syncs the sidebar. The phone sees the note
  and stops listing the change; the desktop then forgets the id. The note
  comes only after the change is made, so a change is never lost when the
  server's answer to a sync does not arrive.
- With several desktops in the chain, the others see the note and leave
  the change alone. Two that sync at the same moment may both make it,
  which changes nothing.
- A change that adds something carries the id of what it adds, so it is
  never added twice.
- Not made: an address Magnetite does not open from stored data; a
  change to what does not sync (the private space, a space kept on this
  device alone, what is in them, and favorites of a profile only such
  spaces use); a change for a space deleted meanwhile; renaming or
  removing a Today tab, and removing a folder; and a kind of change this
  version does not know (it stays listed, for a desktop that does).

Not synced: the archive, downloads, cookies and logins, which pages are
open, split views, the search engine (a device holding the words could
otherwise send every search to an address of its choice), appearance and
all other settings, pinned extensions, and smart folder accounts and tokens.
Today tabs of a private profile are not synced; its pinned tabs and
favorites sync with the address they return to, not the page they show.

### Only on this device

A space can be kept on one device: Settings → Sync → "Spaces" → "Only on
this device". The choice is that device's own and is not synced, and it can
be made before the device starts or joins a chain. For a work space that
should not reach a private device, turn it on at the work device.

- The device no longer sends the space. A device whose spaces are all kept
  this way sends no sidebar at all and only takes the chain's.
- A sidebar that arrives leaves the space as it is on the device, at its
  place in the list (see below).
- On the other devices the space goes with their next sync.
- A separate profile that only such spaces use stays on the device with
  them: the profile, its favorites, and tabs in other spaces that run in
  it are not sent either. Settings → Spaces marks the space "only on this
  device". On the other devices that profile and its favorites stay, as
  for any profile a sidebar does not name, until they are removed there.
  As soon as a synced space uses the profile it is sent again. The default
  profile always is.
- Turned off again, the space is sent like the rest.

## Taking another device's data

The sidebar from the server, with this device's own changes merged in
("When two devices changed the sidebar"), takes the place of this device's,
with limits so that whoever holds the words cannot harm the device through
it:

- This device's profiles stay, and whether a profile is private, and which
  one is the default, is decided here. A profile only this device has
  keeps its favorites. Cookies and logins of a profile are deleted only
  when the profile is deleted here.
- A profile can be private here and not on another device. That device's
  Today tabs in it show here as private tabs, and what this device writes
  leaves them, and where that device's pinned tabs and favorites in it
  went, on the chain as they are there: closing one of those tabs here
  does not close it there. Nothing of this device's browsing in the
  profile is sent.
- Deleting a profile counts for the device it is done on. The other
  devices keep the profile until it is deleted there, and the device that
  deleted it does not take it back from their list. It returns only with a
  sidebar that uses it (a space, a favorite or a tab runs in it), and then
  empty.
- A space that is only on this device stays untouched, and so do a
  profile that stays with it and that profile's favorites, whatever the
  other device says about them. A copy of such a space that arrives is old
  and is dropped, and this device writes its sidebar so that the other
  devices lose the copy. A tab or folder of such a space that arrives in
  another space was moved there on another device, and is there.
- Tabs the other device never had stay as Today tabs: the one last active
  in each space, and those that are never sent (private Today tabs,
  Magnetite's own pages, tabs in a synced space that run in a profile that
  stays on this device). A private one stays private.
- A page that has to open again in another profile closes and leaves its
  split.
- Pages of tabs that still exist stay open and keep their own address and
  title. Pages of tabs that are gone are closed. Nothing that arrived loads
  before you open it.
- Smart folders added or changed on another device do not fetch until they
  are turned on here (`smart-folders.md`).
- Tabs with addresses Magnetite does not open from stored data are dropped.
  Colours, timestamps, smart folder limits and duplicate ids are checked,
  and settings pass the same checks as on the Settings page.
- A sidebar without spaces is refused, and data this version cannot read is
  not taken. Neither counts as applied; Settings → Sync shows the reason
  after every round until usable data is there. No device writes a sidebar
  without spaces.

## Safety

- Every request is signed with a key derived from the sync code. The
  signature covers the server's host, so no other server can replay it,
  and the time, so the device's clock must be right to within 5 minutes.
- Redirects are not followed. A request gives up after 10 s without a
  connection or 60 s in total. An answer over 32 MiB is refused.
- Errors are shown in Magnetite's own words by status code; text from the
  server never reaches the page.
- Only the Settings page and the welcome page of the first start
  (`first-start.md`) may show the code or start, join, leave or delete a
  chain.
- Leaving and deleting take effect on the device at once, even while a
  request hangs. The deletion on the server follows. Until the server
  confirms it (204, or 404 for a chain already gone), the device keeps the
  code and tries again at its next start.

## Where the data lives

| Where | What |
|---|---|
| `sync.key` in the data folder | the sync code, readable by the owner only (0600); all keys and ids are derived from it |
| `sync.delete` in the data folder | code and server of a chain whose deletion the server has not confirmed yet |
| `settings.sync` in `state.json` | on or off, the server address, the last version per collection, the time of the last sync, the last error |
| `synced_sidebar` in `state.json` | the sidebar this device and the chain last agreed on; gone when the device leaves the chain |
| the server | the chain's public key, the days it was created and last used, collection ids, versions and ciphertext; by default a chain unused for 400 days is deleted (its README, "Data it stores") |

The code is 32 random bytes, shown as 24 BIP39 words. From it the device
derives an Ed25519 signing key (its public half is the chain's id on the
server), an XChaCha20-Poly1305 key and the collection ids. Data is padded to
a power of two, at least 1 KiB, before it is encrypted. This protocol code
is the `magnetite-sync-proto` crate in the server's repository; the browser
builds it from a checkout next to this one.

## Not built yet

- Syncing the archive.

Code: `crates/app/src/sync.rs` (client), `crates/core/src/merge.rs` (merging
two sidebars), `crates/app/src/devices.rs` (tabs
of other devices, phones' changes), `crates/app/src/state/phone_edits.rs`
(making those changes), `crates/app/src/state/adopt.rs`
(`adopt_pull`, `lasting_sidebar`), `crates/app/src/app/syncing.rs` (`sync_now`,
`on_sync_outcome`), `crates/ui/web/src/settings/SyncSection.tsx`.
