//! Data structures and schema types for language definition modules.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};

/// Error produced when a language definition module fails semantic validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageValidationError {
    /// Human-readable explanation of why the module is invalid.
    pub reason: String,
}

impl LanguageValidationError {
    /// Creates a new validation failure with the given reason.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for LanguageValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for LanguageValidationError {}

/// Root language module parsed from `languages/<code\>.yaml`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageModule {
    /// Schema format version, must be 1.
    pub format_version: u32,
    /// 2-3 letter lowercase language code matching the file stem.
    pub code: String,
    /// Human-readable language name in its native script.
    pub display_name: String,
    /// Lexical rules: stop words, negations, language/country markers, transliterations.
    pub lexical: LexicalRules,
    /// Deterministic voice command dictionaries.
    pub commands: CommandDictionary,
}

impl LanguageModule {
    /// Performs semantic validation on the module and checks code matches the expected file stem.
    pub fn validate_semantics(&self, expected_code: &str) -> Result<(), LanguageValidationError> {
        if self.format_version != 1 {
            return Err(LanguageValidationError::new(format!(
                "unsupported format_version {}, expected 1",
                self.format_version
            )));
        }
        if self.code != expected_code {
            return Err(LanguageValidationError::new(format!(
                "language code '{}' does not match file stem '{}'",
                self.code, expected_code
            )));
        }
        if self.code.len() < 2
            || self.code.len() > 3
            || !self.code.bytes().all(|b| b.is_ascii_lowercase())
        {
            return Err(LanguageValidationError::new(format!(
                "invalid language code '{}', must be 2-3 lowercase ASCII letters",
                self.code
            )));
        }
        if self.display_name.trim().is_empty() {
            return Err(LanguageValidationError::new("display_name cannot be empty"));
        }

        // Validate country codes are 2 uppercase ASCII letters
        for country in &self.lexical.countries.country_names {
            validate_country_code(&country.code)?;
        }
        for inflection in &self.lexical.countries.inflections {
            validate_country_code(&inflection.code)?;
        }
        for demonym in &self.lexical.demonyms {
            validate_country_code(&demonym.code)?;
        }

        // Validate broadcast language codes
        for lang in &self.lexical.explicit_languages.languages {
            validate_language_code(&lang.code)?;
        }
        for phrase in &self.lexical.explicit_languages.phrases {
            validate_language_code(&phrase.code)?;
        }
        for standalone in &self.lexical.explicit_languages.standalone {
            validate_language_code(&standalone.code)?;
        }

        Ok(())
    }
}

fn validate_country_code(code: &str) -> Result<(), LanguageValidationError> {
    if code.len() != 2 || !code.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(LanguageValidationError::new(format!(
            "invalid country code '{code}', must be 2 uppercase ASCII letters"
        )));
    }
    Ok(())
}

fn validate_language_code(code: &str) -> Result<(), LanguageValidationError> {
    if !(2..=3).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err(LanguageValidationError::new(format!(
            "invalid language code '{code}', must be 2-3 lowercase ASCII letters"
        )));
    }
    Ok(())
}

/// Lexical rules section of a language definition module.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LexicalRules {
    /// Command verbs and filler words removed from search term matching.
    pub stop_words: Vec<String>,
    /// Negation words (e.g. "не", "без", "not", "without").
    pub negations: Vec<String>,
    /// Rules for identifying explicit broadcast-language requests in speech/text.
    pub explicit_languages: ExplicitLanguages,
    /// Rules for identifying origin countries and prepositional phrases.
    pub countries: CountryRules,
    /// Demonyms and nationality modifiers (e.g. "немецкий", "german").
    pub demonyms: Vec<DemonymEntry>,
    /// Station name hint prefixes and vocabulary transliterations.
    pub name_bases: NameBases,
}

/// Broadcast-language identification markers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitLanguages {
    /// Prepositions indicating language (e.g. "на", "in").
    pub prepositions: Vec<String>,
    /// Suffix words indicating language (e.g. "язык", "языке", "language").
    pub language_suffixes: Vec<String>,
    /// Multi-token phrases indicating language (e.g. ["по", "русски"]).
    pub phrases: Vec<ExplicitPhraseEntry>,
    /// Standalone single-word markers (e.g. "русскоязычный", "англоязычный").
    pub standalone: Vec<ExplicitStandaloneEntry>,
    /// Words that veto language interpretation (e.g. "style" in "in French style").
    pub style_exclusions: Vec<String>,
    /// Known broadcast languages and their morphological forms in this language.
    pub languages: Vec<LanguageFormEntry>,
}

/// Explicit language multi-token phrase entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitPhraseEntry {
    /// Resulting ISO 639 language code.
    pub code: String,
    /// Sequence of tokens matching this phrase.
    pub phrase: Vec<String>,
}

/// Explicit language standalone single-token entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitStandaloneEntry {
    /// Resulting ISO 639 language code.
    pub code: String,
    /// Tokens matching this standalone language marker.
    pub terms: Vec<String>,
}

/// Language forms mapping a language code to words in this language.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageFormEntry {
    /// Target language code (e.g. "de", "fr").
    pub code: String,
    /// Forms representing this language (e.g. ["немецком", "немецкий"] or ["german"]).
    pub forms: Vec<String>,
}

