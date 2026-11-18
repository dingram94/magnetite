# Text and languages

Spell check and the languages websites are asked for are Chromium's own.
Magnetite has their settings, in Settings → General → "Text and languages".
Neither is synced.

| Setting | Values | Default |
|---|---|---|
| "Check spelling as you type" | on, off | on |
| "Languages websites are asked for" | language codes in the order of preference, or none | none: the languages of the system |

## Spell check

- On, the text fields of web pages mark the words the spell checker does
  not know. On macOS that is the system's spell checker, with the
  languages chosen in the system.
- The switch counts at once, in every profile; a field that is being
  typed in follows with the next page.

## Languages

- The languages are language codes such as `de`, `en-US`, `es-419` or
  `zh-Hant-TW` (a language, then if wanted a script, a region and variants),
  typed with commas between and saved when the field is left. Up to ten
  are kept, each once. While one of them is no language code, or there are
  more than ten, nothing is saved and the field says why: the whole list
  is saved or none of it.
- Websites get them as the `Accept-Language` header, in that order, and
  read them as `navigator.languages`. A website with several languages
  can choose by them; many answer in the first one they know, and some
  go by a choice of their own, such as the one of an account.
- An empty field gives websites the languages of the system, as before
  there was a setting.
- A change counts from the next start of Magnetite: the engine takes the
  languages when it starts, for every profile.

## Not built yet

- Choosing the languages of the spell checker on Linux and Windows, where
  Chromium has dictionaries of its own.
- Choosing the languages from a list of names.

Code: `crates/engine/src/languages.rs`, `crates/engine/src/bootstrap.rs`
and `engine.rs` (`accept_language_list`), `crates/app/src/main.rs`.
