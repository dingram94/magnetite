# Settings

Settings are a page of Magnetite's own, `magnetite://ui/settings`, opened as
a tab. A list of sections on the left, the settings of the chosen section on
the right. What each setting does is in the spec of its feature.

## Sections

The list has four groups:

| Group | Sections |
|---|---|
| Basics | General, Appearance, Shortcuts |
| Browsing | Tabs & archive, Links, Search, Ad blocking, Permissions, Passwords & autofill |
| Spaces | Isolation, Sync, Smart folders |
| Advanced | Extensions, Custom CSS & JS, Developer, Data & privacy, About |

- **General** has what belongs to no feature of its own: windows (several
  windows, web panels, playing videos), where downloads are saved, the
  quick search from anywhere and the welcome page.
- **Links** has what happens to a link from another app (the default
  browser, where such a link opens, the sites whose links open in a space
  of their own) and to a link that is copied (trackers removed).
- **Tabs & archive** has when tabs are archived and suspended, how long the
  archive keeps them, the order of Ctrl+Tab and what the archive button
  opens.
- **Permissions** lists every site with a choice of its own for the
  camera, microphone, location, notifications, pop-ups or the clipboard
  (`site-controls.md`).
- **Data & privacy** has the history (how long it is kept, clearing it),
  deleting the cookies, site data and cache of all websites
  (`site-controls.md`),
  the import from another browser or a bookmarks file, the servers
  Magnetite talks to, each with what for, what websites are told
  (`global-privacy-control.md`), and where the data lives.

The address names the section: `magnetite://ui/settings#links`.

## How a section looks

Settings that belong together stand in one card, a group, under its name
where it has one, with a line about the group where that helps. Each
setting is a row of the card: its name and a description, and its control.
A short control (a switch, a choice from a list, one or two buttons) sits
to the right of the name; a wide one (a list to edit, a text to type)
below it. A choice of two or three short answers is a row of buttons, one
of more or longer answers a list to open.

The groups of Shortcuts fold: a click on a group's name shows or hides its
commands, and the name says how many there are. The first group is open
when the section opens, the others closed. A search shows the commands of
a folded group all the same.

## Search

The field "Search settings" above the list of sections finds a setting
without knowing its section.

- While the field holds text, the page shows every section that holds a
  match instead of one section, each under its name. The settings that
  match look as always; the other settings of these sections are greyed
  out, and so are the sections without a match in the list on the left. No
  section is marked there.
- A setting matches when every typed word is in its name or its
  description; the words are marked where they stand in a setting that
  was found. Upper and lower case make no difference. The name of a
  group ("Windows") matches all its settings.
- A section whose name matches comes first, with nothing greyed out.
- The shortcuts match one by one, by the name of the command; the name of
  a group ("Tabs") matches the whole group.
- All settings shown work where they are, greyed out or not: a switch can
  be flipped in the results.
- A click on a section's name in the results, or on a section in the list,
  opens that section alone and keeps the search: its settings that match
  look as always, the others are greyed out, and the list on the left
  still greys out the sections without a match. A section without any
  match shows all its settings greyed out. "Everything for …" above the
  section goes back to the results, and so does typing.
- Esc in the field empties it and shows the section shown last.
- Nothing found: "No setting matches …".

What is searched is what the sections show, so the names of your spaces,
sites and extensions are found where a section lists them. Nothing typed
leaves the page.

## Where the code is

`crates/ui/web/src/SettingsPage.tsx` (sections, the field),
`crates/ui/web/src/settings/SearchResults.tsx` (matching),
`crates/ui/web/src/settings/fields.tsx` (groups, rows and their controls).

## Not built yet

- A shortcut that puts the cursor into the field.
