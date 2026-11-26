//! Port of `github.com/gobuffalo/flect` v1.0.3 `Pluralize`, the default
//! resource plural of a custom resource (`Resource.GetResourceName`).
//!
//! See docs/port/research/flect.md. Not ported: flect's `init()` loading of
//! `inflections.json` / `acronyms.json` from the working directory or
//! `$INFLECT_PATH` / `$ACRONYMS_PATH`; this port always uses the built-in
//! tables. Character classes follow Go's for ASCII; outside ASCII, Go's
//! `unicode.IsPunct` is approximated by "never punctuation".

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::flect_data::{ACRONYMS, DICTIONARY, SUFFIXES};
use crate::gostr;

/// What a plural rule does with the last part of a word.
enum RuleFn {
    /// `noop`: the word already ends with a plural.
    Noop,
    /// `simpleRuleFunc(suffix, repl)`.
    Replace {
        suffix_len: usize,
        repl: &'static str,
    },
}

struct Rule {
    suffix: &'static str,
    func: RuleFn,
}

struct Tables {
    single_to_plural: HashMap<&'static str, &'static str>,
    plural_to_single: HashSet<&'static str>,
    plural_rules: Vec<Rule>,
    acronyms: HashSet<&'static str>,
}

static TABLES: LazyLock<Tables> = LazyLock::new(build_tables);

fn build_tables() -> Tables {
    let mut single_to_plural = HashMap::new();
    let mut plural_to_single = HashSet::new();
    for wd in DICTIONARY {
        let plural = if wd.uncountable && wd.plural.is_empty() {
            wd.singular
        } else {
            wd.plural
        };
        single_to_plural.insert(wd.singular, plural);
        if !wd.unidirectional {
            plural_to_single.insert(plural);
            if !wd.alternative.is_empty() {
                plural_to_single.insert(wd.alternative);
            }
        }
    }

    // InsertPluralRule prepends [noop(plural), rule(singular -> plural)].
    // The suffix list is inserted back to front, then every non-exact
    // dictionary word front to back.
    let mut prepended: Vec<(&'static str, &'static str)> = Vec::new();
    for (singular, plural) in SUFFIXES.iter().rev() {
        prepended.push((singular, plural));
    }
    for wd in DICTIONARY {
        if wd.exact {
            continue;
        }
        let plural = if wd.uncountable && wd.plural.is_empty() {
            wd.singular
        } else {
            wd.plural
        };
        prepended.push((wd.singular, plural));
    }
    let mut plural_rules = Vec::with_capacity(prepended.len() * 2);
    for (singular, plural) in prepended.into_iter().rev() {
        plural_rules.push(Rule {
            suffix: plural,
            func: RuleFn::Noop,
        });
        plural_rules.push(Rule {
            suffix: singular,
            func: RuleFn::Replace {
                suffix_len: singular.len(),
                repl: plural,
            },
        });
    }

    Tables {
        single_to_plural,
        plural_to_single,
        plural_rules,
        acronyms: ACRONYMS.iter().copied().collect(),
    }
}

fn is_acronym(s: &str) -> bool {
    TABLES.acronyms.contains(gostr::to_upper(s).as_str())
}

/// `spaces` of flect.go.
const SPACES: [char; 5] = ['_', ' ', ':', '-', '/'];

fn is_space(c: char) -> bool {
    SPACES.contains(&c) || gostr::is_space(c)
}

/// Go's `unicode.IsPunct` for ASCII (categories P*, not the symbols
/// `$ + < = > ^ ` | ~`).
fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() && !matches!(c, '$' | '+' | '<' | '=' | '>' | '^' | '`' | '|' | '~')
}

fn is_upper(c: Option<char>) -> bool {
    c.is_some_and(char::is_uppercase)
}

fn xappend(parts: &mut Vec<String>, s: &str) {
    let mut s = s.trim_matches(gostr::is_space);
    for x in SPACES {
        s = s.trim_matches(x);
    }
    let s = if is_acronym(s) {
        gostr::to_upper(s)
    } else {
        s.to_owned()
    };
    if !s.is_empty() {
        parts.push(s);
    }
}

