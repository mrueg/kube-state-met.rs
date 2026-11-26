//! Ports of `pkg/allowdenylist` (ECMAScript regexes, `regexp2` in Go) and
//! `pkg/optin` (RE2 regexes).

use std::collections::BTreeSet;

use crate::generator::FamilyFilter;

/// Errors building a filter; the text is what kube-state-metrics prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterError(pub String);

impl std::fmt::Display for FilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FilterError {}

/// `allowdenylist.AllowDenyList`: names matched (unanchored) by any regex of
/// the list are allowed (allow list) or denied (deny list).
pub struct AllowDenyList {
    list: BTreeSet<String>,
    regexes: Vec<fancy_regex::Regex>,
    is_allow_list: bool,
}

impl AllowDenyList {
    /// At most one of `allow` and `deny` may be non-empty.
    pub fn new(allow: &BTreeSet<String>, deny: &BTreeSet<String>) -> Result<Self, FilterError> {
        if !allow.is_empty() && !deny.is_empty() {
            return Err(FilterError(
                "allowlist and denylist are both set, they are mutually exclusive, only one of them can be set"
                    .into(),
            ));
        }
        let (list, is_allow_list) = if allow.is_empty() {
            (deny.clone(), false)
        } else {
            (allow.clone(), true)
        };
        Ok(AllowDenyList {
            list,
            regexes: Vec::new(),
            is_allow_list,
        })
    }

    /// Compiles every entry as an ECMAScript-flavoured regex.
    pub fn parse(&mut self) -> Result<(), FilterError> {
        self.regexes = self
            .list
            .iter()
            .map(|item| compile_ecmascript(item))
            .collect::<Result<_, _>>()?;
        Ok(())
    }

    pub fn include(&mut self, items: &[String]) {
        for i in items {
            if self.is_allow_list {
                self.list.insert(i.clone());
            } else {
                self.list.remove(i);
            }
        }
    }

    pub fn exclude(&mut self, items: &[String]) {
        for i in items {
            if self.is_allow_list {
                self.list.remove(i);
            } else {
                self.list.insert(i.clone());
            }
        }
    }

    pub fn is_included(&self, item: &str) -> bool {
        // A regex that fails at match time (backtrack limit) counts as excluded,
        // as a regexp2 timeout does in Go.
        let mut matched = false;
        for r in &self.regexes {
            match r.is_match(item) {
                Ok(true) => {
                    matched = true;
                    break;
                }
                Ok(false) => {}
                Err(_) => return false,
            }
        }
        if self.is_allow_list {
            matched
        } else {
            !matched
        }
    }

    pub fn is_excluded(&self, item: &str) -> bool {
        !self.is_included(item)
    }

    /// Text for the startup log line.
    pub fn status(&self) -> String {
        let items: Vec<&str> = self.list.iter().map(String::as_str).collect();
        if self.is_allow_list {
            format!(
                "Including the following lists that were on allowlist: {}",
                items.join(", ")
            )
        } else {
            format!(
                "Excluding the following lists that were on denylist: {}",
                items.join(", ")
            )
        }
    }
}

impl FamilyFilter for AllowDenyList {
    fn test(&self, name: &str, _opt_in: bool) -> bool {
        self.is_included(name)
    }
}

/// Compiles a pattern the way `regexp2` in ECMAScript mode reads it, as far
/// as `fancy-regex` allows (see docs/port/research/regex.md).
pub fn compile_ecmascript(pattern: &str) -> Result<fancy_regex::Regex, FilterError> {
    let translated = crate::ecma::ecma_to_fancy(pattern)
        .map_err(|e| FilterError(format!("error parsing regexp: {e} in `{pattern}`")))?;
    fancy_regex::RegexBuilder::new(&translated)
        .backtrack_limit(1_000_000)
        .build()
        .map_err(|e| FilterError(format!("error parsing regexp: {e}")))
}

/// `optin.MetricFamilyFilter`: opt-in families pass only when a regex of the
/// opt-in list matches their name (unanchored, RE2 semantics).
#[derive(Default)]
pub struct OptInFilter {
    metrics: Vec<regex::Regex>,
}

impl OptInFilter {
    pub fn new(metrics: &BTreeSet<String>) -> Result<Self, FilterError> {
        let metrics = metrics
            .iter()
            .map(|m| regex::Regex::new(m).map_err(|e| FilterError(go_regexp_error(m, &e))))
            .collect::<Result<_, _>>()?;
        Ok(OptInFilter { metrics })
    }

    pub fn count(&self) -> usize {
        self.metrics.len()
    }

    pub fn status(&self) -> String {
        let mut s: Vec<&str> = self.metrics.iter().map(regex::Regex::as_str).collect();
        s.sort_unstable();
        s.join(", ")
    }
}

impl FamilyFilter for OptInFilter {
    fn test(&self, name: &str, opt_in: bool) -> bool {
        !opt_in || self.metrics.iter().any(|r| r.is_match(name))
    }
}

/// Error text in the shape of Go's `regexp.Compile` errors.
fn go_regexp_error(pattern: &str, e: &regex::Error) -> String {
    let _ = e;
    format!("error parsing regexp: invalid pattern: `{pattern}`")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn allow_deny() {
        assert!(AllowDenyList::new(&set(&["a"]), &set(&["b"])).is_err());
        let mut l =
            AllowDenyList::new(&set(&["kube_pod_.*", "kube_node_info"]), &set(&[])).unwrap();
        l.parse().unwrap();
        assert!(l.is_included("kube_pod_info"));
        assert!(l.is_included("x_kube_node_info_y"), "unanchored");
        assert!(!l.is_included("kube_deployment_created"));
        let mut d = AllowDenyList::new(&set(&[]), &set(&["^kube_pod_(?!info)"])).unwrap();
        d.parse().unwrap();
        assert!(d.is_included("kube_pod_info"));
        assert!(!d.is_included("kube_pod_status_phase"));
        let mut empty = AllowDenyList::new(&set(&[]), &set(&[])).unwrap();
        empty.parse().unwrap();
        assert!(empty.is_included("anything"));
    }

    #[test]
    fn opt_in() {
        let f = OptInFilter::new(&set(&["kube_pod_tolerations"])).unwrap();
        assert!(f.test("kube_pod_info", false));
        assert!(f.test("kube_pod_tolerations", true));
        assert!(!f.test("kube_pod_nodeselectors", true));
        assert_eq!(f.count(), 1);
    }
}
