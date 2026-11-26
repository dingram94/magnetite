# ADR-0016: Two devices' sidebars are merged

- Status: accepted (2026-11-26). Amends ADR-0005 ("Merge is whole-collection
  last writer wins"), for the sidebar.

## Context

A collection is written whole, as the version after the one the device read
(ADR-0005). A device that finds a newer sidebar on the server took it in
place of its own. What it had changed since its last sync was lost: tabs
opened, pinned or renamed on a second computer within the minutes between
two rounds, and everything changed while a computer was offline.

The sidebar holds the Today tabs too, which change with every page, so two
computers in use at the same time ran into this all the time.

The server is zero knowledge: it sees ciphertext and versions, and cannot
merge anything. The versions give the sidebars one line of history.

## Options

1. **Sync more often.** Makes the loss rarer and keeps it. The public
   server lets a chain write ten times a minute, all its devices together.
2. **A log of changes** in place of the sidebar, replayed by every device
   (as phones' changes to the sidebar travel, `docs/spec/sync.md`). Exact,
   but every way to change the sidebar becomes an operation that every
   version must know, and the collection grows without end or needs
   compacting.
3. **A CRDT.** Merges without a common ancestor, which the versions make
   unnecessary, and wants its own data model next to the one the app and
   the importer use.
4. **A three-way merge on the device**: the sidebar that arrives, the
   device's own, and the one both started from. No change to the server,
   the protocol or the stored format.

## Decision

Option 4.

- The device keeps the sidebar it and the chain last agreed on
  (`synced_sidebar`, saved in `state.json`): what it would have sent right
  after it took a sidebar, or the sidebar it wrote. It is the common
  ancestor.
- `magnetite_core::merge::merge(base, local, remote)` merges by id and
  field. Its rules are in `docs/spec/sync.md`, "When two devices changed
  the sidebar". Where both sides changed the same thing, the sidebar on the
  server wins: it was written first, and other devices may have taken it.
  A change beats a removal, so nothing a user did is lost; the price is
  that something removed on one device can come back.
- Fields are merged as JSON, key by key, so a field added to the model
  later is merged without a change to the merge.
- The merged sidebar passes the same checks as any that arrives
  (`adopt_pull`), and the device writes it as the next version. A device
  that is behind once more reads again and merges again.
- The settings stay last writer wins: they are few, and rarely changed on
  two devices at once.
- Rounds stay five minutes apart. Merging takes the loss out of that time;
  shorter rounds would only bring changes over sooner, against the server's
  write limit.

## Consequences

- A device without the common ancestor (it just joined, or its state is
  from a version before this) takes the chain's sidebar as before, once.
- A phone still does not write the sidebar. Its changes reach it through a
  desktop (`devices`), which now merges them with whatever else changed.
- A version from before this one in the same chain still replaces its own
  sidebar with the one that arrives. It loses its own changes as before,
  and no others.
- `state.json` holds the sidebar twice while the device syncs.
