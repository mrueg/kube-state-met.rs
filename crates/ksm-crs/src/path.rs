//! `valuePath` of `registry_factory.go`: a compiled path into an
//! unstructured object.

use std::fmt;

use serde_json::Value;

use crate::value::{non_nil, to_float64};

#[derive(Clone, Debug)]
enum Op {
    /// `[key=value]`: the first list element whose `key` matches `value`.
    ListLookup {
        key: String,
        val: String,
        num: Option<f64>,
        boolean: Option<bool>,
    },
    /// A map key (or `key=value`), or a list index.
    Field,
}

#[derive(Clone, Debug)]
struct PathOp {
    part: String,
    op: Op,
}

/// A compiled path (`valuePath`).
#[derive(Clone, Debug, Default)]
pub(crate) struct ValuePath(Vec<PathOp>);

/// `compilePath`.
pub(crate) fn compile_path(path: &[String]) -> Result<ValuePath, String> {
    let mut out = Vec::with_capacity(path.len());
    for part in path {
        if part.len() >= 2 && part.starts_with('[') && part.ends_with(']') {
            let inner = &part[1..part.len() - 1];
            let Some((key, val)) = inner.split_once('=') else {
                return Err(format!("invalid list lookup: {part}"));
            };
            let val_value = Value::String(val.to_owned());
            out.push(PathOp {
                part: part.clone(),
                op: Op::ListLookup {
                    key: key.to_owned(),
                    val: val.to_owned(),
                    num: to_float64(Some(&val_value), false).ok(),
                    boolean: ksm_core::gostd::parse_bool(val).ok(),
                },
            });
        } else {
            out.push(PathOp {
                part: part.clone(),
                op: Op::Field,
            });
        }
    }
    Ok(ValuePath(out))
}

impl PathOp {
    fn apply<'a>(&self, m: &'a Value) -> Option<&'a Value> {
        match &self.op {
            Op::ListLookup {
                key,
                val,
                num,
                boolean,
            } => {
                let Value::Array(s) = m else {
                    return None;
                };
                for v in s {
                    let Value::Object(m) = v else {
                        continue;
                    };
                    let Some(candidate) = m.get(key) else {
                        continue;
                    };
                    if let Value::String(c) = candidate
                        && c == val
                    {
                        return Some(v);
                    }
                    if let Some(num) = num
                        && let Ok(i) = to_float64(Some(candidate), false)
                        && *num == i
                    {
                        return Some(v);
                    }
                    if let Some(b) = boolean
                        && let Value::Bool(c) = candidate
                        && c == b
                    {
                        return Some(v);
                    }
                }
                None
            }
            Op::Field => match m {
                Value::Object(mp) => {
                    let part = &self.part;
                    // strings.Split(part, "=") with exactly two elements.
                    let mut kv = part.split('=');
                    if let (Some(key), Some(val), None) = (kv.next(), kv.next(), kv.next())
                        && let Some(v) = mp.get(key)
                        && let Value::String(s) = v
                        && s == val
                    {
                        return Some(v);
                    }
                    mp.get(part.as_str())
                }
                Value::Array(s) => {
                    // strconv.Atoi
                    let mut i: i64 = self.part.parse().ok()?;
                    let len = i64::try_from(s.len()).ok()?;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return None;
                    }
                    s.get(usize::try_from(i).ok()?)
                }
                _ => None,
            },
        }
    }
}

impl ValuePath {
    /// `valuePath.Get`: `None` is Go's `nil` (missing or JSON `null`).
    pub(crate) fn get<'a>(&self, obj: Option<&'a Value>) -> Option<&'a Value> {
        let mut obj = non_nil(obj);
        for op in &self.0 {
            let o = obj?;
            obj = non_nil(op.apply(o));
        }
        obj
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The path followed by `other`.
    pub(crate) fn join(&self, other: &ValuePath) -> ValuePath {
        let mut v = self.0.clone();
        v.extend(other.0.iter().cloned());
        ValuePath(v)
    }
}

impl fmt::Display for ValuePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[")?;
        for (i, op) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            f.write_str(&op.part)?;
        }
        f.write_str("]")
    }
}
