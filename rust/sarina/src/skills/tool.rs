//! Go: `go/skills/tool.go`: the `skill` tool (only model_invocable skills listed or runnable; a skill's allowed_tools in
//! the metadata, which the agent uses only to narrow its registry), and `skill_section` and `skill_search` for long
//! skills (testdata/parity/skills.json).

use std::collections::BTreeMap;
use std::sync::Arc;

use super::discover::Catalog;
use super::recall::Library;
use crate::goany::Any;
use crate::gotext;
use crate::llm::types::ToolSpec;
use crate::tools::files::fail;
use crate::tools::registry::{Class, Output, Permission, Tool};

pub const TOOL_NAME: &str = "skill";
pub const SECTION_TOOL_NAME: &str = "skill_section";
pub const SEARCH_TOOL_NAME: &str = "skill_search";

fn obj(pairs: &[(&str, Any)]) -> Any {
    Any::Object(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
}

fn s(v: &str) -> Any {
    Any::String(v.to_owned())
}

fn str_prop(desc: &str, min: bool) -> Any {
    let mut p = vec![("type", s("string")), ("description", s(desc))];
    if min {
        p.push(("minLength", Any::Number(1.0)));
    }
    obj(&p)
}

fn params(props: &[(&str, Any)], required: &[&str]) -> BTreeMap<String, Any> {
    BTreeMap::from([
        ("type".to_owned(), s("object")),
        ("properties".to_owned(), obj(props)),
        ("required".to_owned(), Any::Array(required.iter().map(|r| s(r)).collect())),
    ])
}

fn arg(args: &BTreeMap<String, Any>, k: &str) -> String {
    match args.get(k) {
        Some(Any::String(v)) => v.clone(),
        _ => String::new(),
    }
}

/// Go's `Tool`: the skill tool, or None when no skill is model_invocable (then it is not offered).
pub fn skill_tool(cat: Arc<Catalog>, lib: Option<Arc<Library>>) -> Option<Tool> {
    let list: Vec<String> = cat.model_invocable().iter().map(|s| s.name.clone()).collect();
    if list.is_empty() {
        return None;
    }
    let mut b =
        String::from("Run a named, reusable procedure. The result is the procedure's instructions; follow them. Available skills:\n");
    for sk in cat.model_invocable() {
        b.push_str(&format!("- {}: {}", sk.name, sk.description));
        if !sk.when_to_use.is_empty() {
            b.push_str(&format!(" (use when: {})", sk.when_to_use));
        }
        b.push('\n');
    }
    let spec = ToolSpec {
        name: TOOL_NAME.into(),
        description: b.trim_end_matches('\n').to_owned(),
        parameters: Some(params(
            &[
                ("name", str_prop("The skill's name.", true)),
                ("arguments", str_prop("Text substituted for $ARGUMENTS in the skill.", false)),
            ],
            &["name"],
        )),
    };
    let handler = move |args: &BTreeMap<String, Any>, _: &crate::tools::registry::Context| -> Output {
        let (name, arguments) = (arg(args, "name"), arg(args, "arguments"));
        let Some(sk) = cat.get(&name).filter(|sk| sk.model_invocable) else {
            return fail(format!("no skill named {} is available to you; available: {}", gotext::quote(&name), list.join(", ")));
        };
        let mut md = BTreeMap::from([("skill".to_owned(), s(&sk.name))]);
        if sk.has_allowed {
            md.insert("skill_allowed_tools".into(), Any::Array(sk.allowed_tools.iter().map(|t| s(t)).collect()));
        }
        Output { content: sk.expand_with(&arguments, lib.as_deref()), metadata: md, error: None }
    };
    Some(Tool { spec, handler: Arc::new(handler), requires: Permission::None, class: Class::ReadOnly, stream: None })
}

/// Go's `LibraryTools`: skill_section and skill_search, offered when any skill is long (none otherwise).
pub fn library_tools(cat: Arc<Catalog>, lib: Arc<Library>) -> Vec<Tool> {
    if !cat.skills.iter().any(|sk| sk.is_library()) {
        return Vec::new();
    }
    let long: Vec<String> = cat.skills.iter().filter(|sk| sk.is_library() && sk.model_invocable).map(|sk| sk.name.clone()).collect();
    let names = if long.is_empty() { "(only one the operator has started)".to_owned() } else { long.join(", ") };
    let (c1, l1, n1) = (cat.clone(), lib.clone(), names.clone());
    let section = Tool {
        spec: ToolSpec {
            name: SECTION_TOOL_NAME.into(),
            description: format!("Read one section of a long skill (its table of contents is in the skill's text). Long skills: {names}"),
            parameters: Some(params(
                &[
                    ("skill", str_prop("The skill's name.", true)),
                    ("section", str_prop("The section's number from the table of contents, or (part of) its title.", true)),
                ],
                &["skill", "section"],
            )),
        },
        handler: Arc::new(move |args, _| {
            let (name, which) = (arg(args, "skill"), arg(args, "section"));
            let Some(sk) = c1.get(&name).filter(|sk| sk.is_library() && l1.readable(sk)) else {
                return fail(format!("no long skill named {} that you may read; long skills: {n1}", gotext::quote(&name)));
            };
            let Some((n, sec)) = sk.section(&which) else {
                return fail(format!(
                    "skill {} has no section {} (it has {}; use a number from its table of contents, or skill_search)",
                    sk.name,
                    gotext::quote(&which),
                    sk.sections.len()
                ));
            };
            let next =
                if n < sk.sections.len() { format!("\n\n(next: section {}, {})", n + 1, sk.sections[n].title) } else { String::new() };
            Output {
                content: format!("{}, section {n} of {}: {}\n\n{}{next}", sk.name, sk.sections.len(), sec.title, sec.text),
                metadata: BTreeMap::from([("skill".to_owned(), s(&sk.name)), ("section".to_owned(), Any::Int(n as i64))]),
                error: None,
            }
        }),
        requires: Permission::None,
        class: Class::ReadOnly,
        stream: None,
    };
    let search = Tool {
        spec: ToolSpec {
            name: SEARCH_TOOL_NAME.into(),
            description: format!(
                "Find the passages of long skills that are about something (by meaning when the embedding model is there, else by \
                 words). Long skills: {names}"
            ),
            parameters: Some(params(
                &[
                    ("query", str_prop("What to find, in words.", true)),
                    ("skill", str_prop("Only this skill (optional; all long skills without it).", false)),
                    ("limit", obj(&[("type", s("integer")), ("description", s("How many passages (default 5, at most 10)."))])),
                ],
                &["query"],
            )),
        },
        handler: Arc::new(move |args, _| {
            let (query, skill) = (arg(args, "query"), arg(args, "skill"));
            let mut limit = 5;
            if let Some(Any::Number(f)) = args.get("limit") {
                if *f >= 1.0 {
                    limit = (*f as i64).min(10) as usize;
                }
            }
            let (found, how) = match lib.search(&skill, &query, limit) {
                Ok(r) => r,
                Err(e) => return fail(e),
            };
            if found.is_empty() {
                return Output {
                    content: format!(
                        "Nothing in the long skills matches that ({how}). Try other words, or open a section from a table of contents."
                    ),
                    ..Output::default()
                };
            }
            let mut b = format!("{} passage(s), found {how}:\n", found.len());
            for p in &found {
                b.push_str(&format!("\n--- {}, section {}: {} ---\n{}\n", p.skill, p.section, p.title, p.text));
            }
            Output { content: b, metadata: BTreeMap::from([("found".to_owned(), Any::Int(found.len() as i64))]), error: None }
        }),
        requires: Permission::None,
        class: Class::ReadOnly,
        stream: None,
    };
    vec![section, search]
}
