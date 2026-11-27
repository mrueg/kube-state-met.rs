//! Translates a Go `regexp` (RE2) pattern, as the opt-in list reads it, into
//! an equivalent pattern for the `regex` crate (docs/port/research/regex.md
//! §4). The two dialects mostly agree; this handles where they don't:
//! `\Q..\E` quoting, a `{` that is no repetition (a literal in Go), octal
//! escapes, escaped punctuation (`\<` is a literal in Go, a word boundary in
//! `regex`), class set operators and nested `[` (literals in Go), the ASCII
//! Perl classes, and the syntax Go rejects but `regex` accepts (nested
//! repetitions, repeat counts over 1000, unknown escapes and flags).

/// Go's `syntax.ErrInvalidRepeatSize` limit.
const MAX_REPEAT: u32 = 1000;

fn err(what: &str, s: &[char]) -> String {
    format!("{what}: `{}`", s.iter().collect::<String>())
}

/// A literal character for `regex`.
fn lit(out: &mut String, c: char) {
    let mut b = [0u8; 4];
    out.push_str(&regex::escape(c.encode_utf8(&mut b)));
}

/// `{n}`, `{n,}` or `{n,m}` at `p[i]` (Go's `parseRepeat`): its length, or
/// `None` when it is no repetition and `{` is a literal.
fn repeat_len(p: &[char], i: usize) -> Option<(usize, Vec<u32>)> {
    let mut j = i + 1;
    let mut nums = Vec::new();
    loop {
        let start = j;
        while p.get(j).is_some_and(char::is_ascii_digit) {
            j += 1;
        }
        if j == start {
            // `{n,}` has no second number.
            if nums.len() == 1 && p.get(j) == Some(&'}') && p.get(j - 1) == Some(&',') {
                return Some((j + 1 - i, nums));
            }
            return None;
        }
        let digits: String = p[start..j].iter().collect();
        nums.push(digits.parse().unwrap_or(u32::MAX));
        match p.get(j) {
            Some('}') => return Some((j + 1 - i, nums)),
            Some(',') if nums.len() == 1 => j += 1,
            _ => return None,
        }
    }
}

/// The ASCII Perl classes as Go defines them (`perl_groups.go`), without
/// brackets.
fn perl_class(c: char) -> &'static str {
    match c {
        'd' => "0-9",
        'w' => "0-9A-Za-z_",
        _ => "\\t\\n\\x0C\\r ",
    }
}

/// Translates the escape at `p[i]` (`p[i] == '\\'`); returns chars consumed.
fn escape(p: &[char], i: usize, in_class: bool, out: &mut String) -> Result<usize, String> {
    let Some(&c) = p.get(i + 1) else {
        return Err(err("trailing backslash at end of expression", &[]));
    };
    match c {
        'd' | 'w' | 's' | 'D' | 'W' | 'S' => {
            let class = perl_class(c.to_ascii_lowercase());
            // A negated class inside a class becomes a nested negated set.
            if c.is_ascii_uppercase() {
                out.push_str(&format!("[^{class}]"));
            } else if in_class {
                out.push_str(class);
            } else {
                out.push_str(&format!("[{class}]"));
            }
            Ok(2)
        }
        'b' | 'B' if !in_class => {
            out.push_str(&format!("(?-u:\\{c})"));
            Ok(2)
        }
        'A' | 'z' if !in_class => {
            out.push('\\');
            out.push(c);
            Ok(2)
        }
        'a' | 'f' | 'n' | 'r' | 't' | 'v' => {
            out.push('\\');
            out.push(c);
            Ok(2)
        }
        'x' | 'p' | 'P' => {
            // `\x41`, `\x{41}`, `\pL`, `\p{Greek}`: the same in both.
            let mut j = i + 2;
            if p.get(j) == Some(&'{') {
                while j < p.len() && p[j] != '}' {
                    j += 1;
                }
                j += 1;
            } else {
                j += if c == 'x' { 2 } else { 1 };
            }
            let j = j.min(p.len());
            out.extend(&p[i..j]);
            Ok(j - i)
        }
        '0'..='7' => {
            // `\1`-`\7` alone would be a backreference, which Go rejects.
            if c != '0' && !p.get(i + 2).is_some_and(|d| d.is_ascii_digit()) {
                return Err(err("invalid escape sequence", &p[i..i + 2]));
            }
            let digits: Vec<char> = p[i + 1..]
                .iter()
                .take(3)
                .take_while(|d| ('0'..='7').contains(*d))
                .copied()
                .collect();
            let v = digits
                .iter()
                .fold(0u32, |v, d| v * 8 + d.to_digit(8).unwrap_or(0));
            out.push_str(&format!("\\x{{{v:X}}}"));
            Ok(1 + digits.len())
        }
        // parseEscape: any other ASCII character but a letter or digit is
        // itself (punctuation, but also space and control characters).
        c if c.is_ascii() && !c.is_ascii_alphanumeric() => {
            lit(out, c);
            Ok(2)
        }
        _ => Err(err("invalid escape sequence", &p[i..i + 2])),
    }
}

