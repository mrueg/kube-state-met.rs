//! Port of the parts of `k8s.io/apimachinery/pkg/fields` used to build field
//! selectors: `ParseSelector`, `AndSelectors`, `OneTerm(Not)EqualSelector`,
//! `EscapeValue`/`UnescapeValue`, `Empty()` and `String()`.

/// A parsed field selector (`fields.Selector`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Selector {
    /// `field=value` (`hasTerm`).
    Has(String, String),
    /// `field!=value` (`notHasTerm`).
    NotHas(String, String),
    /// A conjunction (`andTerm`); empty means everything.
    And(Vec<Selector>),
}

impl Selector {
    /// `Selector.Empty()`.
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Selector::Has(..) | Selector::NotHas(..) => false,
            Selector::And(terms) => terms.iter().all(Selector::is_empty),
        }
    }

    /// `Selector.Transform` with the identity function, as `ParseSelector` uses.
    fn transform(self) -> Selector {
        match self {
            Selector::Has(f, v) | Selector::NotHas(f, v) if f.is_empty() && v.is_empty() => {
                Selector::And(Vec::new())
            }
            Selector::And(terms) => Selector::And(
                terms
                    .into_iter()
                    .map(Selector::transform)
                    .filter(|t| !t.is_empty())
                    .collect(),
            ),
            other => other,
        }
    }
}

impl std::fmt::Display for Selector {
    /// `Selector.String()`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Selector::Has(field, value) => write!(f, "{field}={}", escape_value(value)),
            Selector::NotHas(field, value) => write!(f, "{field}!={}", escape_value(value)),
            Selector::And(terms) => {
                for (i, t) in terms.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{t}")?;
                }
                Ok(())
            }
        }
    }
}

/// `fields.EscapeValue`.
pub(crate) fn escape_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | ',' | '=') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `fields.UnescapeValue`.
fn unescape_value(s: &str) -> Result<String, String> {
    if !s.contains(['\\', ',', '=']) {
        return Ok(s.to_owned());
    }
    let mut out = String::with_capacity(s.len());
    let mut in_slash = false;
    for c in s.chars() {
        if in_slash {
            match c {
                '\\' | ',' | '=' => out.push(c),
                _ => {
                    return Err(format!(
                        "invalid field selector: invalid escape sequence: \\{c}"
                    ));
                }
            }
            in_slash = false;
            continue;
        }
        match c {
            '\\' => in_slash = true,
            ',' | '=' => {
                return Err(format!(
                    "invalid field selector: unescaped character in value: {}",
                    c as u32
                ));
            }
            _ => out.push(c),
        }
    }
    if in_slash {
        return Err("invalid field selector: invalid escape sequence: \\".to_owned());
    }
    Ok(out)
}

/// `splitTerms`: splits on unescaped commas.
fn split_terms(selector: &str) -> Vec<&str> {
    if selector.is_empty() {
        return Vec::new();
    }
    let mut terms = Vec::new();
    let mut start = 0;
    let mut in_slash = false;
    for (i, c) in selector.char_indices() {
        if in_slash {
            in_slash = false;
        } else if c == '\\' {
            in_slash = true;
        } else if c == ',' {
            terms.push(&selector[start..i]);
            start = i + 1;
        }
    }
    terms.push(&selector[start..]);
    terms
}

/// `splitTerm`: finds the first operator (`!=`, `==`, `=`).
fn split_term(term: &str) -> Option<(&str, &str, &str)> {
    for (i, _) in term.char_indices() {
        let remaining = &term[i..];
        for op in ["!=", "==", "="] {
            if remaining.starts_with(op) {
                return Some((&term[..i], op, &term[i + op.len()..]));
            }
        }
    }
    None
}

/// `fields.ParseSelector`.
pub(crate) fn parse_selector(selector: &str) -> Result<Selector, String> {
    let mut parts = split_terms(selector);
    parts.sort_unstable();
    let mut items = Vec::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let Some((lhs, op, rhs)) = split_term(part) else {
            return Err(format!(
                "invalid selector: '{selector}'; can't understand '{part}'"
            ));
        };
        let value = unescape_value(rhs)?;
        items.push(if op == "!=" {
            Selector::NotHas(lhs.to_owned(), value)
        } else {
            Selector::Has(lhs.to_owned(), value)
        });
    }
    if items.len() == 1
        && let Some(item) = items.pop()
    {
        return Ok(item.transform());
    }
    Ok(Selector::And(items).transform())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_print() {
        let s = parse_selector("b=2,a!=1,c==x\\,y").unwrap();
        assert_eq!(s.to_string(), "a!=1,b=2,c=x\\,y");
        assert!(parse_selector("").unwrap().is_empty());
        assert!(parse_selector("=").unwrap().is_empty());
        assert_eq!(
            parse_selector("a").unwrap_err(),
            "invalid selector: 'a'; can't understand 'a'"
        );
        assert_eq!(
            parse_selector("a=b=c").unwrap_err(),
            "invalid field selector: unescaped character in value: 61"
        );
        assert_eq!(
            parse_selector("a=\\x").unwrap_err(),
            "invalid field selector: invalid escape sequence: \\x"
        );
    }
}
