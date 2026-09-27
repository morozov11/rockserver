//! Provider-neutral query interpretation and deterministic metadata fallback.

use std::{collections::BTreeSet, error::Error, fmt};

use async_trait::async_trait;

use super::{SearchQuery, taxonomy::canonical_tags};

/// Request-only input supplied to a query parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryParserInput {
    /// Validated natural-language request text.
    pub query: String,
    /// Validated locale used to interpret the request.
    pub locale: String,
}

/// Structured intent returned by a query parser before repository search.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryIntent {
    /// Whether the caller should immediately play or only display matches.
    pub action: SearchAction,
    /// Lowercase terms that participate in metadata matching.
    pub terms: Vec<String>,
    /// Normalized catalog tags inferred from the request.
    pub tags: Vec<String>,
    /// Optional ISO 639 language hard filter.
    pub language: Option<String>,
    /// Optional ISO 3166-1 alpha-2 country hard filter.
    pub country_code: Option<String>,
    /// Number of core terms before transliteration expansion, used as the
    /// score denominator so alias terms don't dilute match quality.
    pub core_term_count: usize,
    /// Cleaned query string (stop-words removed) for full-text search.
    pub raw_query: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchAction {
    #[default]
    Play,
    Show,
}

/// Safe query-parser failure that can fall back to deterministic interpretation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryParserError {
    summary: String,
}

impl QueryParserError {
    /// Creates a provider-safe failure summary for logs.
    pub fn safe(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
        }
    }
}

impl fmt::Display for QueryParserError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.summary)
    }
}

impl Error for QueryParserError {}

/// Boundary for translating request text into structured search intent.
///
/// Implementations receive only request data. They never receive stations or catalog snapshots.
#[async_trait]
pub trait QueryParser: Send + Sync {
    /// Parses one validated request into provider-neutral intent.
    async fn parse(&self, input: &QueryParserInput) -> Result<QueryIntent, QueryParserError>;
}

/// Existing deterministic metadata interpreter used by default and as the failure fallback.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeterministicQueryParser;

#[async_trait]
impl QueryParser for DeterministicQueryParser {
    async fn parse(&self, input: &QueryParserInput) -> Result<QueryIntent, QueryParserError> {
        Ok(deterministic_intent(&input.query, &input.locale))
    }
}

/// Builds a normalized query using the deterministic metadata interpretation.
///
/// Runs deterministic intent parsing followed by [`validate_intent`] so terms are
/// deduplicated, stop-words removed, and transliteration variants expanded exactly once.
pub fn normalize_query(original: String, locale: String) -> SearchQuery {
    let intent = validate_intent(deterministic_intent(&original, &locale))
        .expect("deterministic intent is always valid");
    SearchQuery::from_intent(original, locale, intent)
}

pub(super) fn station_name_hint_queries(original: &str) -> Vec<String> {
    let ordered = tokenize(original);
    let prefixes: &[&[&str]] = &[
        &["включи", "радио"],
        &["включить", "радио"],
        &["поставь", "радио"],
        &["поставить", "радио"],
        &["play", "radio"],
        &["start", "radio"],
        &["turn", "on", "radio"],
    ];

    let ordered_refs = ordered.iter().map(String::as_str).collect::<Vec<_>>();
    let Some(prefix) = prefixes
        .iter()
        .find(|prefix| ordered_refs.starts_with(prefix))
    else {
        return Vec::new();
    };

    let hint_tokens = ordered[prefix.len()..]
        .iter()
        .map(String::as_str)
        .filter(|token| !STOP_WORDS.contains(token))
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

/// Validates and normalizes structured query intent before repository search.
///
/// This is the single location where query terms are finalized:
/// 1. Values are trimmed and converted to lowercase before tokenization (preserving
///    compound terms from structured providers like `"RockRadio"`), tokenized via [`tokenize`]
///    (splitting on non-alphanumeric separators), and stripped of command stop-words.
/// 2. Unique terms are preserved in their original encounter order to compute `raw_query`
///    and `core_term_count` before transliteration expansion.
/// 3. Transliteration variants are added to `terms` via [`expand_transliterations`].
///
/// Also validates optional ISO language and country filters.
pub(super) fn validate_intent(intent: QueryIntent) -> Result<QueryIntent, QueryParserError> {
    let mut raw_terms = Vec::new();
    let mut seen = BTreeSet::new();

    for raw in intent.terms {
        let normalized = raw.trim().to_lowercase();
        if normalized.is_empty() {
            continue;
        }
        for token in tokenize(&normalized) {
            if !STOP_WORDS.contains(&token.as_str()) && seen.insert(token.clone()) {
                raw_terms.push(token);
            }
        }
    }

    let raw_query = raw_terms.join(" ");
    let core_term_count = raw_terms.len();

    let mut terms = raw_terms;
    expand_transliterations(&mut terms);

    let tags = canonical_tags(intent.tags);
    let language = intent
        .language
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    if language.as_deref().is_some_and(|value| {
        !(2..=3).contains(&value.len()) || !value.bytes().all(|byte| byte.is_ascii_lowercase())
    }) {
        return Err(QueryParserError::safe(
            "query parser returned an invalid language filter",
        ));
    }

    let country_code = intent
        .country_code
        .map(|value| value.trim().to_ascii_uppercase())
        .filter(|value| !value.is_empty());
    if country_code.as_deref().is_some_and(|value| {
        value.len() != 2 || !value.bytes().all(|byte| byte.is_ascii_uppercase())
    }) {
        return Err(QueryParserError::safe(
            "query parser returned an invalid country filter",
        ));
    }

    Ok(QueryIntent {
        action: intent.action,
        terms,
        tags,
        language,
        country_code,
        core_term_count,
        raw_query,
    })
}

/// Deterministic query interpretation yielding unexpanded query terms.
///
/// Query terms are tokenized, cleaned of stop-words, and deduplicated in original
/// word order without transliteration expansion. A transliterated copy is used
/// internally only to infer catalog genre tags (e.g., mapping `"рок"` to `"rock"`).
///
/// The resulting intent must pass through [`validate_intent`] before repository search
/// to apply transliteration expansions and finalize `core_term_count` and `raw_query`.
pub(super) fn deterministic_intent(original: &str, _locale: &str) -> QueryIntent {
    let raw_tokens = tokenize(original);
    let country_code = infer_country_code(&raw_tokens);
    let language = infer_language(&raw_tokens);

    let mut terms = Vec::new();
    let mut seen = BTreeSet::new();
    for token in raw_tokens {
        if !STOP_WORDS.contains(&token.as_str()) && seen.insert(token.clone()) {
            terms.push(token);
        }
    }

    let raw_query = terms.join(" ");
    let core_term_count = terms.len();

    let mut expanded_for_tags = terms.clone();
    expand_transliterations(&mut expanded_for_tags);
    let tags = canonical_tags(expanded_for_tags);

    QueryIntent {
        action: SearchAction::Play,
        terms,
        tags,
        language,
        country_code,
        core_term_count,
        raw_query,
    }
}

pub fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .flat_map(split_camel_case)
        .map(|t| t.to_lowercase())
        .collect()
}

