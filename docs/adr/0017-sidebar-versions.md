# ADR-0017: An older version keeps what a newer one wrote in the sidebar

- Status: accepted (2026-11-26).

## Context

The sidebar is one collection, written whole (ADR-0005). A device reads it
into the model's types and writes it from them. A field the device's version
does not know was dropped on reading, and gone for every device once that
device wrote: an older desktop in a chain removed what a newer one added (a
profile's colour, a space's own icon, the spaces a favorite shows in).

Versions in one chain differ for days after every release, and for longer
once a phone writes the sidebar too: its apps come out on another schedule
than the desktop.

## Options

1. **Make everyone update**: a device tells the chain its version, and a
   device that is too old stops until it is updated. It needs a rule for
   which version is too old for which data, and stops a browser for what is
   a matter of sync.
2. **Write only what changed**: no collection written whole, but changes
   to it. A new protocol and server (ADR-0016, option 2).
3. **Keep what is not known**: each part of the model keeps the fields it
   does not know and writes them back.

## Decision

Option 3, with a limit for what it cannot cover.

- `Profile`, `FavoriteSet`, `Space`, `Folder`, `TabItem`, `SmartFolder` and
  `SidebarSnapshot` have `other`: every field of the JSON object that the
  type does not name, kept as it came (`serde(flatten)`) and written back.
  Nothing reads it. The merge (ADR-0016) works on the JSON object, so these
  fields merge like the rest.
- This covers what a version adds. A field whose meaning depends on another
  one that an older version changes without knowing, or a change to what a
  field means, is not covered. Such a change raises
  `SidebarSnapshot::VERSION`. Every device writes its own version, also
  over a sidebar an older one wrote, and a device that reads a higher
  version than its own neither takes that sidebar nor writes one, and says
  that an update is needed.

## Consequences

- New fields need a default for when they are missing, as before, and no
  longer a release of every client before they are used.
- What a version does not know is kept unchecked. It is never opened,
  shown or run; its size is limited by the collection's.
- A device stores such fields in `state.json` with the rest, and the UI
  pages receive them with the sidebar, where nothing reads them either.
- Versions from before this one still drop unknown fields. The phone apps'
  core is built against the model once more before a phone writes the
  sidebar, and not for every field after.
