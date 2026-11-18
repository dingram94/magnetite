//! Spell check and the languages websites are asked for: Chromium's own.
//! Spell check is a preference of every profile. The languages are given
//! to CEF when it starts and with every profile it makes
//! (`accept_language_list`): CEF sets Chromium's preference from that
//! itself and puts it back when the preference is changed under it, so a
//! change counts from the next start.

use cef::*;
use serde::{Deserialize, Serialize};

use crate::autofill::set_bool_pref;
use crate::engine::Engine;

/// The most languages kept.
const MAX_LANGUAGES: usize = 10;

/// What the app's settings keep about text and languages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LanguagePrefs {
    /// Check spelling in text fields as the user types.
    pub spellcheck: bool,
    /// The languages websites are asked for, the preferred one first, as
    /// language tags (`de`, `en-US`). Empty: Chromium's own choice, the
    /// system's languages.
    pub languages: Vec<String>,
}

impl Default for LanguagePrefs {
    fn default() -> Self {
        Self {
            spellcheck: true,
            languages: Vec::new(),
        }
    }
}

const SPELLCHECK: &str = "browser.enable_spellchecking";

/// Whether `tag` is a language tag as websites are sent them: a language
/// of two or three letters, then parts of two to eight letters and digits
/// (`en-US`, `zh-Hant-TW`). A part of one letter starts an extension, which
/// no website is asked for.
fn is_tag(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let language = parts.next().unwrap_or_default();
    (2..=3).contains(&language.len())
        && language.bytes().all(|b| b.is_ascii_alphabetic())
        && parts.all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

impl LanguagePrefs {
    /// Only language tags, each once, the first `MAX_LANGUAGES` of them.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        let mut kept: Vec<String> = Vec::new();
        for tag in std::mem::take(&mut self.languages) {
            let tag = tag.trim().to_string();
            let known = kept.iter().any(|k| k.eq_ignore_ascii_case(&tag));
            if is_tag(&tag) && !known && kept.len() < MAX_LANGUAGES {
                kept.push(tag);
            }
        }
        self.languages = kept;
        self
    }

    /// The languages as CEF takes them (`accept_language_list`): the tags
    /// with commas between. Empty for Chromium's own choice.
    #[must_use]
    pub fn accept_languages(&self) -> String {
        self.languages.join(",")
    }

    /// Set spell check on `ctx`. False if Chromium refused (a context that
    /// is not initialised yet does; it gets it in `prepare_context`).
    pub(crate) fn apply(&self, ctx: &RequestContext) -> bool {
        set_bool_pref(ctx, SPELLCHECK, self.spellcheck)
    }
}

impl Engine {
    /// Use `prefs` from now on: spell check in every profile that exists,
    /// and in those made later. The languages count from the next start
    /// (`EngineConfig::accept_languages`).
    pub fn set_languages(&self, prefs: &LanguagePrefs) {
        let contexts: Vec<RequestContext> = {
            let mut inner = self.lock();
            inner.languages.clone_from(prefs);
            inner.contexts.values().cloned().collect()
        };
        for ctx in request_context_get_global_context()
            .into_iter()
            .chain(contexts)
        {
            prefs.apply(&ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LanguagePrefs;

    #[test]
    fn only_language_tags_are_kept_each_once() {
        let prefs = LanguagePrefs {
            spellcheck: true,
            languages: [
                "de",
                " en-US ",
                "EN-us",
                "zh-Hant-TW",
                "",
                "x",
                "de;q=0.5",
                "en-a",
                "en\r\nX-Evil: 1",
                "english",
            ]
            .map(String::from)
            .to_vec(),
        };
        assert_eq!(prefs.sanitised().languages, ["de", "en-US", "zh-Hant-TW"]);
        let many = LanguagePrefs {
            spellcheck: false,
            languages: (0..40)
                .map(|n| format!("a{}", (b'a' + n % 26) as char))
                .collect(),
        };
        let kept = many.sanitised();
        assert_eq!(kept.languages.len(), 10);
        assert!(!kept.spellcheck);
        // Spell check is Chromium's own until turned off, the languages the system's.
        assert_eq!(
            LanguagePrefs::default(),
            LanguagePrefs {
                spellcheck: true,
                languages: vec![]
            }
        );
    }
}
