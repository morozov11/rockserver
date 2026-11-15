//! In-memory registry of loaded valid language modules and lexical query interpreters.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use super::{
    schema::LanguageModule,
    translit::{
        is_cyrillic_token, is_latin_token, transliterate_lat_to_ru, transliterate_ru_to_lat,
    },
};

/// Repository of loaded and verified language modules.
#[derive(Clone, Debug)]
pub struct LanguageRegistry {
    modules: BTreeMap<String, Arc<LanguageModule>>,
}

impl LanguageRegistry {
    /// Creates a new registry from validated language modules.
    pub fn new(modules: BTreeMap<String, Arc<LanguageModule>>) -> Self {
        Self { modules }
    }

    /// Returns the language module for the requested code if present.
    pub fn get(&self, code: &str) -> Option<Arc<LanguageModule>> {
        self.modules.get(code).cloned()
    }

    /// Returns the English language module, guaranteed to exist by loader invariants.
    pub fn en(&self) -> Arc<LanguageModule> {
        self.modules
            .get("en")
            .cloned()
            .expect("en language module invariant violated")
    }

    /// Returns all valid language codes currently loaded, sorted alphabetically.
    pub fn valid_codes(&self) -> Vec<String> {
        self.modules.keys().cloned().collect()
    }

    /// Returns whether the specified language code is loaded and valid.
    pub fn is_valid(&self, code: &str) -> bool {
        self.modules.contains_key(code)
    }

    /// Returns whether the token is a recognized basic command token across loaded modules.
    pub fn is_basic_command_token(&self, token: &str) -> bool {
        self.modules.values().any(|module| {
            module.commands.basic_command_tokens().contains(token)
                || module
                    .lexical
                    .name_bases
                    .station_prefixes
                    .iter()
                    .any(|p| p.iter().any(|w| w == token))
        })
    }

    /// Removes stop words from search terms using the resolved language and English.
    pub fn remove_stop_words(&self, terms: &mut Vec<String>, resolved_lang: &str) {
        let module_opt = self.get(resolved_lang);
        let en = self.en();

        let mut stop_words = BTreeSet::new();
        if let Some(ref module) = module_opt {
            stop_words.extend(module.lexical.stop_words.iter().map(String::as_str));
        }
        stop_words.extend(en.lexical.stop_words.iter().map(String::as_str));

        terms.retain(|term| !stop_words.contains(term.as_str()));
    }

    /// Infers an origin country code from search terms.
    ///
    /// Checks the module for `resolved_lang` first, then falls back to `en` if no country matched.
    pub fn infer_country_code(&self, terms: &[String], resolved_lang: &str) -> Option<String> {
        let mut matched_codes = self.match_countries_in_module(terms, resolved_lang);
        if matched_codes.is_empty() && resolved_lang != "en" {
            matched_codes = self.match_countries_in_module(terms, "en");
        }

        if matched_codes.len() == 1 {
            matched_codes.into_iter().next()
        } else {
            None
        }
    }

    fn match_countries_in_module(&self, terms: &[String], lang: &str) -> BTreeSet<String> {
        let Some(module) = self.get(lang) else {
            return BTreeSet::new();
        };

        let mut matched = BTreeSet::new();

        // 1. Country names
        for country in &module.lexical.countries.country_names {
            for name in &country.names {
                if contains_token_sequence(terms, name) {
                    matched.insert(country.code.clone());
                }
            }
        }

        // 2. Inflections
        for inflection in &module.lexical.countries.inflections {
            for form in &inflection.forms {
                if terms.iter().any(|term| term == form) {
                    matched.insert(inflection.code.clone());
                }
            }
        }

        // 3. Demonyms
        for demonym in &module.lexical.demonyms {
            for d in &demonym.demonyms {
                if contains_token_sequence(terms, d) {
                    matched.insert(demonym.code.clone());
                }
            }
        }

        matched
    }

    /// Returns whether the request includes an explicit country preposition ("из", "from").
    pub fn has_explicit_country_request(&self, terms: &[String]) -> bool {
        let mut prepositions = BTreeSet::new();
        for module in self.modules.values() {
            prepositions.extend(
                module
                    .lexical
                    .countries
                    .prepositions
                    .iter()
                    .map(String::as_str),
            );
        }

        terms
            .windows(2)
            .any(|window| prepositions.contains(window[0].as_str()) && !window[1].is_empty())
    }

    /// Detects an explicit broadcast language request across all loaded modules.
    pub fn explicit_language_code(&self, terms: &[String]) -> Option<&'static str> {
        let mut requested: BTreeSet<&'static str> = BTreeSet::new();

        // 1. Check prepositions and suffixes across modules
        for (index, pair) in terms.windows(2).enumerate() {
            for module in self.modules.values() {
                let explicit = &module.lexical.explicit_languages;

                // Preposition match: e.g. "in" / "на"
                if explicit.prepositions.iter().any(|p| p == &pair[0]) {
                    // Check style exclusion (e.g. "in" followed by lang followed by "style")
                    let is_style = terms
                        .get(index + 2)
                        .is_some_and(|next| explicit.style_exclusions.iter().any(|e| e == next));
                    if !is_style
                        && let Some(code) = find_language_by_form(&explicit.languages, &pair[1])
                    {
                        requested.insert(leak_code(code));
                    }
                }

                // Suffix match: e.g. pair[1] == "language" or "язык" / "языке"
                if explicit.language_suffixes.iter().any(|s| s == &pair[1])
                    && let Some(code) = find_language_by_form(&explicit.languages, &pair[0])
                {
                    requested.insert(leak_code(code));
                }

                // Phrase match: e.g. ["по", "русски"]
                for phrase in &explicit.phrases {
                    if phrase.phrase.len() == 2
                        && phrase.phrase[0] == pair[0]
                        && phrase.phrase[1] == pair[1]
                    {
                        requested.insert(leak_code(&phrase.code));
                    }
                }
            }
        }

