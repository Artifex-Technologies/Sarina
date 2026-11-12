//! What Go's `encoding/json` puts in an `any`: nil, bool, float64, string, `[]any`, `map[string]any`. Tool arguments and
//! schemas travel in this form.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Any {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Any>),
    /// Go's map: one value per key, read in sorted order where Go's order would be random
    Object(BTreeMap<String, Any>),
}
