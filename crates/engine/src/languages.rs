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
/// of two or three letters, then, each at most once and in this order, a
/// script of four letters and a region of two letters or three digits
/// (`en-US`, `zh-Hant-TW`, `es-419`), then variants (`de-1996`). Extensions
/// and private parts, which start with a single letter, are none.
fn is_tag(tag: &str) -> bool {
    let letters = |p: &str, n: usize| p.len() == n && p.bytes().all(|b| b.is_ascii_alphabetic());
    let mut parts = tag.split('-').peekable();
    let language = parts.next().unwrap_or_default();
    if !(letters(language, 2) || letters(language, 3)) {
        return false;
    }
    parts.next_if(|p| letters(p, 4));
    parts.next_if(|p| letters(p, 2) || (p.len() == 3 && p.bytes().all(|b| b.is_ascii_digit())));
    // Variants, each once whatever its case.
    let mut seen: Vec<&str> = Vec::new();
    parts.all(|p| {
        let b = p.as_bytes();
        let variant = b.iter().all(u8::is_ascii_alphanumeric)
            && ((5..=8).contains(&b.len()) || (b.len() == 4 && b[0].is_ascii_digit()));
        let again = seen.iter().any(|s| s.eq_ignore_ascii_case(p));
        seen.push(p);
        variant && !again
    })
}

impl LanguagePrefs {
    /// Only language tags, each once, the first `MAX_LANGUAGES` of them.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        let mut kept: Vec<String> = Vec::new();
        for tag in std::mem::take(&mut self.languages) {
            if kept.len() == MAX_LANGUAGES {
                break;
            }
            let trimmed = tag.trim();
            if !is_tag(trimmed) || kept.iter().any(|k| k.eq_ignore_ascii_case(trimmed)) {
                continue;
            }
            // The string that came in, unless it had spaces around it.
            let cut = (trimmed.len() != tag.len()).then(|| trimmed.to_string());
            kept.push(cut.unwrap_or(tag));
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
}

/// Turn spell check on or off in `ctx`. False if Chromium refused (a context
/// that is not initialised yet does; it gets it in `prepare_context`).
pub(crate) fn apply_spellcheck(ctx: &RequestContext, on: bool) -> bool {
    set_bool_pref(ctx, SPELLCHECK, on)
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
            apply_spellcheck(&ctx, prefs.spellcheck);
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
                "en-12",
                "es-419",
                "de-1996",
                "en-US-US",
                "sl-rozaj-ROZAJ",
                "e",
                "en\r\nX-Evil: 1",
                "english",
            ]
            .map(String::from)
            .to_vec(),
        };
        assert_eq!(
            prefs.sanitised().languages,
            ["de", "en-US", "zh-Hant-TW", "es-419", "de-1996"]
        );
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
