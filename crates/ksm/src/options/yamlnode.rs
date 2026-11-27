//! A YAML node tree with both the resolved type and the source text of every
//! scalar, as `gopkg.in/yaml.v3` sees it, plus the decoding rules
//! `yaml.Unmarshal` applies to the Go types `Options` uses.
//!
//! serde does not hand a visitor the source text of a resolved scalar
//! (`0x10` arrives as `16`), so the document is read twice: once resolving
//! every value, and once more, following the shape found the first time,
//! asking for every scalar as a string, which yields its source text.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};

/// The resolved type of a scalar.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Scalar {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Node {
    Scalar(Scalar, String),
    Seq(Vec<Node>),
    Map(Vec<(Node, Node)>),
}

impl Node {
    /// yaml.v3's short tag of the node.
    fn tag(&self) -> &'static str {
        match self {
            Node::Scalar(Scalar::Null, _) => "!!null",
            Node::Scalar(Scalar::Bool(_), _) => "!!bool",
            Node::Scalar(Scalar::Int(_), _) => "!!int",
            Node::Scalar(Scalar::Float(_), _) => "!!float",
            Node::Scalar(Scalar::Str, _) => "!!str",
            Node::Seq(_) => "!!seq",
            Node::Map(_) => "!!map",
        }
    }

    pub(crate) fn is_null(&self) -> bool {
        matches!(self, Node::Scalar(Scalar::Null, _))
    }
}

struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any YAML value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Node, E> {
        Ok(Node::Scalar(Scalar::Bool(v), String::new()))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Node, E> {
        Ok(Node::Scalar(Scalar::Int(i128::from(v)), String::new()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Node, E> {
        Ok(Node::Scalar(Scalar::Int(i128::from(v)), String::new()))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Node, E> {
        Ok(Node::Scalar(Scalar::Float(v), String::new()))
    }

    fn visit_str<E>(self, v: &str) -> Result<Node, E> {
        Ok(Node::Scalar(Scalar::Str, v.to_owned()))
    }

    fn visit_unit<E>(self) -> Result<Node, E> {
        Ok(Node::Scalar(Scalar::Null, String::new()))
    }

    fn visit_none<E>(self) -> Result<Node, E> {
        Ok(Node::Scalar(Scalar::Null, String::new()))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(NodeSeed)? {
            items.push(item);
        }
        Ok(Node::Seq(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut entries = Vec::new();
        while let Some(key) = map.next_key_seed(NodeSeed)? {
            let value = map.next_value_seed(NodeSeed)?;
            entries.push((key, value));
        }
        Ok(Node::Map(entries))
    }
}

struct NodeSeed;

impl<'de> DeserializeSeed<'de> for NodeSeed {
    type Value = Node;

    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Node, D::Error> {
        d.deserialize_any(NodeVisitor)
    }
}

/// Second pass: fills in the source text of the scalars of a node tree.
struct RawSeed<'n>(&'n mut Node);

struct RawStr;

impl Visitor<'_> for RawStr {
    type Value = String;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a scalar")
    }

    fn visit_str<E>(self, v: &str) -> Result<String, E> {
        Ok(v.to_owned())
    }
}

struct RawSeqVisitor<'n>(&'n mut [Node]);

impl<'de> Visitor<'de> for RawSeqVisitor<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a sequence")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        for item in self.0.iter_mut() {
            if seq.next_element_seed(RawSeed(item))?.is_none() {
                break;
            }
        }
        while seq.next_element::<de::IgnoredAny>()?.is_some() {}
        Ok(())
    }
}

struct RawMapVisitor<'n>(&'n mut [(Node, Node)]);

impl<'de> Visitor<'de> for RawMapVisitor<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a mapping")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        for (key, value) in self.0.iter_mut() {
            if map.next_key_seed(RawSeed(key))?.is_none() {
                return Ok(());
            }
            map.next_value_seed(RawSeed(value))?;
        }
        while map
            .next_entry::<de::IgnoredAny, de::IgnoredAny>()?
            .is_some()
        {}
        Ok(())
    }
}

impl<'de> DeserializeSeed<'de> for RawSeed<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        match self.0 {
            Node::Scalar(Scalar::Null, _) => d.deserialize_ignored_any(de::IgnoredAny).map(|_| ()),
            Node::Scalar(_, raw) => {
                *raw = d.deserialize_str(RawStr)?;
                Ok(())
            }
            Node::Seq(items) => d.deserialize_seq(RawSeqVisitor(items)),
            Node::Map(entries) => d.deserialize_map(RawMapVisitor(entries)),
        }
    }
}