/// Translates a class at `p[i] == '['`; returns chars consumed. Ranges
/// follow Go's `parseClass`: an atom followed by `-` and anything but `]`
/// starts a range, every other `-` is a literal (in `regex`, `--` would be
/// set difference), and so are `[`, `&&` and `~~`.
fn class(p: &[char], i: usize, out: &mut String) -> Result<usize, String> {
    let mut j = i + 1;
    out.push('[');
    if p.get(j) == Some(&'^') {
        out.push('^');
        j += 1;
    }
    let first = j;
    // The next atom ends a range: it can't start one.
    let mut range_end = false;
    while j < p.len() {
        let c = p[j];
        let starts_range = |k: usize| {
            !range_end && p.get(k) == Some(&'-') && p.get(k + 1).is_some_and(|n| *n != ']')
        };
        if c == ']' && j > first {
            out.push(']');
            return Ok(j + 1 - i);
        }
        if c == '[' && p.get(j + 1) == Some(&':') {
            // `[:alpha:]`: the same in both; copy through `:]`.
            if let Some(k) =
                (j + 2..p.len().saturating_sub(1)).find(|k| p[*k] == ':' && p[*k + 1] == ']')
            {
                out.extend(&p[j..k + 2]);
                j = k + 2;
                range_end = false;
                continue;
            }
        }
        if c == '\\' {
            let n = escape(p, j, true, out)?;
            let single = !matches!(
                p.get(j + 1),
                Some('d' | 'w' | 's' | 'D' | 'W' | 'S' | 'p' | 'P')
            );
            j += n;
            if single && starts_range(j) {
                out.push('-');
                j += 1;
                range_end = true;
            } else {
                range_end = false;
            }
            continue;
        }
        if starts_range(j + 1)
            && p[j + 2] != '\\'
            && !(p[j + 2] == '[' && p.get(j + 3) == Some(&':'))
        {
            let hi = p[j + 2];
            if hi < c {
                return Err(err("invalid character class range", &p[j..j + 3]));
            }
            lit(out, c);
            out.push('-');
            lit(out, hi);
            j += 3;
            range_end = false;
            continue;
        }
        lit(out, c);
        j += 1;
        if starts_range(j) {
            out.push('-');
            j += 1;
            range_end = true;
        } else {
            range_end = false;
        }
    }
    Err(err("missing closing ]", &p[i..]))
}

/// Go accepts only the `i`, `m`, `s` and `U` flags, and named groups as
/// `(?P<name>` or `(?<name>`.
fn check_group(p: &[char], i: usize) -> Result<(), String> {
    if p.get(i + 1) != Some(&'?') {
        return Ok(());
    }
    match p.get(i + 2) {
        Some('P') if p.get(i + 3) == Some(&'<') => Ok(()),
        Some('<') => Ok(()),
        _ => {
            let flags: Vec<char> = p[i + 2..]
                .iter()
                .take_while(|c| !matches!(c, ':' | ')'))
                .copied()
                .collect();
            if flags
                .iter()
                .all(|f| matches!(f, 'i' | 'm' | 's' | 'U' | '-'))
            {
                Ok(())
            } else {
                Err(err(
                    "invalid or unsupported Perl syntax",
                    &p[i..(i + 3).min(p.len())],
                ))
            }
        }
    }
}

