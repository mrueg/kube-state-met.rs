//! A port of the `spf13/pflag` argument parser (`FlagSet.Parse` with
//! interspersed arguments) and the error texts it produces.

use super::goparse::quote;

/// A flag definition: its name, shorthand and `NoOptDefVal`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FlagSpec {
    pub(crate) name: &'static str,
    pub(crate) shorthand: Option<u8>,
    /// The value used when the flag is given without one (`NoOptDefVal`),
    /// set for boolean flags.
    pub(crate) no_opt_def_val: Option<&'static str>,
}

impl FlagSpec {
    pub(crate) const fn value(name: &'static str) -> Self {
        Self {
            name,
            shorthand: None,
            no_opt_def_val: None,
        }
    }

    pub(crate) const fn boolean(name: &'static str) -> Self {
        Self {
            name,
            shorthand: None,
            no_opt_def_val: Some("true"),
        }
    }

    pub(crate) const fn short(mut self, shorthand: u8) -> Self {
        self.shorthand = Some(shorthand);
        self
    }

    /// `InvalidValueError`'s flag name: `-v, --v` or `--port`.
    fn display_name(&self) -> String {
        match self.shorthand {
            Some(s) => format!("-{}, --{}", char::from(s), self.name),
            None => format!("--{}", self.name),
        }
    }
}

/// A set of flags (`pflag.FlagSet`).
pub(crate) struct FlagSet {
    pub(crate) specs: &'static [FlagSpec],
}

/// Go's `%q` for a rune made from one byte (`rune(name[0])`).
fn quote_rune(b: u8) -> String {
    let c = char::from(b);
    match c {
        '\'' => "'\\''".to_owned(),
        '\\' => "'\\\\'".to_owned(),
        '\u{07}' => "'\\a'".to_owned(),
        '\u{08}' => "'\\b'".to_owned(),
        '\u{0c}' => "'\\f'".to_owned(),
        '\n' => "'\\n'".to_owned(),
        '\r' => "'\\r'".to_owned(),
        '\t' => "'\\t'".to_owned(),
        '\u{0b}' => "'\\v'".to_owned(),
        _ => {
            let q = quote(&c.to_string());
            format!("'{}'", &q[1..q.len() - 1])
        }
    }
}

impl FlagSet {
    pub(crate) fn lookup(&self, name: &str) -> Option<&'static FlagSpec> {
        self.specs.iter().find(|f| f.name == name)
    }

    fn shorthand_lookup(&self, c: u8) -> Option<&'static FlagSpec> {
        self.specs.iter().find(|f| f.shorthand == Some(c))
    }

    /// `hasNoOptDefVal` in cobra.
    pub(crate) fn has_no_opt_def_val(&self, name: &str) -> bool {
        self.lookup(name)
            .is_some_and(|f| f.no_opt_def_val.is_some())
    }

    /// `shortHasNoOptDefVal` in cobra.
    pub(crate) fn short_has_no_opt_def_val(&self, name: &str) -> bool {
        name.as_bytes()
            .first()
            .and_then(|&c| self.shorthand_lookup(c))
            .is_some_and(|f| f.no_opt_def_val.is_some())
    }

    /// `FlagSet.Parse`: calls `set(flag, value)` for every flag, in order, and
    /// returns the positional arguments. `set` returns the cause of an
    /// invalid value, which is wrapped as pflag's `InvalidValueError`.
    pub(crate) fn parse(
        &self,
        args: &[String],
        set: &mut dyn FnMut(&FlagSpec, &str) -> Result<(), String>,
    ) -> Result<Vec<String>, String> {
        let mut positional = Vec::new();
        let mut rest: &[String] = args;
        while let Some((s, tail)) = rest.split_first() {
            rest = tail;
            let b = s.as_bytes();
            if b.is_empty() || b[0] != b'-' || b.len() == 1 {
                positional.push(s.clone());
                continue;
            }
            if b[1] == b'-' {
                if b.len() == 2 {
                    positional.extend(rest.iter().cloned());
                    break;
                }
                rest = self.parse_long(s, rest, set)?;
            } else {
                rest = self.parse_short(s, rest, set)?;
            }
        }
        Ok(positional)
    }

    fn apply(
        flag: &FlagSpec,
        value: &str,
        set: &mut dyn FnMut(&FlagSpec, &str) -> Result<(), String>,
    ) -> Result<(), String> {
        set(flag, value).map_err(|cause| {
            format!(
                "invalid argument {} for {} flag: {cause}",
                quote(value),
                quote(&flag.display_name())
            )
        })
    }

    fn parse_long<'a>(
        &self,
        s: &str,
        args: &'a [String],
        set: &mut dyn FnMut(&FlagSpec, &str) -> Result<(), String>,
    ) -> Result<&'a [String], String> {
        let name = &s[2..];
        if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
            return Err(format!("bad flag syntax: {s}"));
        }
        let (name, inline) = match name.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (name, None),
        };
        let Some(flag) = self.lookup(name) else {
            return Err(format!("unknown flag: --{name}"));
        };
        let mut rest = args;
        let value = if let Some(v) = inline {
            v.to_owned()
        } else if let Some(v) = flag.no_opt_def_val {
            v.to_owned()
        } else if let Some((v, tail)) = args.split_first() {
            rest = tail;
            v.clone()
        } else {
            return Err(format!("flag needs an argument: --{name}"));
        };
        Self::apply(flag, &value, set)?;
        Ok(rest)
    }

    fn parse_short<'a>(
        &self,
        s: &str,
        args: &'a [String],
        set: &mut dyn FnMut(&FlagSpec, &str) -> Result<(), String>,
    ) -> Result<&'a [String], String> {
        let mut shorthands = &s[1..];
        let mut out = args;
        // As in pflag, every shorthand in the group sees the original args.
        while !shorthands.is_empty() {
            let (next, next_args) = self.parse_single_short(shorthands, args, set)?;
            shorthands = next;
            out = next_args;
        }
        Ok(out)
    }

    fn parse_single_short<'s, 'a>(
        &self,
        shorthands: &'s str,
        args: &'a [String],
        set: &mut dyn FnMut(&FlagSpec, &str) -> Result<(), String>,
    ) -> Result<(&'s str, &'a [String]), String> {
        if shorthands.starts_with("test.") {
            return Ok(("", args));
        }
        let b = shorthands.as_bytes();
        let c = b[0];
        let Some(flag) = self.shorthand_lookup(c) else {
            return Err(format!(
                "unknown shorthand flag: {} in -{shorthands}",
                quote_rune(c)
            ));
        };
        // `c` is an ASCII shorthand here, so byte offset 1 is a char boundary.
        let mut out_shorts = &shorthands[1..];
        let mut out_args = args;
        let value = if b.len() > 2 && b[1] == b'=' {
            out_shorts = "";
            shorthands[2..].to_owned()
        } else if let Some(v) = flag.no_opt_def_val {
            v.to_owned()
        } else if b.len() > 1 {
            out_shorts = "";
            shorthands[1..].to_owned()
        } else if let Some((v, tail)) = args.split_first() {
            out_args = tail;
            v.clone()
        } else {
            return Err(format!(
                "flag needs an argument: {} in -{shorthands}",
                quote_rune(c)
            ));
        };
        Self::apply(flag, &value, set)?;
        Ok((out_shorts, out_args))
    }
}