        // Also check "по-русски" when tokenized as ["по", "русски"] in a window
        for pair in terms.windows(2) {
            for module in self.modules.values() {
                for phrase in &module.lexical.explicit_languages.phrases {
                    if phrase.phrase.len() == 2
                        && phrase.phrase[0] == pair[0]
                        && phrase.phrase[1] == pair[1]
                    {
                        requested.insert(leak_code(&phrase.code));
                    }
                }
            }
        }

        // 2. Check standalone single-token markers
        for term in terms {
            for module in self.modules.values() {
                for standalone in &module.lexical.explicit_languages.standalone {
                    if standalone.terms.iter().any(|t| t == term) {
                        requested.insert(leak_code(&standalone.code));
                    }
                }
            }
        }

        if requested.len() != 1 {
            return None;
        }

        let code = *requested.first().unwrap();

        // Veto if terms mention another language word without preposition/suffix
        let mentions_other_language = terms.iter().any(|term| {
            for module in self.modules.values() {
                if let Some(mentioned) =
                    find_language_by_form(&module.lexical.explicit_languages.languages, term)
                    && mentioned != code
                {
                    return true;
                }
            }
            false
        });

        if mentions_other_language {
            None
        } else {
            Some(code)
        }
    }

    /// Extracts station name hint queries from request tokens.
    pub fn station_name_hint_queries(&self, original: &str, resolved_lang: &str) -> Vec<String> {
        let ordered = crate::search::tokenize(original);
        let ordered_refs = ordered.iter().map(String::as_str).collect::<Vec<_>>();

        let module_opt = self.get(resolved_lang);
        let en = self.en();

        let mut prefixes: Vec<Vec<&str>> = Vec::new();
        if let Some(ref module) = module_opt {
            for prefix in &module.lexical.name_bases.station_prefixes {
                prefixes.push(prefix.iter().map(String::as_str).collect());
            }
        }
        for prefix in &en.lexical.name_bases.station_prefixes {
            prefixes.push(prefix.iter().map(String::as_str).collect());
        }

        let Some(prefix) = prefixes
            .iter()
            .find(|prefix| ordered_refs.starts_with(prefix.as_slice()))
        else {
            return Vec::new();
        };

        let mut stop_words = BTreeSet::new();
        if let Some(ref module) = module_opt {
            stop_words.extend(module.lexical.stop_words.iter().map(String::as_str));
        }
        stop_words.extend(en.lexical.stop_words.iter().map(String::as_str));

        let hint_tokens = ordered[prefix.len()..]
            .iter()
            .map(String::as_str)
            .filter(|token| !stop_words.contains(token))
            .collect::<Vec<_>>();
        if hint_tokens.is_empty() {
            return Vec::new();
        }

        let raw_hint = hint_tokens.join(" ");
        let mut hints = BTreeSet::from([raw_hint.clone()]);
        if is_cyrillic_token(&raw_hint) {
            hints.insert(transliterate_ru_to_lat(&raw_hint));
        } else if is_latin_token(&raw_hint) {
            hints.insert(transliterate_lat_to_ru(&raw_hint));
        }
        hints
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect()
    }

    /// Expands search terms with transliterations from module dictionaries and generic script rules.
    pub fn expand_transliterations(&self, terms: &mut Vec<String>, resolved_lang: &str) {
        let mut all = terms.iter().cloned().collect::<BTreeSet<_>>();
        let snapshot = all.iter().cloned().collect::<Vec<_>>();

        let module = self.get(resolved_lang).unwrap_or_else(|| self.en());

        for term in snapshot {
            for entry in &module.lexical.name_bases.word_transliterations {
                if term == entry.source {
                    all.insert(entry.target.clone());
                } else if term == entry.target {
                    all.insert(entry.source.clone());
                }
            }

            if is_cyrillic_token(&term) {
                all.insert(transliterate_ru_to_lat(&term));
            } else if is_latin_token(&term) {
                all.insert(transliterate_lat_to_ru(&term));
            }
        }

        *terms = all.into_iter().collect();
    }
}

fn contains_token_sequence(terms: &[String], sequence: &str) -> bool {
    let seq_tokens = sequence.split_whitespace().collect::<Vec<_>>();
    if seq_tokens.is_empty() {
        return false;
    }
    terms.windows(seq_tokens.len()).any(|window| {
        window
            .iter()
            .map(String::as_str)
            .eq(seq_tokens.iter().copied())
    })
}

fn find_language_by_form<'a>(
    languages: &'a [super::schema::LanguageFormEntry],
    word: &str,
) -> Option<&'a str> {
    languages
        .iter()
        .find(|lang| lang.forms.iter().any(|form| form == word))
        .map(|lang| lang.code.as_str())
}

fn leak_code(code: &str) -> &'static str {
    match code {
        "ar" => "ar",
        "cs" => "cs",
        "da" => "da",
        "de" => "de",
        "el" => "el",
        "en" => "en",
        "es" => "es",
        "fi" => "fi",
        "fr" => "fr",
        "he" => "he",
        "hi" => "hi",
        "hu" => "hu",
        "id" => "id",
        "it" => "it",
        "ja" => "ja",
        "ko" => "ko",
        "nl" => "nl",
        "no" => "no",
        "pl" => "pl",
        "pt" => "pt",
        "ro" => "ro",
        "ru" => "ru",
        "sv" => "sv",
        "th" => "th",
        "tr" => "tr",
        "uk" => "uk",
        "vi" => "vi",
        "zh" => "zh",
        other => Box::leak(other.to_owned().into_boxed_str()),
    }
}