/// Country rules for origin detection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountryRules {
    /// Prepositions indicating country origin (e.g. "из", "from").
    pub prepositions: Vec<String>,
    /// Country names in this language.
    pub country_names: Vec<CountryNameEntry>,
    /// Irregular inflections of country names.
    pub inflections: Vec<CountryInflectionEntry>,
}

/// Country name entry mapping ISO 3166-1 alpha-2 code to localized names.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountryNameEntry {
    /// ISO 3166-1 alpha-2 country code.
    pub code: String,
    /// Localized country names.
    pub names: Vec<String>,
}

/// Irregular country inflection entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountryInflectionEntry {
    /// ISO 3166-1 alpha-2 country code.
    pub code: String,
    /// Inflected forms (e.g. "англии", "германии").
    pub forms: Vec<String>,
}

/// Demonym entry mapping ISO 3166-1 alpha-2 code to nationality adjectives.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemonymEntry {
    /// ISO 3166-1 alpha-2 country code.
    pub code: String,
    /// Demonyms and nationality adjectives.
    pub demonyms: Vec<String>,
}

/// Station name extraction bases and transliterations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameBases {
    /// Multi-token prefixes before station names (e.g. [["включи", "радио"], ["play", "radio"]]).
    pub station_prefixes: Vec<Vec<String>>,
    /// Fixed dictionary transliterations between script terms.
    pub word_transliterations: Vec<WordTranslitEntry>,
}

/// Single word transliteration mapping.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WordTranslitEntry {
    /// Source term.
    pub source: String,
    /// Target transliterated equivalent.
    pub target: String,
}

/// Deterministic voice command dictionary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandDictionary {
    /// Unsupported IoT or device words that map to unknown command.
    pub unsupported: Vec<String>,
    /// Tokens or phrases requesting playback stop.
    pub stop: Vec<String>,
    /// Tokens or phrases requesting playback resume.
    pub resume: Vec<String>,
    /// Tokens or phrases requesting next station navigation.
    pub next_station: Vec<String>,
    /// Tokens or phrases requesting previous station navigation.
    pub previous_station: Vec<String>,
    /// Tokens or phrases requesting volume increase.
    pub volume_up: Vec<String>,
    /// Tokens or phrases requesting volume decrease.
    pub volume_down: Vec<String>,
    /// Tokens or phrases indicating an absolute volume command.
    pub set_volume: Vec<String>,
    /// Tokens or phrases requesting adding to favorites.
    pub add_favorite: Vec<String>,
    /// Basic genre words for fallback radio intent resolution.
    pub genres: GenreKeywords,
}

impl CommandDictionary {
    /// Returns true if the token is an unsupported device or IoT keyword.
    pub fn is_unsupported(&self, token: &str) -> bool {
        self.unsupported.iter().any(|w| w == token)
    }

    /// Returns true if the token indicates an absolute volume command.
    pub fn is_set_volume(&self, token: &str) -> bool {
        self.set_volume.iter().any(|w| w == token)
    }

    /// Returns true if the token requests playback stop.
    pub fn is_stop(&self, token: &str) -> bool {
        self.stop.iter().any(|w| w == token)
    }

    /// Returns true if the token requests playback resume.
    pub fn is_resume(&self, token: &str) -> bool {
        self.resume.iter().any(|w| w == token)
    }

    /// Returns true if the token requests adding station to favorites.
    pub fn is_add_favorite(&self, token: &str) -> bool {
        self.add_favorite.iter().any(|w| w == token)
    }

    /// Returns true if the token requests volume increase.
    pub fn is_volume_up(&self, token: &str) -> bool {
        self.volume_up.iter().any(|w| w == token)
    }

    /// Returns true if the token requests volume decrease.
    pub fn is_volume_down(&self, token: &str) -> bool {
        self.volume_down.iter().any(|w| w == token)
    }

    /// Checks if the token matches the requested genre keyword.
    pub fn matches_genre(&self, token: &str, genre: &str) -> bool {
        match genre {
            "rock" => self.genres.rock.iter().any(|w| w == token),
            "jazz" => self.genres.jazz.iter().any(|w| w == token),
            "metal" => self.genres.metal.iter().any(|w| w == token),
            _ => false,
        }
    }

    /// Collects all basic command tokens that can rescue an unknown command.
    pub fn basic_command_tokens(&self) -> BTreeSet<String> {
        let mut tokens = BTreeSet::new();
        tokens.extend(self.stop.iter().cloned());
        tokens.extend(self.resume.iter().cloned());
        tokens.extend(self.next_station.iter().cloned());
        tokens.extend(self.previous_station.iter().cloned());
        tokens.extend(self.volume_up.iter().cloned());
        tokens.extend(self.volume_down.iter().cloned());
        tokens.extend(self.set_volume.iter().cloned());
        tokens.extend(self.add_favorite.iter().cloned());
        tokens.extend(self.genres.rock.iter().cloned());
        tokens.extend(self.genres.jazz.iter().cloned());
        tokens.extend(self.genres.metal.iter().cloned());
        tokens
    }
}

/// Genre keywords for deterministic query mapping.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenreKeywords {
    /// Rock genre synonyms.
    pub rock: Vec<String>,
    /// Jazz genre synonyms.
    pub jazz: Vec<String>,
    /// Metal genre synonyms.
    pub metal: Vec<String>,
}
