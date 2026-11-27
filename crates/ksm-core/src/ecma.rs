//! Translates a pattern as `regexp2` reads it in ECMAScript mode (Go's allow
//! and deny lists) into an equivalent `fancy-regex` pattern, following
//! docs/port/research/regex.md §3: Annex-B identity escapes become literals,
//! invalid `{` becomes literal, legacy octal escapes, ECMAScript class syntax.
//! Constructs regexp2 rejects are rejected.

fn is_hex(c: char) -> bool {
    c.is_ascii_hexdigit()
}

/// A literal character, escaped for fancy-regex.
fn lit(out: &mut String, c: char) {
    // `<` and `>` are not special in either engine, and `\<`/`\>` are word
    // boundaries in fancy-regex.
    if c.is_ascii_alphanumeric() || matches!(c, '_' | '<' | '>') {
        out.push(c);
    } else if (c as u32) < 0x20 || c == '\u{7f}' {
        out.push_str(&format!("\\x{{{:X}}}", c as u32));
    } else if c.is_ascii() {
        out.push('\\');
        out.push(c);
    } else {
        out.push(c);
    }
}

/// The capturing groups: their number and the names of the named ones;
/// an error for a construct regexp2 rejects.
fn scan_groups(p: &[char]) -> Result<Groups, String> {
    let mut n = 0;
    let mut names: Vec<String> = Vec::new();
    let mut i = 0;
    let mut in_class = false;
    while i < p.len() {
        match p[i] {
            '\\' => i += 1,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' if !in_class => {
                if p.get(i + 1) == Some(&'?') {
                    let rest: String = p[i + 2..].iter().take(2).collect();
                    if rest.starts_with("P<") {
                        return Err("unrecognized grouping construct (?P<".into());
                    }
                    let close = match p.get(i + 2) {
                        Some('<') if !matches!(p.get(i + 3), Some('=' | '!')) => Some('>'),
                        Some('\'') => Some('\''),
                        _ => None,
                    };
                    if let Some(close) = close {
                        let name: String = p[i + 3..].iter().take_while(|c| **c != close).collect();
                        if names.contains(&name) {
                            return Err(format!("duplicate group name {name}"));
                        }
                        names.push(name);
                        n += 1;
                    }
                } else {
                    n += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    Ok(Groups { count: n, names })
}

/// Parses `{n}`, `{n,}` or `{n,m}` at `p[i]`; returns its length.
fn quantifier_len(p: &[char], i: usize) -> Option<usize> {
    let mut j = i + 1;
    let start = j;
    while j < p.len() && p[j].is_ascii_digit() {
        j += 1;
    }
    if j == start {
        return None;
    }
    if p.get(j) == Some(&',') {
        j += 1;
        while j < p.len() && p[j].is_ascii_digit() {
            j += 1;
        }
    }
    (p.get(j) == Some(&'}')).then_some(j + 1 - i)
}

/// The pattern's capturing groups.
struct Groups {
    count: usize,
    names: Vec<String>,
}

/// Translates the escape at `p[i]` (`p[i] == '\\'`); returns chars consumed.
fn escape(
    p: &[char],
    i: usize,
    groups: &Groups,
    in_class: bool,
    out: &mut String,
) -> Result<usize, String> {
    let Some(&c) = p.get(i + 1) else {
        out.push_str("\\\\");
        return Ok(1);
    };
    match c {
        'd' | 'D' | 'w' | 'W' | 's' | 'S' | 'f' | 'n' | 'r' | 't' | 'v' => {
            out.push('\\');
            out.push(c);
            Ok(2)
        }
        'b' if in_class => {
            out.push_str("\\x{8}");
            Ok(2)
        }
        'b' | 'B' => {
            out.push('\\');
            out.push(c);
            Ok(2)
        }
        // Annex B: in a class `\c` also takes a digit or `_`.
        'c' => match p.get(i + 2) {
            Some(l)
                if l.is_ascii_alphabetic() || (in_class && (l.is_ascii_digit() || *l == '_')) =>
            {
                lit(out, char::from((*l as u8) % 32));
                Ok(3)
            }
            _ => {
                out.push_str("\\\\");
                Ok(1)
            }
        },
        'x' => {
            if p.get(i + 2).copied().is_some_and(is_hex)
                && p.get(i + 3).copied().is_some_and(is_hex)
            {
                out.push_str("\\x");
                out.push(p[i + 2]);
                out.push(p[i + 3]);
                Ok(4)
            } else {
                out.push('x');
                Ok(2)
            }
        }
        'u' => {
            if (2..6).all(|k| p.get(i + k).copied().is_some_and(is_hex)) {
                let hex: String = p[i + 2..i + 6].iter().collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    lit(out, ch);
                } else {
                    out.push_str(&format!("\\x{{{hex}}}"));
                }
                Ok(6)
            } else {
                out.push('u');
                Ok(2)
            }
        }
        // Without named groups `\k` is an identity escape; with them it must
        // name one. A group that took no part in the match matches empty
        // (ECMAScript), which fancy-regex spells as a conditional.
        'k' if !in_class && p.get(i + 2) == Some(&'<') && !groups.names.is_empty() => {
            let name: String = p[i + 3..].iter().take_while(|c| **c != '>').collect();
            if !groups.names.contains(&name) {
                return Err(format!("reference to undefined group name {name}"));
            }
            out.push_str(&format!("(?(<{name}>)\\k<{name}>)"));
            Ok(3 + name.chars().count() + 1)
        }
        '0'..='9' => {
            let digits: String = p[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            let n: usize = digits.parse().unwrap_or(usize::MAX);
            if c != '0' && !in_class && n <= groups.count {
                out.push_str(&format!("(?({digits})\\{digits})"));
                return Ok(1 + digits.len());
            }
            // Legacy octal: up to three octal digits (value <= 0o377), else a literal digit.
            let oct: String = p[i + 1..]
                .iter()
                .take(3)
                .take_while(|c| ('0'..='7').contains(*c))
                .collect();
            if oct.is_empty() {
                lit(out, c);
                return Ok(2);
            }
            let mut take = oct.len();
            while take > 0 && u32::from_str_radix(&oct[..take], 8).unwrap_or(256) > 0o377 {
                take -= 1;
            }
            let v = u32::from_str_radix(&oct[..take], 8).unwrap_or(0);
            lit(out, char::from_u32(v).unwrap_or('\0'));
            Ok(1 + take)
        }
        c => {
            lit(out, c);
            Ok(2)
        }
    }
}

fn is_class_escape(p: &[char], i: usize) -> bool {
    p.get(i) == Some(&'\\') && matches!(p.get(i + 1), Some('d' | 'D' | 'w' | 'W' | 's' | 'S'))
}

/// Translates a character class starting at `p[i] == '['`; returns chars consumed.
fn class(p: &[char], i: usize, groups: &Groups, out: &mut String) -> Result<usize, String> {
    let mut j = i + 1;
    let negated = p.get(j) == Some(&'^');
    if negated {
        j += 1;
    }
    if p.get(j) == Some(&']') {
        out.push_str(if negated { "[\\s\\S]" } else { "(?!)" });
        return Ok(j + 1 - i);
    }
    out.push('[');
    if negated {
        out.push('^');
    }
    let mut prev_class_escape = false;
    while j < p.len() {
        let c = p[j];
        match c {
            ']' => {
                out.push(']');
                return Ok(j + 1 - i);
            }
            '\\' => {
                prev_class_escape = is_class_escape(p, j);
                j += escape(p, j, groups, true, out)?;
                continue;
            }
            '-' => {
                let at_edge =
                    out.ends_with('[') || out.ends_with('^') || p.get(j + 1) == Some(&']');
                if at_edge || prev_class_escape || is_class_escape(p, j + 1) || out.ends_with('-') {
                    out.push_str("\\-");
                } else {
                    out.push('-');
                }
            }
            '[' | '&' | '~' => {
                out.push('\\');
                out.push(c);
            }
            c => lit(out, c),
        }
        prev_class_escape = false;
        j += 1;
    }
    Err("unterminated [] set".into())
}

/// The fancy-regex pattern matching what `regexp2` (ECMAScript) matches.
pub fn ecma_to_fancy(pattern: &str) -> Result<String, String> {
    let p: Vec<char> = pattern.chars().collect();
    let groups = &scan_groups(&p)?;
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut i = 0;
    let mut after_quantifier = false;
    while i < p.len() {
        let c = p[i];
        if after_quantifier && c == '+' {
            return Err("nested quantifier '+'".into());
        }
        after_quantifier = false;
        match c {
            '\\' => i += escape(&p, i, groups, false, &mut out)?,
            '[' => i += class(&p, i, groups, &mut out)?,
            '{' => match quantifier_len(&p, i) {
                Some(n) if i > 0 => {
                    out.extend(&p[i..i + n]);
                    i += n;
                    if p.get(i) == Some(&'?') {
                        out.push('?');
                        i += 1;
                    }
                    after_quantifier = true;
                }
                _ => {
                    out.push_str("\\{");
                    i += 1;
                }
            },
            '}' => {
                out.push_str("\\}");
                i += 1;
            }
            '*' | '+' | '?' => {
                out.push(c);
                i += 1;
                if p.get(i) == Some(&'?') {
                    out.push('?');
                    i += 1;
                }
                after_quantifier = true;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str, input: &str) -> Result<bool, String> {
        let t = ecma_to_fancy(pattern)?;
        let re = fancy_regex::Regex::new(&t).map_err(|e| format!("{t}: {e}"))?;
        re.is_match(input).map_err(|e| e.to_string())
    }

    #[test]
    fn go_verified_vectors() {
        // docs/port/research/regex.md §7 and §3.
        let cases: &[(&str, &str, bool)] = &[
            ("kube_pod_.*", "kube_pod_info", true),
            ("kube_pod_.*", "xkube_pod_info", true),
            ("kube_pod_.*", "kube_node_info", false),
            ("^kube_pod_info$", "kube_pod_info_x", false),
            ("^kube_(?!pod_).*", "kube_pod_info", false),
            ("^kube_(?!pod_).*", "kube_node_info", true),
            ("(?<!kube_)pod", "kube_pod", false),
            ("(?<!kube_)pod", "x_pod", true),
            ("^a\\z", "a", false),
            ("^a\\z", "az", true),
            ("^\\Aa", "a", false),
            ("^\\Aa", "Aa", true),
            ("^\\h$", "h", true),
            ("^[[:alpha:]]$", "a", false),
            ("^[[:alpha:]]$", "[]", true),
            ("^a{,3}$", "a{,3}", true),
            ("^a{,3}$", "aa", false),
            ("^\\u{41}$", "A", false),
            ("^\\p{L}$", "p{L}", true),
            ("^\\p{L}$", "a", false),
            ("^\\x{41}$", "A", false),
            ("^\\cJ$", "\n", true),
            ("^\\0$", "\0", true),
            ("^\\8$", "8", true),
            ("^[^]$", "a", true),
            ("^[]a$", "a", false),
            ("^[]a]$", "a", false),
            ("^[a-\\d]$", "-", true),
            ("^{$", "{", true),
            ("^x{1,2$", "x{1,2", true),
            ("^(a)\\1$", "aa", true),
            ("^(?<n>ku)_\\k<n>$", "ku_ku", true),
            ("^\\u0041$", "A", true),
            ("^\\x41$", "A", true),
            ("^a&&b$", "a&&b", true),
            // Go-verified (regexp2 v2.13.1, ECMAScript).
            ("\\<kube_pod", "kube_pod_info", false),
            ("^\\<$", "<", true),
            ("^\\>$", ">", true),
            ("^\\074$", "<", true),
            ("^(a)?b\\1$", "b", true),
            ("^\\1(a)$", "a", true),
            ("^(a)?b\\1$", "aba", true),
            ("^(a)?b\\1$", "ab", false),
            ("^\\k<n>$", "k<n>", true),
            ("^(?<n>a)?b\\k<n>$", "b", true),
            ("^[\\c1]$", "\u{11}", true),
            ("^[\\c1]$", "c", false),
            ("^[\\c_]$", "\u{1f}", true),
        ];
        for (p, input, want) in cases {
            assert_eq!(m(p, input), Ok(*want), "{p} on {input:?}");
        }
    }

    #[test]
    fn rejected_like_regexp2() {
        for p in [
            "^(?P<n>ku)$",
            "^(?<a>x)(?<a>y)$",
            "^a++$",
            "^a*+$",
            "[abc",
            "^(?<a>x)\\k<b>$",
        ] {
            assert!(
                ecma_to_fancy(p).is_err()
                    || fancy_regex::Regex::new(&ecma_to_fancy(p).unwrap_or_default()).is_err(),
                "{p}"
            );
        }
    }
}