/// Parses the first document of `yaml` (as `yaml.Unmarshal` does). `None`
/// means an empty document.
pub(crate) fn parse_first_document(yaml: &str) -> Result<Option<Node>, String> {
    let Some(doc) = serde_yaml_ng::Deserializer::from_str(yaml).next() else {
        return Ok(None);
    };
    let mut node = NodeSeed
        .deserialize(doc)
        .map_err(|e| format!("yaml: {e}"))?;
    if let Some(doc) = serde_yaml_ng::Deserializer::from_str(yaml).next() {
        RawSeed(&mut node)
            .deserialize(doc)
            .map_err(|e| format!("yaml: {e}"))?;
    }
    Ok(Some(node))
}

/// Outcome of decoding one node into a Go value.
pub(crate) enum Decoded<T> {
    /// The value to store.
    Set(T),
    /// The node was `null` and the Go type keeps its value.
    Keep,
    /// A type error was recorded.
    Failed,
}

impl<T> Decoded<T> {
    pub(crate) fn map<U>(self, f: impl FnOnce(T) -> U) -> Decoded<U> {
        match self {
            Decoded::Set(v) => Decoded::Set(f(v)),
            Decoded::Keep => Decoded::Keep,
            Decoded::Failed => Decoded::Failed,
        }
    }
}

/// yaml.v3's decoder state: the type errors collected so far.
#[derive(Default)]
pub(crate) struct Decoder {
    pub(crate) errors: Vec<String>,
}

impl Decoder {
    /// `decoder.terror`.
    pub(crate) fn type_error(&mut self, node: &Node, go_type: &str) {
        let value = match node {
            Node::Seq(_) | Node::Map(_) => String::new(),
            Node::Scalar(_, raw) => {
                if raw.len() > 10 {
                    let cut = (0..=7)
                        .rev()
                        .find(|&i| raw.is_char_boundary(i))
                        .unwrap_or(0);
                    format!(" `{}...`", &raw[..cut])
                } else {
                    format!(" `{raw}`")
                }
            }
        };
        self.errors.push(format!(
            "cannot unmarshal {}{value} into {go_type}",
            node.tag()
        ));
    }

    /// yaml.v3's duplicate key check for a mapping. Returns false when a key
    /// repeats; yaml.v3 then decodes nothing of the mapping.
    pub(crate) fn check_unique_keys(&mut self, entries: &[(Node, Node)]) -> bool {
        let before = self.errors.len();
        for (i, (ki, _)) in entries.iter().enumerate() {
            for (kj, _) in &entries[i + 1..] {
                if let (Node::Scalar(_, a), Node::Scalar(_, b)) = (ki, kj)
                    && a == b
                {
                    self.errors.push(format!(
                        "mapping key {} already defined",
                        super::goparse::quote(a)
                    ));
                }
            }
        }
        self.errors.len() == before
    }

