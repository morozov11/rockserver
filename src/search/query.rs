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
pub fn normalize_query(original: String, locale: String) -> SearchQuery {
    let intent = deterministic_intent(&original, &locale);
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

pub(super) fn validate_intent(intent: QueryIntent) -> Result<QueryIntent, QueryParserError> {
    // LLM can return multi-word `terms` like "викер радио" or include symbols.
    // We must normalize them into atomic matchable tokens using `tokenize()`.
    let mut terms = normalize_values(intent.terms);
    terms = terms
        .into_iter()
        .flat_map(|term| tokenize(&term))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    remove_stop_words(&mut terms);

    let raw_query = terms.join(" ");
    let core_term_count = terms.len();

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

pub(super) fn deterministic_intent(original: &str, _locale: &str) -> QueryIntent {
    let mut terms = tokenize(original);
    let country_code = infer_country_code(&terms);
    let language = infer_language(&terms);
    remove_stop_words(&mut terms);
    let raw_query = terms.join(" ");
    let core_term_count = terms.len();
    expand_transliterations(&mut terms);
    let tags = canonical_tags(terms.clone());

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

fn normalize_values(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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

/// Removes command verbs from query terms so they don't dilute match scores.
pub(super) fn remove_stop_words(terms: &mut Vec<String>) {
    terms.retain(|term| !STOP_WORDS.contains(&term.as_str()));
}

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

fn infer_language(terms: &[String]) -> Option<String> {
    let term_set = terms.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if term_set.contains("русскоязычный") || term_set.contains("русскоязычная")
    {
        Some("ru".to_owned())
    } else if term_set.contains("англоязычный") || term_set.contains("англоязычная")
    {
        Some("en".to_owned())
    } else {
        None
    }
}

pub(super) fn infer_country_code(terms: &[String]) -> Option<String> {
    // Country is a hard filter only when a name or demonym occurs in the command.
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

/// Returns whether a request explicitly asks for stations from a country.
///
/// Only unambiguous source-country prepositions are recognized. This prevents
/// cultural adjectives such as "английский рок" from becoming country filters.
pub(super) fn has_explicit_country_request(terms: &[String]) -> bool {
    terms
        .windows(2)
        .any(|window| matches!(window[0].as_str(), "из" | "from") && !window[1].is_empty())
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

// ISO 3166-1 alpha-2 countries with English/Russian names and common demonyms.
// Ambiguous forms such as "конго" are intentionally omitted.
const COUNTRY_ALIASES: &str = "AF=afghanistan;афганистан;афганский;afghan|AL=albania;албания;албанский;albanian|DZ=algeria;алжир;алжирский;algerian|AD=andorra;андорра;andorran|AO=angola;ангола;angolan|AG=antigua and barbuda;антигуа и барбуда|AR=argentina;аргентина;аргентинский;argentinian|AM=armenia;армения;армянский;armenian|AU=australia;австралия;австралийский;australian|AT=austria;австрия;австрийский;austrian|AZ=azerbaijan;азербайджан;азербайджанский|BS=bahamas;багамы;bahamian|BH=bahrain;бахрейн;bahraini|BD=bangladesh;бангладеш;bangladeshi|BB=barbados;барбадос;barbadian|BY=belarus;беларусь;белоруссия;белорусский;belarusian|BE=belgium;бельгия;бельгийский;belgian|BZ=belize;белиз;belizean|BJ=benin;бенин;beninese|BT=bhutan;бутан;bhutanese|BO=bolivia;боливия;bolivian|BA=bosnia and herzegovina;босния и герцеговина;bosnian|BW=botswana;ботсвана;botswanan|BR=brazil;бразилия;бразильский;brazilian|BN=brunei;бруней;bruneian|BG=bulgaria;болгария;болгарский;bulgarian|BF=burkina faso;буркина фасо|BI=burundi;бурунди;burundian|CV=cabo verde;cape verde;кабо верде|KH=cambodia;камбоджа;cambodian|CM=cameroon;камерун;cameroonian|CA=canada;канада;канадский;canadian|CF=central african republic;центральноафриканская республика|TD=chad;чад;chadian|CL=chile;чили;chilean|CN=china;китай;китайский;chinese|CO=colombia;колумбия;colombian|KM=comoros;коморы;comorian|CG=republic of congo;республика конго|CD=democratic republic of congo;демократическая республика конго|CR=costa rica;коста рика;costa rican|CI=cote d ivoire;ivory coast;кот д ивуар;ivorian|HR=croatia;хорватия;croatian|CU=cuba;куба;cuban|CY=cyprus;кипр;cypriot|CZ=czechia;czech republic;чехия;czech|DK=denmark;дания;данский;danish|DJ=djibouti;джибути|DM=dominica;доминика|DO=dominican republic;доминиканская республика|EC=ecuador;эквадор;ecuadorian|EG=egypt;египет;египетский;egyptian|SV=el salvador;сальвадор;salvadoran|GQ=equatorial guinea;экваториальная гвинея|ER=eritrea;эритрея;eritrean|EE=estonia;эстония;эстонский;estonian|SZ=eswatini;свазиленд;эсватини|ET=ethiopia;эфиопия;ethiopian|FJ=fiji;фиджи;fijian|FI=finland;финляндия;финский;finnish|FR=france;франция;французский;french|GA=gabon;габон;gabonese|GM=gambia;гамбия;gambian|GE=georgia;грузия;грузинский;georgian|DE=germany;германия;немецкий;немецкая;немецкое;немецкие;german|GH=ghana;гана;ghanaian|GR=greece;греция;греческий;greek|GD=grenada;гренада|GT=guatemala;гватемала|GN=guinea;гвинея|GW=guinea bissau;гвинея бисау|GY=guyana;гайана|HT=haiti;гаити;haitian|VA=holy see;vatican;ватикан|HN=honduras;гондурас;honduran|HU=hungary;венгрия;венгерский;hungarian|IS=iceland;исландия;icelandic|IN=india;индия;индийский;indian|ID=indonesia;индонезия;indonesian|IR=iran;иран;iranian|IQ=iraq;ирак;iraqi|IE=ireland;ирландия;irish|IL=israel;израиль;israeli|IT=italy;италия;итальянский;italian|JM=jamaica;ямайка;jamaican|JP=japan;япония;японский;japanese|JO=jordan;иордания;jordanian|KZ=kazakhstan;казахстан;казахский;kazakh|KE=kenya;кения;kenyan|KI=kiribati;кирибати|KP=north korea;северная корея;северокорейский|KR=south korea;южная корея;южнокорейский|KW=kuwait;кувейт;kuwaiti|KG=kyrgyzstan;киргизия;кыргызстан;kyrgyz|LA=laos;лаос|LV=latvia;латвия;latvian|LB=lebanon;ливан;lebanese|LS=lesotho;лесото|LR=liberia;либерия;liberian|LY=libya;ливия;libyan|LI=liechtenstein;лихтенштейн|LT=lithuania;литва;lithuanian|LU=luxembourg;люксембург|MG=madagascar;мадагаскар;malagasy|MW=malawi;малави|MY=malaysia;малайзия;malaysian|MV=maldives;мальдивы|ML=mali;мали|MT=malta;мальта;maltese|MH=marshall islands;маршалловы острова|MR=mauritania;мавритания|MU=mauritius;маврикий|MX=mexico;мексика;мексиканский;mexican|FM=micronesia;микронезия|MD=moldova;молдова;moldovan|MC=monaco;монако|MN=mongolia;монголия;mongolian|ME=montenegro;черногория|MA=morocco;марокко;moroccan|MZ=mozambique;мозамбик|MM=myanmar;мьянма;бирма;burmese|NA=namibia;намибия|NR=nauru;науру|NP=nepal;непал;nepali|NL=netherlands;holland;нидерланды;голландия;dutch|NZ=new zealand;новая зеландия|NI=nicaragua;никарагуа;nicaraguan|NE=niger;нигер;nigerien|NG=nigeria;нигерия;nigerian|MK=north macedonia;северная македония;macedonian|NO=norway;норвегия;норвежский;norwegian|OM=oman;оман;omani|PK=pakistan;пакистан;pakistani|PW=palau;палау|PS=palestine;палестина;palestinian|PA=panama;панама;panamanian|PG=papua new guinea;папуа новая гвинея|PY=paraguay;парагвай;paraguayan|PE=peru;перу;peruvian|PH=philippines;филиппины;filipino|PL=poland;польша;польский;polish|PT=portugal;португалия;португальский;portuguese|QA=qatar;катар;qatari|RO=romania;румыния;румынский;romanian|RU=russia;россия;россии;российский;российская;русский;russian|RW=rwanda;руанда;rwandan|KN=saint kitts and nevis;сент китс и невис|LC=saint lucia;сент люсия|VC=saint vincent and the grenadines;сент винсент и гренадины|WS=samoa;самоа;samoan|SM=san marino;сан марино|ST=sao tome and principe;сан томе и принсипи|SA=saudi arabia;саудовская аравия;saudi|SN=senegal;сенегал;senegalese|RS=serbia;сербия;сербский;serbian|SC=seychelles;сейшелы|SL=sierra leone;сьерра леоне|SG=singapore;сингапур;singaporean|SK=slovakia;словакия;slovak|SI=slovenia;словения;slovenian|SB=solomon islands;соломоновы острова|SO=somalia;сомали;somali|ZA=south africa;южная африка;южноафриканский;south african|SS=south sudan;южный судан|ES=spain;испания;испанский;spanish|LK=sri lanka;шри ланка|SD=sudan;судан;sudanese|SR=suriname;суринам|SE=sweden;швеция;шведский;swedish|CH=switzerland;швейцария;швейцарский;swiss|SY=syria;сирия;syrian|TJ=tajikistan;таджикистан;tajik|TZ=tanzania;танзания;tanzanian|TH=thailand;таиланд;тайланд;тайский;thai|TL=timor leste;east timor;восточный тимор|TG=togo;того|TO=tonga;тонга|TT=trinidad and tobago;тринидад и тобаго|TN=tunisia;тунис;tunisian|TR=turkey;türkiye;турция;турецкий;turkish|TM=turkmenistan;туркменистан;turkmen|TV=tuvalu;тувалу|UG=uganda;уганда;ugandan|UA=ukraine;украина;украинский;ukrainian|AE=united arab emirates;объединенные арабские эмираты;эмираты;emirati|GB=united kingdom;great britain;britain;великобритания;британия;соединенное королевство;англия;британский;british;uk|US=united states;united states of america;сша;соединенные штаты;америка;американский;american;usa|UY=uruguay;уругвай;uruguayan|UZ=uzbekistan;узбекистан;узбекский;uzbek|VU=vanuatu;вануату|VE=venezuela;венесуэла;venezuelan|VN=vietnam;вьетнам;вьетнамский;vietnamese|YE=yemen;йемен;yemeni|ZM=zambia;замбия;zambian|ZW=zimbabwe;зимбабве;zimbabwean";

/// Russian forms that cannot be safely derived by suffix stripping.
const RUSSIAN_COUNTRY_INFLECTIONS: &[(&str, &str)] = &[
    ("англии", "GB"),
    ("германии", "DE"),
    ("германию", "DE"),
    ("германией", "DE"),
];

#[cfg(test)]
mod tests {
    use super::{
        QueryIntent, SearchAction, deterministic_intent, station_name_hint_queries, validate_intent,
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
    fn country_filter_recognizes_world_country_names_and_demonyms() {
        for (request, expected_country_code) in [
            ("Включи немецкий рок", "DE"),
            ("включи рок из Германии", "DE"),
            ("Japanese jazz", "JP"),
            ("Бразилия радио", "BR"),
            ("south african metal", "ZA"),
            ("Эмираты поп", "AE"),
        ] {
            assert_eq!(
                deterministic_intent(request, "ru-RU")
                    .country_code
                    .as_deref(),
                Some(expected_country_code),
                "{request}"
            );
        }
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
    }

    #[test]
    fn transliteration_expands_terms() {
        let intent = deterministic_intent("включи радио диджей", "ru-RU");
        assert!(intent.terms.contains(&"radio".to_owned()));
        assert!(intent.terms.contains(&"dj".to_owned()));
    }

    #[test]
    fn transliteration_expands_common_station_tokens() {
        let intent = deterministic_intent("включи радио ультра рокс викер боб год", "ru-RU");
        assert!(intent.terms.contains(&"ультра".to_owned()));
        assert!(intent.terms.contains(&"ultra".to_owned()));
        assert!(intent.terms.contains(&"рокс".to_owned()));
        assert!(intent.terms.contains(&"roks".to_owned()));
        assert!(intent.terms.contains(&"викер".to_owned()));
        assert!(intent.terms.contains(&"viker".to_owned()));
        assert!(intent.terms.contains(&"боб".to_owned()));
        assert!(intent.terms.contains(&"bob".to_owned()));
        assert!(intent.terms.contains(&"год".to_owned()));
        assert!(intent.terms.contains(&"god".to_owned()));
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
