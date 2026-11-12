//! Go: `go/tools/schema.go`. ValidateArgs checks a tool call's arguments against the tool's JSON-Schema parameters and
//! returns a coerced copy. It enforces required, type, enum, minimum, maximum, minLength, and (recursively) array items and
//! nested object properties.
//!
//! Local models often send numbers and booleans as strings, so numeric strings are accepted for integer/number (as Go's
//! strconv.ParseFloat reads them) and "true"/"false" for boolean, and the copy holds the coerced value. A JSON boolean is
//! never accepted as a number. Unknown properties are passed through untouched.

use std::collections::BTreeMap;

use crate::goany::Any;
use crate::gotext::{float_v, parse_float, quote, to_lower, trim_space};

/// Go's `ValidateArgs`. `schema` None is Go's nil schema: no checks. Go checks an object's properties in map order, so when
/// two of them are wrong it names either; the port names the first in sorted order.
pub fn validate_args(schema: Option<&BTreeMap<String, Any>>, args: BTreeMap<String, Any>) -> Result<BTreeMap<String, Any>, String> {
    let Some(schema) = schema else { return Ok(args) };
    match validate_value(schema, Any::Object(args), "arguments")? {
        Any::Object(m) => Ok(m),
        _ => Ok(BTreeMap::new()),
    }
}

fn schema_map(v: Option<&Any>) -> Option<&BTreeMap<String, Any>> {
    match v {
        Some(Any::Object(m)) => Some(m),
        _ => None,
    }
}

fn string_list(v: Option<&Any>) -> Vec<&str> {
    match v {
        Some(Any::Array(xs)) => xs
            .iter()
            .filter_map(|x| match x {
                Any::String(s) => Some(s.as_str()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn number_of(v: Option<&Any>) -> Option<f64> {
    match v {
        Some(Any::Number(f)) => Some(*f),
        _ => None,
    }
}

fn qualify(where_: &str, name: &str) -> String {
    if where_ == "arguments" {
        name.to_owned()
    } else {
        format!("{where_}.{name}")
    }
}

/// Numbers and numeric strings, never booleans.
fn to_number(v: &Any) -> Option<f64> {
    match v {
        Any::Number(f) => f.is_finite().then_some(*f),
        Any::String(s) => parse_float(trim_space(s)).ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

fn validate_value(schema: &BTreeMap<String, Any>, v: Any, where_: &str) -> Result<Any, String> {
    let typ = match schema.get("type") {
        Some(Any::String(t)) => t.as_str(),
        _ => "",
    };
    match typ {
        "object" => {
            let Any::Object(mut out) = v else { return Err(format!("{where_} must be an object")) };
            for name in string_list(schema.get("required")) {
                if matches!(out.get(name), None | Some(Any::Null)) {
                    return Err(format!("{} is required", qualify(where_, name)));
                }
            }
            if let Some(props) = schema_map(schema.get("properties")) {
                for (name, ps) in props {
                    let Some(x) = out.remove(name) else { continue };
                    if x == Any::Null {
                        continue; // a null property is dropped
                    }
                    let Some(sub) = schema_map(Some(ps)) else {
                        out.insert(name.clone(), x);
                        continue;
                    };
                    let cv = validate_value(sub, x, &qualify(where_, name))?;
                    out.insert(name.clone(), cv);
                }
            }
            Ok(Any::Object(out))
        }
        "array" => {
            let Any::Array(list) = v else { return Err(format!("{where_} must be an array")) };
            let Some(items) = schema_map(schema.get("items")) else { return Ok(Any::Array(list)) };
            let mut out = Vec::with_capacity(list.len());
            for (i, x) in list.into_iter().enumerate() {
                out.push(validate_value(items, x, &format!("{where_}[{i}]"))?);
            }
            Ok(Any::Array(out))
        }
        "string" => {
            let Any::String(s) = v else { return Err(format!("{where_} must be a string")) };
            if let Some(n) = number_of(schema.get("minLength")) {
                if (s.chars().count() as f64) < n {
                    if n == 1.0 {
                        return Err(format!("{where_} must not be empty"));
                    }
                    return Err(format!("{where_} must be at least {} characters", float_v(n)));
                }
            }
            let allowed = string_list(schema.get("enum"));
            if !allowed.is_empty() && !allowed.contains(&s.as_str()) {
                return Err(format!("{where_} must be one of {}, got {}", allowed.join(", "), quote(&s)));
            }
            Ok(Any::String(s))
        }
        "integer" | "number" => {
            let Some(f) = to_number(&v) else { return Err(format!("{where_} must be a {typ}")) };
            if typ == "integer" && f != f.trunc() {
                return Err(format!("{where_} must be a whole number, got {}", float_v(f)));
            }
            if let Some(lo) = number_of(schema.get("minimum")) {
                if f < lo {
                    return Err(format!("{where_} must be at least {}, got {}", float_v(lo), float_v(f)));
                }
            }
            if let Some(hi) = number_of(schema.get("maximum")) {
                if f > hi {
                    return Err(format!("{where_} must be at most {}, got {}", float_v(hi), float_v(f)));
                }
            }
            Ok(Any::Number(f))
        }
        "boolean" => match v {
            Any::Bool(b) => Ok(Any::Bool(b)),
            Any::String(ref t) if to_lower(trim_space(t)) == "true" => Ok(Any::Bool(true)),
            Any::String(ref t) if to_lower(trim_space(t)) == "false" => Ok(Any::Bool(false)),
            _ => Err(format!("{where_} must be true or false")),
        },
        _ => Ok(v),
    }
}