    /// The final error, as `yaml.TypeError.Error()` formats it.
    pub(crate) fn into_result(self) -> Result<(), String> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "yaml: unmarshal errors:\n  {}",
                self.errors.join("\n  ")
            ))
        }
    }

    /// Decodes into a Go `string` (or a string type such as `NodeType`).
    pub(crate) fn string(&mut self, node: &Node, go_type: &str) -> Decoded<String> {
        match node {
            Node::Scalar(Scalar::Null, _) => Decoded::Keep,
            Node::Scalar(_, raw) => Decoded::Set(raw.clone()),
            _ => {
                self.type_error(node, go_type);
                Decoded::Failed
            }
        }
    }

    /// Decodes into a Go `bool`, including yaml.v3's YAML 1.1 spellings.
    pub(crate) fn boolean(&mut self, node: &Node) -> Decoded<bool> {
        match node {
            Node::Scalar(Scalar::Null, _) => Decoded::Keep,
            Node::Scalar(Scalar::Bool(b), _) => Decoded::Set(*b),
            Node::Scalar(Scalar::Str, raw) => match raw.as_str() {
                "y" | "Y" | "yes" | "Yes" | "YES" | "on" | "On" | "ON" => Decoded::Set(true),
                "n" | "N" | "no" | "No" | "NO" | "off" | "Off" | "OFF" => Decoded::Set(false),
                _ => {
                    self.type_error(node, "bool");
                    Decoded::Failed
                }
            },
            _ => {
                self.type_error(node, "bool");
                Decoded::Failed
            }
        }
    }

    /// Decodes into a Go signed integer of `bits` bits.
    pub(crate) fn int(&mut self, node: &Node, bits: u32, go_type: &str) -> Decoded<i64> {
        let min = -(1i128 << (bits - 1));
        let max = (1i128 << (bits - 1)) - 1;
        match node {
            Node::Scalar(Scalar::Null, _) => return Decoded::Keep,
            Node::Scalar(Scalar::Int(v), _) if (min..=max).contains(v) => {
                if let Ok(v) = i64::try_from(*v) {
                    return Decoded::Set(v);
                }
            }
            Node::Scalar(Scalar::Float(f), _) if *f <= i64::MAX as f64 => {
                // Go converts with int64(f), which truncates.
                let v = i128::from(*f as i64);
                if (min..=max).contains(&v)
                    && let Ok(v) = i64::try_from(v)
                {
                    return Decoded::Set(v);
                }
            }
            _ => {}
        }
        self.type_error(node, go_type);
        Decoded::Failed
    }

    /// Decodes into a Go `float64`.
    pub(crate) fn float(&mut self, node: &Node) -> Decoded<f64> {
        match node {
            Node::Scalar(Scalar::Null, _) => Decoded::Keep,
            Node::Scalar(Scalar::Int(v), _) => Decoded::Set(*v as f64),
            Node::Scalar(Scalar::Float(f), _) => Decoded::Set(*f),
            _ => {
                self.type_error(node, "float64");
                Decoded::Failed
            }
        }
    }

    /// Decodes into a Go `time.Duration`: only strings, via `time.ParseDuration`.
    pub(crate) fn duration(&mut self, node: &Node) -> Decoded<i64> {
        match node {
            Node::Scalar(Scalar::Null, _) => Decoded::Keep,
            Node::Scalar(Scalar::Str, raw) => match super::goparse::parse_duration(raw) {
                Ok(d) => Decoded::Set(d),
                Err(_) => {
                    self.type_error(node, "time.Duration");
                    Decoded::Failed
                }
            },
            _ => {
                self.type_error(node, "time.Duration");
                Decoded::Failed
            }
        }
    }

    /// Decodes a `map[string]struct{}` into an existing set (keys are added).
    /// `null` empties the set.
    pub(crate) fn set_into(
        &mut self,
        node: &Node,
        go_type: &str,
        set: &mut std::collections::BTreeSet<String>,
    ) {
        match node {
            Node::Scalar(Scalar::Null, _) => set.clear(),
            Node::Map(entries) => {
                if !self.check_unique_keys(entries) {
                    return;
                }
                for (key, value) in entries {
                    let key = match self.string(key, "string") {
                        Decoded::Set(k) => k,
                        Decoded::Keep | Decoded::Failed => continue,
                    };
                    match value {
                        Node::Scalar(Scalar::Null, _) => {
                            set.insert(key);
                        }
                        Node::Map(inner) => {
                            if self.check_unique_keys(inner) {
                                set.insert(key);
                            }
                        }
                        _ => self.type_error(value, "struct {}"),
                    }
                }
            }
            _ => self.type_error(node, go_type),
        }
    }

    /// Decodes a `[]string`; `null` items are skipped, as yaml.v3 does.
    pub(crate) fn string_list(&mut self, node: &Node, go_type: &str) -> Decoded<Vec<String>> {
        match node {
            Node::Scalar(Scalar::Null, _) => Decoded::Set(Vec::new()),
            Node::Seq(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    if let Decoded::Set(s) = self.string(item, "string") {
                        out.push(s);
                    }
                }
                Decoded::Set(out)
            }
            _ => {
                self.type_error(node, go_type);
                Decoded::Failed
            }
        }
    }

    /// Decodes a `map[string][]string` into a new map.
    pub(crate) fn string_list_map(
        &mut self,
        node: &Node,
    ) -> Decoded<BTreeMap<String, Vec<String>>> {
        const GO_TYPE: &str = "map[string][]string";
        match node {
            Node::Scalar(Scalar::Null, _) => Decoded::Set(BTreeMap::new()),
            Node::Map(entries) => {
                if !self.check_unique_keys(entries) {
                    return Decoded::Failed;
                }
                let mut out = BTreeMap::new();
                for (key, value) in entries {
                    let key = match self.string(key, "string") {
                        Decoded::Set(k) => k,
                        Decoded::Keep | Decoded::Failed => continue,
                    };
                    if value.is_null() {
                        out.insert(key, Vec::new());
                    } else if let Decoded::Set(list) = self.string_list(value, "[]string") {
                        out.insert(key, list);
                    }
                }
                Decoded::Set(out)
            }
            _ => {
                self.type_error(node, GO_TYPE);
                Decoded::Failed
            }
        }
    }
}