/// The `regex` pattern matching what Go's `regexp` matches.
pub fn re2_to_regex(pattern: &str) -> Result<String, String> {
    let p: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut i = 0;
    // Just after a repetition (and its `?`): another one is an error in Go.
    let mut repeated = false;
    while i < p.len() {
        let c = p[i];
        let was_repeated = std::mem::take(&mut repeated);
        match c {
            '\\' if p.get(i + 1) == Some(&'Q') => {
                let body = &p[i + 2..];
                let end =
                    (0..body.len()).find(|k| body[*k] == '\\' && body.get(k + 1) == Some(&'E'));
                let text = &body[..end.unwrap_or(body.len())];
                for ch in text {
                    lit(&mut out, *ch);
                }
                i += 2 + text.len() + if end.is_some() { 2 } else { 0 };
            }
            '\\' => i += escape(&p, i, false, &mut out)?,
            '[' => i += class(&p, i, &mut out)?,
            '(' => {
                check_group(&p, i)?;
                out.push('(');
                i += 1;
            }
            '*' | '+' | '?' => {
                if was_repeated {
                    return Err(err("invalid nested repetition operator", &p[i - 1..=i]));
                }
                out.push(c);
                i += 1;
                if p.get(i) == Some(&'?') {
                    out.push('?');
                    i += 1;
                }
                repeated = true;
            }
            '{' => match repeat_len(&p, i) {
                Some((n, nums)) => {
                    if nums.iter().any(|v| *v > MAX_REPEAT)
                        || (nums.len() == 2 && nums[0] > nums[1])
                    {
                        return Err(err("invalid repeat count", &p[i..i + n]));
                    }
                    if was_repeated {
                        return Err(err("invalid nested repetition operator", &p[i..i + n]));
                    }
                    out.extend(&p[i..i + n]);
                    i += n;
                    if p.get(i) == Some(&'?') {
                        out.push('?');
                        i += 1;
                    }
                    repeated = true;
                }
                None => {
                    out.push_str("\\{");
                    i += 1;
                }
            },
            '}' => {
                out.push_str("\\}");
                i += 1;
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

    fn m(pattern: &str, input: &str) -> Option<bool> {
        let t = re2_to_regex(pattern).ok()?;
        Some(regex::Regex::new(&t).ok()?.is_match(input))
    }

    /// Go-verified (`regexp.Compile` + `MatchString`, Go 1.26); `None`:
    /// Go's compile error.
    #[test]
    fn go_verified_vectors() {
        let cases: &[(&str, &str, Option<bool>)] = &[
            ("kube_pod_.*", "kube_pod_info", Some(true)),
            // parseEscape: escaped space and control characters are literals.
            ("^a\\ b$", "a b", Some(true)),
            ("^a\\\tb$", "a\tb", Some(true)),
            ("^[\\ ]$", " ", Some(true)),
            ("^a{,3}$", "a{,3}", Some(true)),
            ("^kube_{$", "kube_{", Some(true)),
            ("^x{1,2$", "x{1,2", Some(true)),
            ("^\\Qa.b\\E$", "a.b", Some(true)),
            ("^\\Qa.b\\E$", "axb", Some(false)),
            ("^\\Qa.b$", "a.b$", Some(true)),
            ("^[a&&b]$", "&", Some(true)),
            ("^[[a]]$", "[]", Some(true)),
            ("^[[a]]$", "a]", Some(true)),
            ("\\<kube", "kube_pod", Some(false)),
            ("^\\<kube$", "<kube", Some(true)),
            ("^a{1001}$", "a", None),
            ("^a{2,1}$", "a", None),
            ("^a**$", "a", None),
            ("^a++$", "a", None),
            ("^a*?$", "a", Some(true)),
            ("^a{2}{3}$", "a", None),
            ("^\\u0041$", "A", None),
            ("^\\0$", "\0", Some(true)),
            ("^\\01$", "\u{1}", Some(true)),
            ("^\\1$", "1", None),
            ("^(?x)a$", "a", None),
            ("^(?i)A$", "a", Some(true)),
            ("^(?P<n>a)$", "a", Some(true)),
            ("^(?<n>a)$", "a", Some(true)),
            ("^\\d$", "٣", Some(false)),
            ("^\\w$", "é", Some(false)),
            ("^\\s$", "\u{a0}", Some(false)),
            ("^\\s$", "\u{b}", Some(false)),
            ("^[\\d]$", "5", Some(true)),
            ("^[^\\d]$", "5", Some(false)),
            ("\\bpod\\b", "épodé", Some(true)),
            ("^\\pL$", "a", Some(true)),
            ("^\\p{Greek}$", "α", Some(true)),
            ("^\\x41\\x{42}$", "AB", Some(true)),
            ("^[[:alpha:]]$", "a", Some(true)),
            ("^[a-c]$", "b", Some(true)),
            ("^[a--]$", "-", None),
            ("^[--a]$", "-", Some(true)),
            ("^[\\x41-C]$", "B", Some(true)),
            ("^[\\d-z]$", "-", Some(true)),
            ("^[a-]$", "-", Some(true)),
            ("^[]a]$", "]", Some(true)),
            ("^[a~~b]$", "~", Some(true)),
            ("^}$", "}", Some(true)),
            ("^\\z$", "", Some(true)),
        ];
        for (p, input, want) in cases {
            assert_eq!(
                m(p, input),
                *want,
                "{p} on {input:?} ({:?})",
                re2_to_regex(p)
            );
        }
    }
}