/// `toParts` (ident.go).
fn to_parts(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let s = s.trim_matches(gostr::is_space);
    if s.is_empty() {
        return parts;
    }
    if is_acronym(s) {
        return vec![gostr::to_upper(s)];
    }
    let mut prev: Option<char> = None;
    let mut x = String::with_capacity(s.len());
    for c in s.chars() {
        if is_space(c) {
            xappend(&mut parts, &x);
            x.clear();
            x.push(c);
            prev = Some(c);
            continue;
        }
        if c.is_uppercase() && !is_upper(prev) {
            xappend(&mut parts, &x);
            x.clear();
            x.push(c);
            prev = Some(c);
            continue;
        }
        if c.is_uppercase() && is_acronym(&x) {
            xappend(&mut parts, &x);
            x.clear();
            x.push(c);
            prev = Some(c);
            continue;
        }
        if c.is_alphabetic() || c.is_numeric() || is_punct(c) || c == '`' {
            prev = Some(c);
            x.push(c);
            continue;
        }
        xappend(&mut parts, &x);
        x.clear();
        prev = Some(c);
    }
    xappend(&mut parts, &x);
    parts
}

fn last_part(s: &str) -> Option<String> {
    to_parts(s).pop()
}

/// `flect.Capitalize`.
fn capitalize(s: &str) -> String {
    if to_parts(s).is_empty() {
        return String::new();
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => {
            let mut out = String::with_capacity(s.len());
            out.push(gostr::to_title_rune(first));
            out.push_str(chars.as_str());
            out
        }
        None => String::new(),
    }
}

/// `Ident.ReplaceSuffix(orig, new)`: `TrimSuffix(Original, orig) + new`.
fn replace_suffix(original: &str, orig: &str, new: &str) -> String {
    let mut out = original.strip_suffix(orig).unwrap_or(original).to_owned();
    out.push_str(new);
    out
}

/// `flect.Pluralize`.
pub fn pluralize(original: &str) -> String {
    let Some(s) = last_part(original) else {
        return String::new();
    };
    let t = &*TABLES;
    if let Some(p) = t.single_to_plural.get(original) {
        return replace_suffix(original, original, p);
    }
    if t.plural_to_single.contains(original) {
        return original.to_owned();
    }
    let ls = gostr::to_lower(&s);
    if t.plural_to_single.contains(ls.as_str()) {
        return original.to_owned();
    }
    if let Some(p) = t.single_to_plural.get(ls.as_str()) {
        let p = if s == capitalize(&s) {
            capitalize(p)
        } else {
            (*p).to_owned()
        };
        return replace_suffix(original, &s, &p);
    }
    for r in &t.plural_rules {
        if s.ends_with(r.suffix) {
            let replaced = match r.func {
                RuleFn::Noop => s.clone(),
                RuleFn::Replace { suffix_len, repl } => {
                    let mut out = s[..s.len() - suffix_len].to_owned();
                    out.push_str(repl);
                    out
                }
            };
            return replace_suffix(original, &s, &replaced);
        }
    }
    if ls.ends_with('s') {
        return original.to_owned();
    }
    format!("{original}s")
}

#[cfg(test)]
mod tests {
    use super::pluralize;

    #[test]
    fn research_vectors() {
        for (k, want) in [
            ("Foo", "Foos"),
            ("Index", "Indices"),
            ("Video", "Videoes"),
            ("Safe", "Saves"),
            ("Price", "Price"),
            ("Octopus", "Octupuses"),
            ("DNS", "DNSes"),
            ("ID", "IDs"),
            ("POLICY", "POLICYs"),
            ("HTTPRoute", "HTTPRoutes"),
            ("Postman", "Postmen"),
            ("Human", "Humans"),
            ("Database", "Databases"),
            ("", ""),
        ] {
            assert_eq!(pluralize(k), want, "{k}");
        }
    }
}
