//! Transliteration utilities between Russian Cyrillic and Latin scripts.

/// Returns whether any character in the term belongs to the Russian Cyrillic alphabet or is 'ё'.
pub fn is_cyrillic_token(term: &str) -> bool {
    term.chars().any(is_cyrillic_char)
}

/// Returns whether any character in the term is an ASCII alphabetic letter.
pub fn is_latin_token(term: &str) -> bool {
    term.chars().any(|ch| ch.is_ascii_alphabetic())
}

/// Returns whether a character is in the Russian lowercase Cyrillic range or is 'ё'.
pub fn is_cyrillic_char(ch: char) -> bool {
    ('а'..='я').contains(&ch) || ch == 'ё'
}

/// Generic transliteration from Russian Cyrillic to Latin.
///
/// This complements dictionary mappings so station names like `боб` can match
/// `bob` without explicit per-word entries.
pub fn transliterate_ru_to_lat(term: &str) -> String {
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
pub fn transliterate_lat_to_ru(term: &str) -> String {
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
