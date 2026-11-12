//! The tool definitions the model sees on every request (name, description, parameter schema), with each tool's
//! permission and plan-mode class, for every built-in and provider tool of the Go build with its optional tool sets on.
//! `defs.json` is written by go/cmd/sarina's parity test from the Go tools themselves (`SARINA_PARITY_UPDATE=1 go test
//! ./cmd/sarina -run ParityToolDefs`), so the two cannot drift; testdata/parity/tooldefs.json holds what Go sends for
//! each, which tests/parity_tooldefs.rs compares with what this port writes. Which of them a session offers (the
//! SARINA_CRYPTO, SARINA_MATH, ... switches and their order) comes with the providers.

use super::registry::{Class, Permission};
use crate::goany::Any;
use crate::gojson;
use crate::llm::types::ToolSpec;
use std::sync::OnceLock;

/// One tool's definition.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDef {
    pub spec: ToolSpec,
    pub requires: Permission,
    pub class: Class,
}

/// Every definition, in the Go build's order.
pub fn definitions() -> &'static [ToolDef] {
    static DEFS: OnceLock<Vec<ToolDef>> = OnceLock::new();
    DEFS.get_or_init(|| {
        let (all, err) = gojson::unmarshal_any(include_bytes!("defs.json"));
        assert!(err.is_none(), "defs.json: {err:?}");
        let Any::Array(items) = &all else { panic!("defs.json is not a list") };
        items
            .iter()
            .map(|it| {
                let Any::Object(m) = it else { panic!("defs.json: not an object") };
                let s = |k: &str| match m.get(k) {
                    Some(Any::String(s)) => s.clone(),
                    _ => panic!("defs.json: {k}"),
                };
                ToolDef {
                    spec: ToolSpec {
                        name: s("name"),
                        description: s("description"),
                        parameters: match m.get("parameters") {
                            Some(Any::Object(p)) => Some(p.clone()),
                            _ => None,
                        },
                    },
                    requires: match s("requires").as_str() {
                        "write" => Permission::Write,
                        "shell" => Permission::Shell,
                        _ => Permission::None,
                    },
                    class: match m.get("class") {
                        Some(Any::Number(n)) if *n == 1.0 => Class::ReadOnly,
                        Some(Any::Number(n)) if *n == 2.0 => Class::Mutating,
                        Some(Any::Number(n)) if *n == 3.0 => Class::PlanControl,
                        _ => Class::Unset,
                    },
                }
            })
            .collect()
    })
}

/// A definition by name.
pub fn definition(name: &str) -> Option<&'static ToolDef> {
    definitions().iter().find(|d| d.spec.name == name)
}

/// A tool with the Go build's definition of that name and the given handler.
pub fn tool(name: &str, handler: super::registry::Handler) -> super::registry::Tool {
    let d = definition(name).unwrap_or_else(|| panic!("no definition named {name}"));
    super::registry::Tool { spec: d.spec.clone(), handler, requires: d.requires, class: d.class }
}
