//! Shared `key=value,...` specs for the trainable parameter structs
//! ([`crate::eval::EvalParams`], [`crate::search::SearchParams`]).
//!
//! UCI `setoption`, the CLI flags and the training tools all use the same
//! syntax, so the parser and the generated accessors live here instead of
//! being copied into each module. Keys are listed exactly once: `KEYS`,
//! `set` and `get` are generated from the same field list, which keeps
//! serialisation and parsing from drifting apart.

/// Parse `key=value,key=value` (commas or semicolons) and call `apply` for
/// every pair. An empty spec is valid (and applies nothing). Unknown keys
/// and non-integer values are errors, so a typo in a training run cannot
/// silently fall back to the baseline.
pub(crate) fn parse_spec(
    spec: &str,
    mut apply: impl FnMut(&str, i32) -> Result<(), String>,
) -> Result<(), String> {
    for part in spec.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| format!("expected key=value, got '{part}'"))?;
        let v: i32 = v
            .trim()
            .parse()
            .map_err(|_| format!("bad integer '{}' for '{}'", v.trim(), k.trim()))?;
        apply(k.trim(), v)?;
    }
    Ok(())
}

/// Generate `KEYS`, `set` and `get` from one field list, so key names and
/// accessors cannot drift apart. `$what` names the parameter group in the
/// unknown-key error message.
macro_rules! param_accessors {
    ($what:literal; $($field:ident),* $(,)?) => {
        /// Accepted and serialised key names, in declaration order.
        pub const KEYS: &'static [&'static str] = &[$(stringify!($field)),*];

        /// Set one parameter by name.
        pub fn set(&mut self, key: &str, value: i32) -> Result<(), String> {
            match key {
                $(stringify!($field) => self.$field = value,)*
                other => {
                    return Err(format!(concat!("unknown ", $what, " parameter '{}'"), other))
                }
            }
            Ok(())
        }

        /// Current value of one parameter by name (0 for unknown keys).
        pub fn get(&self, key: &str) -> i32 {
            match key {
                $(stringify!($field) => self.$field,)*
                _ => 0,
            }
        }
    };
}

pub(crate) use param_accessors;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_spec_handles_separators_whitespace_and_empty_specs() {
        let mut seen = Vec::new();
        parse_spec("a=1, b=2; c=3", |k, v| {
            seen.push((k.to_string(), v));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, [("a".to_string(), 1), ("b".to_string(), 2), ("c".to_string(), 3)]);

        // An empty spec is the baseline: the callback must never run.
        parse_spec("", |_, _| panic!("empty spec called apply")).unwrap();

        assert!(parse_spec("no_equals", |_, _| Ok(())).is_err());
        let err = parse_spec("a=x", |_, _| Ok(())).unwrap_err();
        assert!(err.contains("bad integer"), "{err}");
    }
}
