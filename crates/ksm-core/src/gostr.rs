//! Go string helpers: `strings.ToLower`/`ToUpper` (simple, one rune to one
//! rune case mapping), `unicode.IsSpace`, `unicode.ToTitle`.

/// Go's `unicode.IsSpace` (the Unicode White_Space property).
pub fn is_space(c: char) -> bool {
    c.is_whitespace()
}

fn single(mut it: impl Iterator<Item = char>, c: char) -> char {
    match (it.next(), it.next()) {
        (Some(m), None) => m,
        _ => c,
    }
}

/// `unicode.ToUpper` of one rune: Go maps runes one to one, so a rune whose
/// full uppercase mapping is several runes (`ß`) stays as it is.
pub fn to_upper_rune(c: char) -> char {
    single(c.to_uppercase(), c)
}

/// `unicode.ToLower` of one rune.
pub fn to_lower_rune(c: char) -> char {
    if c == '\u{130}' {
        // LATIN CAPITAL LETTER I WITH DOT ABOVE: Go's simple mapping is 'i'.
        return 'i';
    }
    single(c.to_lowercase(), c)
}

/// `unicode.ToTitle` of one rune (title case is upper case outside the few
/// digraphs, which have no single-rune upper case either).
pub fn to_title_rune(c: char) -> char {
    match c {
        'Ǆ' | 'ǅ' | 'ǆ' => 'ǅ',
        'Ǉ' | 'ǈ' | 'ǉ' => 'ǈ',
        'Ǌ' | 'ǋ' | 'ǌ' => 'ǋ',
        'Ǳ' | 'ǲ' | 'ǳ' => 'ǲ',
        _ => to_upper_rune(c),
    }
}

/// `strings.ToUpper`.
pub fn to_upper(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_uppercase();
    }
    s.chars().map(to_upper_rune).collect()
}

/// `strings.ToLower`.
pub fn to_lower(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    s.chars().map(to_lower_rune).collect()
}
