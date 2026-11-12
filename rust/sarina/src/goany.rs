//! What Go's `encoding/json` puts in an `any`: nil, bool, float64, string, `[]any`, `map[string]any`. Tool arguments and
//! schemas travel in this form.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Any {
    Null,
    Bool(bool),
    Number(f64),
    /// an `int` that Go code put in a map (a message's revision count): never decoded, written as a decimal
    Int(i64),
    /// a `json.Number`: the number's own text, from a decoder with UseNumber (the Chymaera ports read JSON that way)
    Num(String),
    String(String),
    Array(Vec<Any>),
    /// Go's map: one value per key, read in sorted order where Go's order would be random
    Object(BTreeMap<String, Any>),
}

impl Drop for Any {
    /// Without recursion: a value decoded from a document 10,000 levels deep (Go's limit) would need megabytes of stack to
    /// drop part by part.
    fn drop(&mut self) {
        let mut parts = match self {
            Any::Array(items) => std::mem::take(items),
            Any::Object(m) => std::mem::take(m).into_values().collect(),
            _ => return,
        };
        while let Some(mut v) = parts.pop() {
            match &mut v {
                Any::Array(items) => parts.append(items),
                Any::Object(m) => parts.extend(std::mem::take(m).into_values()),
                _ => {}
            }
        }
    }
}

impl Any {
    /// The map inside, taken out (a pattern cannot move out of a type with its own Drop), or the value back.
    pub fn into_object(mut self) -> Result<BTreeMap<String, Any>, Any> {
        match &mut self {
            Any::Object(m) => Ok(std::mem::take(m)),
            _ => Err(self),
        }
    }

    /// The list inside, taken out, or the value back.
    pub fn into_array(mut self) -> Result<Vec<Any>, Any> {
        match &mut self {
            Any::Array(items) => Ok(std::mem::take(items)),
            _ => Err(self),
        }
    }

    /// The string inside, taken out, or the value back.
    pub fn into_string(mut self) -> Result<String, Any> {
        match &mut self {
            Any::String(s) => Ok(std::mem::take(s)),
            _ => Err(self),
        }
    }
}