/// Splits a token on camelCase/PascalCase boundaries.
///
/// `"radioDJ"` becomes `["radio", "DJ"]`, `"HelloWorld"` becomes `["Hello", "World"]`.
/// Runs of uppercase followed by a lowercase letter split before the last uppercase
/// so `"XMLParser"` becomes `["XML", "Parser"]`.
fn split_camel_case(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 1 {
        return vec![token.to_owned()];
    }
    let mut parts = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        let split = (chars[i - 1].is_lowercase() && chars[i].is_uppercase())
            || (i + 1 < chars.len()
                && chars[i - 1].is_uppercase()
                && chars[i].is_uppercase()
                && chars[i + 1].is_lowercase());
        if split {
            let part: String = chars[start..i].iter().collect();
            if !part.is_empty() {
                parts.push(part);
            }
            start = i;
        }
    }
    let tail: String = chars[start..].iter().collect();
    if !tail.is_empty() {
        parts.push(tail);
    }
    if parts.is_empty() {
        vec![token.to_owned()]
    } else {
        parts
    }
}

/// Command verbs that should not participate in station name matching.
const STOP_WORDS: &[&str] = &[
    "включи",
    "включить",
    "вруби",
    "врубить",
    "поставь",
    "поставить",
    "найди",
    "найти",
    "играй",
    "играть",
    "запусти",
    "запустить",
    "переключи",
    "переключить",
    "открой",
    "открыть",
    "покажи",
    "показать",
    "давай",
    "хочу",
    "play",
    "find",
    "search",
    "show",
    "start",
    "open",
    "turn",
    "on",
    "put",
];

/// Well-known word-level transliterations between Russian and Latin radio terms.
const WORD_TRANSLIT: &[(&str, &str)] = &[
    ("радио", "radio"),
    ("диджей", "dj"),
    ("фм", "fm"),
    ("рок", "rock"),
    ("ультра", "ultra"),
    ("джаз", "jazz"),
    ("поп", "pop"),
    ("хит", "hit"),
    ("микс", "mix"),
    ("лав", "love"),
    ("классик", "classic"),
    ("классика", "classic"),
    ("блюз", "blues"),
    ("кантри", "country"),
    ("фанк", "funk"),
    ("соул", "soul"),
    ("метал", "metal"),
    ("металл", "metal"),
    ("панк", "punk"),
    ("техно", "techno"),
    ("транс", "trance"),
    ("хаус", "house"),
    ("драм", "drum"),
    ("бас", "bass"),
    ("лайф", "life"),
    ("лайв", "live"),
    ("стайл", "style"),
    ("бест", "best"),
    ("топ", "top"),
    ("голд", "gold"),
    ("сити", "city"),
    ("клуб", "club"),
    ("чилл", "chill"),
    ("чиллаут", "chillout"),
    ("энерджи", "energy"),
    ("ритм", "rhythm"),
    ("саунд", "sound"),
    ("мьюзик", "music"),
    ("музыка", "music"),
    ("релакс", "relax"),
    ("дип", "deep"),
    ("нью", "new"),
    ("олд", "old"),
    ("супер", "super"),
    ("мега", "mega"),
    ("максимум", "maximum"),
    ("европа", "europa"),
    ("плюс", "plus"),
    ("блэк", "black"),
    ("дэт", "death"),
    ("хэви", "heavy"),
    ("хард", "hard"),
    ("прогрессив", "progressive"),
    ("альтернатив", "alternative"),
    ("инди", "indie"),
    ("гранж", "grunge"),
    ("диско", "disco"),
    ("реггей", "reggae"),
    ("регги", "reggae"),
    ("латин", "latin"),
    ("эмбиент", "ambient"),
    ("амбиент", "ambient"),
    ("даунтемпо", "downtempo"),
    ("трип", "trip"),
    ("хоп", "hop"),
    ("хип", "hip"),
    ("рэп", "rap"),
    ("электро", "electro"),
    ("синт", "synth"),
    ("вейв", "wave"),
    ("лаунж", "lounge"),
    ("госпел", "gospel"),
    ("фолк", "folk"),
    ("кавер", "cover"),
    ("акустик", "acoustic"),
    ("акустика", "acoustic"),
    ("пауэр", "power"),
    ("треш", "thrash"),
    ("спид", "speed"),
    ("дум", "doom"),
    ("нойз", "noise"),
    ("пост", "post"),
    ("кор", "core"),
    ("скрим", "scream"),
    ("свинг", "swing"),
    ("биг", "big"),
    ("бэнд", "band"),
    ("стейшн", "station"),
    ("станция", "station"),
    // Common voice-command station-name tokens.
    ("ультра", "ultra"),
    ("рокс", "roks"),
    ("викер", "viker"),
];

