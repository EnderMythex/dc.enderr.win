//! French or English, for the features that speak both: from the Discord
//! client's locale, or guessed from a message's words.

use std::sync::LazyLock;

use regex::Regex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Fr,
    En,
}

impl Lang {
    /// From the locale of an interaction (`fr`, `en-US`...).
    pub fn from_locale(locale: &str) -> Self {
        if locale.starts_with("fr") {
            Self::Fr
        } else {
            Self::En
        }
    }

    pub fn pick<T>(self, fr: T, en: T) -> T {
        match self {
            Self::Fr => fr,
            Self::En => en,
        }
    }
}

static FRENCH_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(je|tu|vous|nous|le|la|les|un|une|des|est|salut|merci|comment|pourquoi|stp|svp|où|ça|peux|donne|avoir|besoin|quoi|nouveau|nouvelle|débute|débutant|avec|pour|sur|dans|c'est|qu'|quelqu'un)\b").unwrap()
});
static ENGLISH_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(the|is|are|you|please|hello|hi|how|what|thanks|thank|do|does|can|could|would|i'm|im|need|help|new|beginner|share|and|with|for|what's|whats|guys|any)\b").unwrap()
});
static FRENCH_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[àâçéèêëîïôùûüÿœ]").unwrap());

/// Guesses the language of a message from common words.
pub fn detect(text: &str) -> Lang {
    let french = FRENCH_WORDS.find_iter(text).count() + FRENCH_CHARS.find_iter(text).count() * 2;
    let english = ENGLISH_WORDS.find_iter(text).count();
    if english > french { Lang::En } else { Lang::Fr }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_languages() {
        assert_eq!(detect("salut je débute, tu peux m'aider ?"), Lang::Fr);
        assert_eq!(detect("hi guys, can you help me please"), Lang::En);
        assert_eq!(Lang::from_locale("fr"), Lang::Fr);
        assert_eq!(Lang::from_locale("en-US"), Lang::En);
    }
}
