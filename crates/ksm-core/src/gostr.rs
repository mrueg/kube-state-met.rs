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
    // Greek with ypogegrammeni: the full mapping is two runes, the simple
    // one (Go's) the matching prosgegrammeni letter.
    let simple = match c as u32 {
        0x1F80..=0x1F87 | 0x1F90..=0x1F97 | 0x1FA0..=0x1FA7 => Some(c as u32 + 8),
        0x1FB3 | 0x1FC3 | 0x1FF3 => Some(c as u32 + 9),
        _ => None,
    };
    if let Some(u) = simple.and_then(char::from_u32) {
        return u;
    }
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
        // Georgian Mkhedruli has an upper case (Mtavruli) but is its own
        // title case.
        '\u{10D0}'..='\u{10FA}' | '\u{10FD}'..='\u{10FF}' => c,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Go-verified (`unicode.ToUpper`, `unicode.ToTitle`).
    #[test]
    fn simple_case_mapping() {
        assert_eq!(to_upper_rune('ᾀ'), '\u{1F88}');
        assert_eq!(to_upper_rune('ᾳ'), '\u{1FBC}');
        assert_eq!(to_upper_rune('ῃ'), '\u{1FCC}');
        assert_eq!(to_upper_rune('ῳ'), '\u{1FFC}');
        assert_eq!(to_upper_rune('ß'), 'ß');
        assert_eq!(to_title_rune('ა'), 'ა');
        assert_eq!(to_upper_rune('ა'), '\u{1C90}');
        assert_eq!(to_lower("İΣ"), "iσ");
    }
}