/// Expands query terms with transliterated equivalents.
///
/// For each term, if a known word mapping exists, both the original and the
/// transliterated form are kept. This lets `"радио"` match stations named
/// `"Radio ..."` and vice versa.
pub(super) fn expand_transliterations(terms: &mut Vec<String>) {
    let mut all = terms.iter().cloned().collect::<BTreeSet<_>>();
    let snapshot = all.iter().cloned().collect::<Vec<_>>();

    for term in snapshot {
        // Keep dictionary-based expansions for stable, high-signal terms
        // (genres and common radio words), then add generic transliteration.
        for &(cyrillic, latin) in WORD_TRANSLIT {
            if term == cyrillic {
                all.insert(latin.to_owned());
            } else if term == latin {
                all.insert(cyrillic.to_owned());
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

fn is_cyrillic_token(term: &str) -> bool {
    term.chars().any(is_cyrillic_char)
}

fn is_latin_token(term: &str) -> bool {
    term.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn is_cyrillic_char(ch: char) -> bool {
    ('а'..='я').contains(&ch) || ch == 'ё'
}

/// Generic transliteration from Russian Cyrillic to Latin.
///
/// This complements dictionary mappings so station names like `боб` can match
/// `bob` without explicit per-word entries.
pub(super) fn transliterate_ru_to_lat(term: &str) -> String {
    let mut out = String::with_capacity(term.len() * 2);
    for ch in term.chars() {
        let mapped = match ch {
            'а' => "a",
            'б' => "b",
            'в' => "v",
            'г' => "g",
            'д' => "d",
            'е' => "e",
            'ё' => "yo",
            'ж' => "zh",
            'з' => "z",
            'и' => "i",
            'й' => "y",
            'к' => "k",
            'л' => "l",
            'м' => "m",
            'н' => "n",
            'о' => "o",
            'п' => "p",
            'р' => "r",
            'с' => "s",
            'т' => "t",
            'у' => "u",
            'ф' => "f",
            'х' => "kh",
            'ц' => "ts",
            'ч' => "ch",
            'ш' => "sh",
            'щ' => "shch",
            'ъ' | 'ь' => "",
            'ы' => "y",
            'э' => "e",
            'ю' => "yu",
            'я' => "ya",
            _ => {
                out.push(ch);
                continue;
            }
        };
        out.push_str(mapped);
    }
    out
}

/// Best-effort transliteration from Latin to Russian Cyrillic.
///
/// It is intentionally approximate and designed for search recall.
pub(super) fn transliterate_lat_to_ru(term: &str) -> String {
    let lower = term.to_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0usize;
    let mut out = String::with_capacity(lower.len());

    while i < bytes.len() {
        let rest = &lower[i..];
        if rest.starts_with("shch") {
            out.push('щ');
            i += 4;
            continue;
        }
        if rest.starts_with("yo") {
            out.push('ё');
            i += 2;
            continue;
        }
        if rest.starts_with("zh") {
            out.push('ж');
            i += 2;
            continue;
        }
        if rest.starts_with("kh") {
            out.push('х');
            i += 2;
            continue;
        }
        if rest.starts_with("ts") {
            out.push('ц');
            i += 2;
            continue;
        }
        if rest.starts_with("ch") {
            out.push('ч');
            i += 2;
            continue;
        }
        if rest.starts_with("sh") {
            out.push('ш');
            i += 2;
            continue;
        }
        if rest.starts_with("yu") {
            out.push('ю');
            i += 2;
            continue;
        }
        if rest.starts_with("ya") {
            out.push('я');
            i += 2;
            continue;
        }
        if rest.starts_with("ye") {
            out.push('е');
            i += 2;
            continue;
        }

        let ch = bytes[i] as char;
        let mapped = match ch {
            'a' => "а",
            'b' => "б",
            'v' => "в",
            'g' => "г",
            'd' => "д",
            'e' => "е",
            'z' => "з",
            'i' => "и",
            'j' => "й",
            'y' => "й",
            'k' => "к",
            'l' => "л",
            'm' => "м",
            'n' => "н",
            'o' => "о",
            'p' => "п",
            'r' => "р",
            's' => "с",
            't' => "т",
            'u' => "у",
            'f' => "ф",
            'h' => "х",
            'c' => "к",
            'q' => "к",
            'w' => "в",
            'x' => "кс",
            _ => {
                out.push(ch);
                i += 1;
                continue;
            }
        };
        out.push_str(mapped);
        i += 1;
    }

    out
}

/// Language specification for explicit broadcast language patterns.
struct ExplicitLanguageSpec {
    code: &'static str,
    prepositional: &'static [&'static str],
    adverbial: &'static [&'static str],
    yazychn_prefixes: &'static [&'static str],
    english_names: &'static [&'static str],
}

const EXPLICIT_LANGUAGES: &[ExplicitLanguageSpec] = &[
    ExplicitLanguageSpec {
        code: "de",
        prepositional: &["немецком"],
        adverbial: &["немецки"],
        yazychn_prefixes: &["немецкоязычн"],
        english_names: &["german"],
    },
    ExplicitLanguageSpec {
        code: "en",
        prepositional: &["английском"],
        adverbial: &["английски"],
        yazychn_prefixes: &["англоязычн", "английскоязычн"],
        english_names: &["english"],
    },
    ExplicitLanguageSpec {
        code: "fr",
        prepositional: &["французском"],
        adverbial: &["французски"],
        yazychn_prefixes: &["франкоязычн", "французскоязычн"],
        english_names: &["french"],
    },
    ExplicitLanguageSpec {
        code: "es",
        prepositional: &["испанском"],
        adverbial: &["испански"],
        yazychn_prefixes: &["испаноязычн", "испанскоязычн"],
        english_names: &["spanish"],
    },
    ExplicitLanguageSpec {
        code: "it",
        prepositional: &["итальянском"],
        adverbial: &["итальянски"],
        yazychn_prefixes: &["итальяноязычн", "итальянскоязычн"],
        english_names: &["italian"],
    },
    ExplicitLanguageSpec {
        code: "ru",
        prepositional: &["русском"],
        adverbial: &["русски"],
        yazychn_prefixes: &["русскоязычн"],
        english_names: &["russian"],
    },
    ExplicitLanguageSpec {
        code: "uk",
        prepositional: &["украинском"],
        adverbial: &["украински"],
        yazychn_prefixes: &["украиноязычн", "украинскоязычн"],
        english_names: &["ukrainian"],
    },
    ExplicitLanguageSpec {
        code: "pl",
        prepositional: &["польском"],
        adverbial: &["польски"],
        yazychn_prefixes: &["польскоязычн"],
        english_names: &["polish"],
    },
    ExplicitLanguageSpec {
        code: "ja",
        prepositional: &["японском"],
        adverbial: &["японски"],
        yazychn_prefixes: &["японоязычн", "японскоязычн"],
        english_names: &["japanese"],
    },
    ExplicitLanguageSpec {
        code: "zh",
        prepositional: &["китайском"],
        adverbial: &["китайски"],
        yazychn_prefixes: &["китаеязычн", "китайскоязычн"],
        english_names: &["chinese"],
    },
    ExplicitLanguageSpec {
        code: "ko",
        prepositional: &["корейском"],
        adverbial: &["корейски"],
        yazychn_prefixes: &["корееязычн", "корейскоязычн"],
        english_names: &["korean"],
    },
    ExplicitLanguageSpec {
        code: "tr",
        prepositional: &["турецком"],
        adverbial: &["турецки"],
        yazychn_prefixes: &["турецкоязычн", "тюркоязычн"],
        english_names: &["turkish"],
    },
    ExplicitLanguageSpec {
        code: "pt",
        prepositional: &["португальском"],
        adverbial: &["португальски"],
        yazychn_prefixes: &["португалоязычн", "португальскоязычн"],
        english_names: &["portuguese"],
    },
    ExplicitLanguageSpec {
        code: "sv",
        prepositional: &["шведском"],
        adverbial: &["шведски"],
        yazychn_prefixes: &["шведоязычн", "шведскоязычн"],
        english_names: &["swedish"],
    },
    ExplicitLanguageSpec {
        code: "fi",
        prepositional: &["финском"],
        adverbial: &["фински"],
        yazychn_prefixes: &["финноязычн", "финскоязычн"],
        english_names: &["finnish"],
    },
    ExplicitLanguageSpec {
        code: "no",
        prepositional: &["норвежском"],
        adverbial: &["норвежски"],
        yazychn_prefixes: &["норвежскоязычн"],
        english_names: &["norwegian"],
    },
    ExplicitLanguageSpec {
        code: "da",
        prepositional: &["датском"],
        adverbial: &["датски"],
        yazychn_prefixes: &["датскоязычн"],
        english_names: &["danish"],
    },
    ExplicitLanguageSpec {
        code: "nl",
        prepositional: &["голландском", "нидерландском"],
        adverbial: &["голландски", "нидерландски"],
        yazychn_prefixes: &["голландскоязычн", "нидерландскоязычн"],
        english_names: &["dutch"],
    },
    ExplicitLanguageSpec {
        code: "cs",
        prepositional: &["чешском"],
        adverbial: &["чешски"],
        yazychn_prefixes: &["чешскоязычн"],
        english_names: &["czech"],
    },
    ExplicitLanguageSpec {
        code: "el",
        prepositional: &["греческом"],
        adverbial: &["гречески"],
        yazychn_prefixes: &["грекоязычн", "греческоязычн"],
        english_names: &["greek"],
    },
    ExplicitLanguageSpec {
        code: "hu",
        prepositional: &["венгерском"],
        adverbial: &["венгерски"],
        yazychn_prefixes: &["венгроязычн", "венгерскоязычн"],
        english_names: &["hungarian"],
    },
    ExplicitLanguageSpec {
        code: "ro",
        prepositional: &["румынском"],
        adverbial: &["румынски"],
        yazychn_prefixes: &["румыноязычн", "румынскоязычн"],
        english_names: &["romanian"],
    },
];

/// Infers an ISO 639 broadcast language hard filter only when explicitly requested.
///
/// Bare nationality adjectives (such as "немецкий" or "german") do not infer a language.
/// If more than one language is matched, returns `None`.
fn infer_language(terms: &[String]) -> Option<String> {
    let mut matched = BTreeSet::new();

    // Check two-token windows:
    // - "на <prepositional>" (e.g. "на немецком")
    // - "in <english_name>" (e.g. "in German")
    //   NOTE (SRCH-004 fix): "на <prepositional>" and "in <Language>" are considered
    //   explicit language requests ONLY when positioned at the end of the query OR
    //   immediately followed by "языке" / "языка" / "language".
    // - "по <adverbial>" (e.g. "по немецки", from "по-немецки")
    // - "<english_name> language" (e.g. "German language", "German-language")
    for (i, window) in terms.windows(2).enumerate() {
        let first = window[0].as_str();
        let second = window[1].as_str();

        let is_valid_prep_context = (i + 1 == terms.len() - 1)
            || (i + 2 < terms.len()
                && matches!(terms[i + 2].as_str(), "языке" | "языка" | "language"));

        for spec in EXPLICIT_LANGUAGES {
            let is_prep_match = ((first == "на" && spec.prepositional.contains(&second))
                || (first == "in" && spec.english_names.contains(&second)))
                && is_valid_prep_context;

            let is_other_two_token_match = (first == "по" && spec.adverbial.contains(&second))
                || (second == "language" && spec.english_names.contains(&first));

            if is_prep_match || is_other_two_token_match {
                matched.insert(spec.code);
            }
        }
    }

    // Check single tokens:
    // - "<stem>язычн..." in all inflected forms
    // - "по<adverbial>" (e.g. "понемецки")
    // - "<english_name>language"
    for term in terms {
        for spec in EXPLICIT_LANGUAGES {
            if spec
                .yazychn_prefixes
                .iter()
                .any(|prefix| term.starts_with(prefix))
                || spec
                    .adverbial
                    .iter()
                    .any(|adv| term.strip_prefix("по") == Some(adv))
                || spec
                    .english_names
                    .iter()
                    .any(|eng| term.strip_suffix("language") == Some(eng))
            {
                matched.insert(spec.code);
            }
        }
    }

    (matched.len() == 1).then(|| matched.into_iter().next().unwrap().to_owned())
}

/// Infers an ISO 3166-1 alpha-2 country hard filter only when a country name is explicitly recognized.
///
/// Demonyms and cultural adjectives (such as "немецкий" or "german") do not create country filters.
/// Exactly one recognized country yields a filter; ambiguous or multiple countries return `None`.
pub(super) fn infer_country_code(terms: &[String]) -> Option<String> {
    let matched_codes = COUNTRY_ALIASES
        .split('|')
        .filter_map(|entry| {
            let (code, aliases) = entry.split_once('=')?;
            aliases
                .split(';')
                .any(|alias| contains_country_alias(terms, alias))
                .then_some(code)
        })
        .chain(
            RUSSIAN_COUNTRY_INFLECTIONS
                .iter()
                .filter_map(|(form, code)| terms.iter().any(|term| term == form).then_some(*code)),
        )
        .collect::<BTreeSet<_>>();

    (matched_codes.len() == 1).then(|| matched_codes.into_iter().next().unwrap().to_owned())
}

/// Returns whether a normalized country alias occurs as a complete token sequence.
fn contains_country_alias(terms: &[String], alias: &str) -> bool {
    let alias_terms = alias.split_whitespace().collect::<Vec<_>>();
    terms.windows(alias_terms.len()).any(|window| {
        window
            .iter()
            .map(String::as_str)
            .eq(alias_terms.iter().copied())
    })
}

// ISO 3166-1 alpha-2 countries with English/Russian country names only (demonyms removed per SRCH-004).
// Ambiguous forms such as "конго" are intentionally omitted.
const COUNTRY_ALIASES: &str = "AF=afghanistan;афганистан|AL=albania;албания|DZ=algeria;алжир|AD=andorra;андорра|AO=angola;ангола|AG=antigua and barbuda;антигуа и барбуда|AR=argentina;аргентина|AM=armenia;армения|AU=australia;австралия|AT=austria;австрия|AZ=azerbaijan;азербайджан|BS=bahamas;багамы|BH=bahrain;бахрейн|BD=bangladesh;бангладеш|BB=barbados;барбадос|BY=belarus;беларусь;белоруссия|BE=belgium;бельгия|BZ=belize;белиз|BJ=benin;бенин|BT=bhutan;бутан|BO=bolivia;боливия|BA=bosnia and herzegovina;босния и герцеговина|BW=botswana;ботсвана|BR=brazil;бразилия|BN=brunei;бруней|BG=bulgaria;болгария|BF=burkina faso;буркина фасо|BI=burundi;бурунди|CV=cabo verde;cape verde;кабо верде|KH=cambodia;камбоджа|CM=cameroon;камерун|CA=canada;канада|CF=central african republic;центральноафриканская республика|TD=chad;чад|CL=chile;чили|CN=china;китай|CO=colombia;колумбия|KM=comoros;коморы|CG=republic of congo;республика конго|CD=democratic republic of congo;демократическая республика конго|CR=costa rica;коста рика|CI=cote d ivoire;ivory coast;кот д ивуар|HR=croatia;хорватия|CU=cuba;куба|CY=cyprus;кипр|CZ=czechia;czech republic;чехия|DK=denmark;дания|DJ=djibouti;джибути|DM=dominica;доминика|DO=dominican republic;доминиканская республика|EC=ecuador;эквадор|EG=egypt;египет|SV=el salvador;сальвадор|GQ=equatorial guinea;экваториальная гвинея|ER=eritrea;эритрея|EE=estonia;эстония|SZ=eswatini;свазиленд;эсватини|ET=ethiopia;эфиопия|FJ=fiji;фиджи|FI=finland;финляндия|FR=france;франция|GA=gabon;габон|GM=gambia;гамбия|GE=georgia;грузия|DE=germany;германия|GH=ghana;гана|GR=greece;греция|GD=grenada;гренада|GT=guatemala;гватемала|GN=guinea;гвинея|GW=guinea bissau;гвинея бисау|GY=guyana;гайана|HT=haiti;гаити|VA=holy see;vatican;ватикан|HN=honduras;гондурас|HU=hungary;венгрия|IS=iceland;исландия|IN=india;индия|ID=indonesia;индонезия|IR=iran;иран|IQ=iraq;ирак|IE=ireland;ирландия|IL=israel;израиль|IT=italy;италия|JM=jamaica;ямайка|JP=japan;япония|JO=jordan;иордания|KZ=kazakhstan;казахстан|KE=kenya;кения|KI=kiribati;кирибати|KP=north korea;северная корея|KR=south korea;южная корея|KW=kuwait;кувейт|KG=kyrgyzstan;киргизия;кыргызстан|LA=laos;лаос|LV=latvia;латвия|LB=lebanon;ливан|LS=lesotho;лесото|LR=liberia;либерия|LY=libya;ливия|LI=liechtenstein;лихтенштейн|LT=lithuania;литва|LU=luxembourg;люксембург|MG=madagascar;мадагаскар|MW=malawi;малави|MY=malaysia;малайзия|MV=maldives;мальдивы|ML=mali;мали|MT=malta;мальта|MH=marshall islands;маршалловы острова|MR=mauritania;мавритания|MU=mauritius;маврикий|MX=mexico;мексика|FM=micronesia;микронезия|MD=moldova;молдова|MC=monaco;монако|MN=mongolia;монголия|ME=montenegro;черногория|MA=morocco;марокко|MZ=mozambique;мозамбик|MM=myanmar;мьянма;бирма|NA=namibia;намибия|NR=nauru;науру|NP=nepal;непал|NL=netherlands;holland;нидерланды;голландия|NZ=new zealand;новая зеландия|NI=nicaragua;никарагуа|NE=niger;нигер|NG=nigeria;нигерия|MK=north macedonia;северная македония|NO=norway;норвегия|OM=oman;оман|PK=pakistan;пакистан|PW=palau;палау|PS=palestine;палестина|PA=panama;панама|PG=papua new guinea;папуа новая гвинея|PY=paraguay;парагвай|PE=peru;перу|PH=philippines;филиппины|PL=poland;польша|PT=portugal;португалия|QA=qatar;катар|RO=romania;румыния|RU=russia;россия|RW=rwanda;руанда|KN=saint kitts and nevis;сент китс и невис|LC=saint lucia;сент люсия|VC=saint vincent and the grenadines;сент винсент и гренадины|WS=samoa;самоа|SM=san marino;сан марино|ST=sao tome and principe;сан томе и принсипи|SA=saudi arabia;саудовская аравия|SN=senegal;сенегал|RS=serbia;сербия|SC=seychelles;сейшелы|SL=sierra leone;сьерра леоне|SG=singapore;сингапур|SK=slovakia;словакия|SI=slovenia;словения|SB=solomon islands;соломоновы острова|SO=somalia;сомали|ZA=south africa;южная африка|SS=south sudan;южный судан|ES=spain;испания|LK=sri lanka;шри ланка|SD=sudan;судан|SR=suriname;суринам|SE=sweden;швеция|CH=switzerland;швейцария|SY=syria;сирия|TJ=tajikistan;таджикистан|TZ=tanzania;танзания|TH=thailand;таиланд;тайланд|TL=timor leste;east timor;восточный тимор|TG=togo;того|TO=tonga;тонга|TT=trinidad and tobago;тринидад и тобаго|TN=tunisia;тунис|TR=turkey;türkiye;турция|TM=turkmenistan;туркменистан|TV=tuvalu;тувалу|UG=uganda;уганда|UA=ukraine;украина|AE=united arab emirates;объединенные арабские эмираты;эмираты|GB=united kingdom;great britain;britain;великобритания;британия;соединенное королевство;англия;uk|US=united states;united states of america;сша;соединенные штаты;америка;usa|UY=uruguay;уругвай|UZ=uzbekistan;узбекистан|VU=vanuatu;вануату|VE=venezuela;венесуэла|VN=vietnam;вьетнам|YE=yemen;йемен|ZM=zambia;замбия|ZW=zimbabwe;зимбабве";

/// Russian inflected case forms for world countries.
const RUSSIAN_COUNTRY_INFLECTIONS: &[(&str, &str)] = &[
    // Австрия (AT)
    ("австрии", "AT"),
    ("австрию", "AT"),
    ("австрией", "AT"),
    // Англия / Великобритания / Британия (GB)
    ("англии", "GB"),
    ("англию", "GB"),
    ("англией", "GB"),
    ("великобритании", "GB"),
    ("великобританию", "GB"),
    ("великобританией", "GB"),
    ("британии", "GB"),
    ("британию", "GB"),
    ("британией", "GB"),
    // Бразилия (BR)
    ("бразилии", "BR"),
    ("бразилию", "BR"),
    ("бразилией", "BR"),
    // Германия (DE)
    ("германии", "DE"),
    ("германию", "DE"),
    ("германией", "DE"),
    // Испания (ES)
    ("испании", "ES"),
    ("испанию", "ES"),
    ("испанией", "ES"),
    // Италия (IT)
    ("италии", "IT"),
    ("италию", "IT"),
    ("италией", "IT"),
    // Китай (CN)
    ("китая", "CN"),
    ("китаю", "CN"),
    ("китаем", "CN"),
    ("китае", "CN"),
    // Польша (PL)
    ("польши", "PL"),
    ("польше", "PL"),
    ("польшу", "PL"),
    ("польшей", "PL"),
    // Россия (RU)
    ("россии", "RU"),
    ("россию", "RU"),
    ("россией", "RU"),
    // США / Америка (US)
    ("америки", "US"),
    ("америке", "US"),
    ("америку", "US"),
    ("америкой", "US"),
    // Турция (TR)
    ("турции", "TR"),
    ("турцию", "TR"),
    ("турцией", "TR"),
    // Украина (UA)
    ("украины", "UA"),
    ("украине", "UA"),
    ("украину", "UA"),
    ("украиной", "UA"),
    // Франция (FR)
    ("франции", "FR"),
    ("францию", "FR"),
    ("францией", "FR"),
    // Швейцария (CH)
    ("швейцарии", "CH"),
    ("швейцарию", "CH"),
    ("швейцарией", "CH"),
    // Япония (JP)
    ("японии", "JP"),
    ("японию", "JP"),
    ("японией", "JP"),
];

#[cfg(test)]
mod tests {
    use super::{
        QueryIntent, SearchAction, deterministic_intent, normalize_query,
        station_name_hint_queries, validate_intent,
    };

    #[test]
    fn provider_intent_is_normalized_and_deduplicated() {
        let intent = validate_intent(QueryIntent {
            action: SearchAction::Play,
            terms: vec![" Jazz ".to_owned(), "jazz".to_owned()],
            tags: vec![" Calm ".to_owned()],
            language: Some("EN".to_owned()),
            country_code: Some("us".to_owned()),
            core_term_count: 0,
            raw_query: String::new(),
        })
        .unwrap();

        assert!(intent.terms.contains(&"jazz".to_owned()));
        assert!(intent.terms.contains(&"джаз".to_owned()));
        assert_eq!(intent.tags, ["calm"]);
        assert_eq!(intent.language.as_deref(), Some("en"));
        assert_eq!(intent.country_code.as_deref(), Some("US"));
        assert_eq!(intent.core_term_count, 1);
        assert_eq!(intent.raw_query, "jazz");
    }

    #[test]
    fn invalid_provider_hard_filter_is_rejected() {
        let error = validate_intent(QueryIntent {
            action: SearchAction::Play,
            terms: vec!["jazz".to_owned()],
            tags: Vec::new(),
            language: Some("english".to_owned()),
            country_code: None,
            core_term_count: 0,
            raw_query: String::new(),
        })
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "query parser returned an invalid language filter"
        );
    }

    #[test]
    fn locale_does_not_create_language_or_country_filters() {
        let intent = deterministic_intent("включи медленный джаз", "ru-RU");
        assert_eq!(intent.language, None);
        assert_eq!(intent.country_code, None);
    }

    #[test]
    fn natural_language_fallback_does_not_guess_catalog_tags() {
        let intent = deterministic_intent("русская народная музыка", "ru-RU");
        assert_eq!(intent.language, None);
        assert_eq!(intent.country_code, None);
    }

    #[test]
    fn country_filter_recognizes_country_names_and_rejects_demonyms() {
        for (request, expected_country_code) in [
            ("включи рок из Германии", Some("DE")),
            ("Бразилия радио", Some("BR")),
            ("Эмираты поп", Some("AE")),
            ("Включи немецкий рок", None),
            ("Japanese jazz", None),
            ("south african metal", None),
        ] {
            assert_eq!(
                deterministic_intent(request, "ru-RU")
                    .country_code
                    .as_deref(),
                expected_country_code,
                "{request}"
            );
        }
    }

    #[test]
    fn table_a_deterministic_acceptance_queries() {
        let cases: &[(&str, Option<&str>, Option<&str>, usize)] = &[
            ("вруби станцию из германии", None, Some("DE"), 3),
            ("станции германии", None, Some("DE"), 2),
            ("рок из австрии", None, Some("AT"), 3),
            ("вруби немецкий рок", None, None, 2),
            ("рок на немецком", Some("de"), None, 3),
            ("рок по-немецки", Some("de"), None, 3),
            ("немецкоязычное радио", Some("de"), None, 2),
            ("рок in German", Some("de"), None, 3),
            ("включи джаз", None, None, 1),
            ("американский рок", None, None, 2),
            ("Включи английский рок", None, None, 2),
            ("русскоязычный рок", Some("ru"), None, 2),
            ("рок из прошлого", None, None, 3),
            ("немецкий рок из австрии", None, Some("AT"), 4),
        ];

        for &(query_str, expected_lang, expected_country, expected_core_terms) in cases {
            let intent = deterministic_intent(query_str, "ru-RU");
            assert_eq!(
                intent.language.as_deref(),
                expected_lang,
                "deterministic_intent language mismatch for: {query_str}"
            );
            assert_eq!(
                intent.country_code.as_deref(),
                expected_country,
                "deterministic_intent country mismatch for: {query_str}"
            );
            assert_eq!(
                intent.core_term_count, expected_core_terms,
                "deterministic_intent core_term_count mismatch for: {query_str}"
            );

            let normalized = normalize_query(query_str.to_owned(), "ru-RU".to_owned());
            assert_eq!(
                normalized.language.as_deref(),
                expected_lang,
                "normalize_query language mismatch for: {query_str}"
            );
            assert_eq!(
                normalized.country_code.as_deref(),
                expected_country,
                "normalize_query country mismatch for: {query_str}"
            );
            assert_eq!(
                normalized.core_term_count, expected_core_terms,
                "normalize_query core_term_count mismatch for: {query_str}"
            );
        }
    }

    #[test]
    fn prepositional_language_requires_end_of_query_or_language_word() {
        // Negative tests: prepositional language form not at end and not followed by language/языке/языка
        assert_eq!(
            deterministic_intent("рок на немецком фестивале", "ru-RU").language,
            None
        );
        assert_eq!(
            deterministic_intent("включи радио Tune In German Rock", "ru-RU").language,
            None
        );

        // Positive tests: at end of query or followed by language word
        assert_eq!(
            deterministic_intent("рок на немецком", "ru-RU")
                .language
                .as_deref(),
            Some("de")
        );
        assert_eq!(
            deterministic_intent("песни на немецком языке", "ru-RU")
                .language
                .as_deref(),
            Some("de")
        );
        assert_eq!(
            deterministic_intent("рок in German", "ru-RU")
                .language
                .as_deref(),
            Some("de")
        );
        assert_eq!(
            deterministic_intent("rock in german language", "ru-RU")
                .language
                .as_deref(),
            Some("de")
        );
    }

    #[test]
    fn country_filter_recognizes_russian_genitive_for_england() {
        let intent = deterministic_intent("Включи рок из Англии", "ru-RU");

        assert_eq!(intent.country_code.as_deref(), Some("GB"));
    }

    #[test]
    fn ambiguous_country_terms_do_not_create_a_hard_filter() {
        assert_eq!(
            deterministic_intent("рок из Конго", "ru-RU").country_code,
            None
        );
    }

    #[test]
    fn stop_words_are_removed() {
        let intent = deterministic_intent("включи радио диджей", "ru-RU");
        assert!(!intent.terms.contains(&"включи".to_owned()));
        assert!(intent.terms.contains(&"радио".to_owned()));
        assert!(intent.terms.contains(&"диджей".to_owned()));
        assert_eq!(intent.core_term_count, 2);
        assert_eq!(intent.raw_query, "радио диджей");
    }

    #[test]
    fn deterministic_intent_keeps_terms_unexpanded() {
        let intent = deterministic_intent("включи радио диджей", "ru-RU");
        assert_eq!(intent.terms, ["радио", "диджей"]);
        assert!(!intent.terms.contains(&"radio".to_owned()));
        assert!(!intent.terms.contains(&"dj".to_owned()));
        assert_eq!(intent.core_term_count, 2);
        assert_eq!(intent.raw_query, "радио диджей");
    }

    #[test]
    fn transliteration_expands_terms() {
        let query = normalize_query("включи радио диджей".to_owned(), "ru-RU".to_owned());
        assert!(query.terms.contains(&"radio".to_owned()));
        assert!(query.terms.contains(&"dj".to_owned()));
        assert_eq!(query.core_term_count, 2);
        assert_eq!(query.raw_query, "радио диджей");
    }

    #[test]
    fn transliteration_expands_common_station_tokens() {
        let query = normalize_query(
            "включи радио ультра рокс викер боб год".to_owned(),
            "ru-RU".to_owned(),
        );
        assert!(query.terms.contains(&"ультра".to_owned()));
        assert!(query.terms.contains(&"ultra".to_owned()));
        assert!(query.terms.contains(&"рокс".to_owned()));
        assert!(query.terms.contains(&"roks".to_owned()));
        assert!(query.terms.contains(&"викер".to_owned()));
        assert!(query.terms.contains(&"viker".to_owned()));
        assert!(query.terms.contains(&"боб".to_owned()));
        assert!(query.terms.contains(&"bob".to_owned()));
        assert!(query.terms.contains(&"год".to_owned()));
        assert!(query.terms.contains(&"god".to_owned()));
    }

    #[test]
    fn raw_query_preserves_word_order_without_duplicates() {
        let query_a = normalize_query("включи рок немецкий".to_owned(), "ru-RU".to_owned());
        assert_eq!(query_a.raw_query, "рок немецкий");
        assert_eq!(query_a.core_term_count, 2);

        let query_b = normalize_query("включи немецкий рок".to_owned(), "ru-RU".to_owned());
        assert_eq!(query_b.raw_query, "немецкий рок");
        assert_eq!(query_b.core_term_count, 2);

        let query_dup = normalize_query("рок рок немецкий рок".to_owned(), "ru-RU".to_owned());
        assert_eq!(query_dup.raw_query, "рок немецкий");
        assert_eq!(query_dup.core_term_count, 2);
    }

    #[test]
    fn normalize_query_single_normalization_no_duplicate_transliteration_artifacts() {
        let query = normalize_query("включи немецкий рок".to_owned(), "ru-RU".to_owned());
        assert_eq!(query.core_term_count, 2);
        assert_eq!(query.raw_query, "немецкий рок");
        assert!(query.terms.contains(&"рок".to_owned()));
        assert!(query.terms.contains(&"rock".to_owned()));
        assert!(!query.terms.contains(&"рокк".to_owned()));
    }

    #[test]
    fn validate_intent_preserves_cased_llm_compound_terms() {
        let intent = validate_intent(QueryIntent {
            action: SearchAction::Play,
            terms: vec!["RockRadio".to_owned()],
            tags: Vec::new(),
            language: None,
            country_code: None,
            core_term_count: 0,
            raw_query: String::new(),
        })
        .unwrap();

        assert_eq!(intent.core_term_count, 1);
        assert_eq!(intent.raw_query, "rockradio");
        assert!(intent.terms.contains(&"rockradio".to_owned()));
    }

    #[test]
    fn station_name_mode_keeps_ordered_phrase_after_vklyuchi_radio() {
        let hints = station_name_hint_queries("Включи радио рок фм");
        assert!(hints.contains(&"рок фм".to_owned()));
        assert!(
            hints
                .iter()
                .any(|hint| hint.contains("rok") || hint.contains("rock"))
        );
    }

    #[test]
    fn camel_case_split_works() {
        use super::split_camel_case;
        assert_eq!(split_camel_case("radioDJ"), vec!["radio", "DJ"]);
        assert_eq!(split_camel_case("HelloWorld"), vec!["Hello", "World"]);
        assert_eq!(split_camel_case("XMLParser"), vec!["XML", "Parser"]);
        assert_eq!(split_camel_case("simple"), vec!["simple"]);
    }

    #[test]
    fn tokenize_splits_camel_case_names() {
        use super::tokenize;
        let tokens = tokenize("radioDJ");
        assert!(tokens.contains(&"radio".to_owned()));
        assert!(tokens.contains(&"dj".to_owned()));
    }
}
