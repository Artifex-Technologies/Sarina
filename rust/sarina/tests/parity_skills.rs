//! testdata/parity/skills.json: skill discovery on the same made-up tree (problems, collisions, notices), long skills,
//! the skill tool, skill_section and skill_search (by words, and by meaning with the same made-up embedder: no model is
//! involved), the index saved and loaded again, ExpandWith, headerless descriptions and cutParts.
//!
//! One test function: it changes the working folder, which the whole process shares.

mod common;

use sarina::embed::Embedder;
use sarina::goany::Any;
use sarina::gojson;
use sarina::gopath;
use sarina::skills::discover::{discover, headerless, roots, Catalog, Options};
use sarina::skills::library::cut_parts;
use sarina::skills::recall::{register_context, Library};
use sarina::skills::tool::{library_tools, skill_tool};
use sarina::tools::registry::{Context, Registry};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

struct FakeEmbedder {
    fail: bool,
}

impl Embedder for FakeEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        if self.fail {
            return Err("embedder down".into());
        }
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0f32; 32];
                for (j, c) in sarina::gotext::to_lower(t).char_indices() {
                    if c.is_ascii_lowercase() {
                        let c = c as usize;
                        v[(c - 'a' as usize) % 32] += 1.0;
                        v[(c * 7 + j % 3) % 32] += 0.25;
                    }
                }
                v
            })
            .collect())
    }

    fn name(&self) -> String {
        "fake-embed".into()
    }
}

fn long_skill(name: &str, sections: usize, section_bytes: usize) -> String {
    let mut b = format!("---\nname: {name}\ndescription: a long one\nmodel_invocable: true\n---\nIntroduction to {name}.\n\n");
    let topics = ["compass navigation", "river crossing", "fire making", "shelter building", "water purification", "knot tying"];
    for i in 0..sections {
        let topic = topics[i % topics.len()];
        b.push_str(&format!("## Chapter {}: {topic}\n\n", i + 1));
        let mut b2 = 0;
        while b2 < section_bytes {
            b.push_str(&format!("Paragraph about {topic}, line {}, with more words to fill the space here.\n\n", b2 / 120));
            b2 += 120;
        }
    }
    b
}

fn sha8(s: &str) -> String {
    sha8_bytes(s.as_bytes())
}

fn sha8_bytes(b: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(b)[..8].iter().map(|x| format!("{x:02x}")).collect()
}

fn catalog_out(c: &Catalog, hide: &dyn Fn(&str) -> String) -> Value {
    let or_null = |v: Value| if v.as_array().is_some_and(|a| a.is_empty()) { Value::Null } else { v };
    let sk: Vec<Value> = c
        .skills
        .iter()
        .map(|s| {
            let mut secs: Vec<Value> = s.sections.iter().map(|x| json!([x.title, x.text.len(), sha8(&x.text)])).collect();
            if s.sections.len() > 50 {
                let all: String = s.sections.iter().map(|x| format!("{}\0{}\0", x.title, x.text)).collect();
                secs = vec![json!("many"), json!(s.sections.len()), json!(sha8(&all))];
            }
            json!([
                s.name,
                s.description,
                s.when_to_use,
                if s.has_allowed { json!(s.allowed_tools) } else { Value::Null },
                s.has_allowed,
                s.user_invocable,
                s.model_invocable,
                hide(&s.path),
                s.source,
                s.prefetch.as_ref().map_or(Value::Null, |p| json!(p)),
                s.body,
                or_null(Value::Array(secs))
            ])
        })
        .collect();
    let probs: Vec<Value> = c.problems.iter().map(|p| json!([hide(&p.path), hide(&p.message)])).collect();
    let cols: Vec<Value> = c.collisions.iter().map(|x| json!([x.name, hide(&x.kept), hide(&x.shadowed)])).collect();
    let notes: Vec<Value> = c.notices.iter().map(|n| json!(hide(n))).collect();
    json!([or_null(Value::Array(sk)), or_null(Value::Array(probs)), or_null(Value::Array(cols)), or_null(Value::Array(notes))])
}

#[test]
fn skills_match_go() {
    let cases = common::cases("skills");
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-skills-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let hide = |s: &str| s.replace(&top, "$BASE");
    let roots_out = |o: &Options| -> Value {
        let r: Vec<Value> = roots(o).iter().map(|r| json!([hide(&r.dir), r.source])).collect();
        if r.is_empty() {
            Value::Null
        } else {
            Value::Array(r)
        }
    };
    let env: HashMap<String, String> =
        HashMap::from([("SARINA_SKILLS_DIR".to_owned(), format!("{top}/envdir1:{top}/envdir2::rel:{top}/envdir1/"))]);
    let opts = Options {
        home: format!("{top}/home"),
        getenv: Some(Arc::new(move |k: &str| env.get(k).cloned().unwrap_or_default())),
        workspace: format!("{top}/ws"),
        system_dir: format!("{top}/sys"),
        trust: Some(Arc::new(|p: &str| if p.contains("untrusted") { "not on the owner's signed list".into() } else { String::new() })),
        ..Options::default()
    };
    for c in &cases {
        match c["fn"].as_str().unwrap() {
            "tree" => {
                for e in c["in"].as_array().unwrap() {
                    let p = format!("{top}/{}", e[0].as_str().unwrap());
                    std::fs::create_dir_all(gopath::os_path(&gopath::dir(p.as_bytes()))).unwrap();
                    let content = e[2].as_str().unwrap();
                    match e[1].as_str().unwrap() {
                        "file" => std::fs::write(&p, content.replace("$BASE", &top)).unwrap(),
                        "link" => std::os::unix::fs::symlink(content, &p).unwrap(),
                        "sparse" => std::fs::File::create(&p).unwrap().set_len(content.parse().unwrap()).unwrap(),
                        "long" => {
                            let f: Vec<&str> = content.split('|').collect();
                            std::fs::write(&p, long_skill(f[0], f[1].parse().unwrap(), f[2].parse().unwrap())).unwrap();
                        }
                        "repeat" => {
                            let f: Vec<&str> = content.split('\x1f').collect();
                            let suffix = f.get(3).copied().unwrap_or("");
                            std::fs::write(&p, format!("{}{}{suffix}", f[0], f[1].repeat(f[2].parse().unwrap()))).unwrap();
                        }
                        k => panic!("{k}"),
                    }
                }
                std::env::set_current_dir(&top).unwrap();
                std::env::set_var("PWD", &top);
            }
            "discover" => {
                let mut o = opts.clone();
                match c["in"].as_str().unwrap() {
                    "workspace" => o.trust_workspace = true,
                    "notrust" => o.trust = None,
                    "nohome" => o.home = String::new(),
                    _ => {}
                }
                let got = json!([roots_out(&o), catalog_out(&discover(&o), &hide)]);
                check.eq(format!("discover {}", c["in"]), &got, &c["out"]);
            }
            "roots" => {
                let sj = c["in"].as_str().unwrap().replace("$BASE", &top);
                std::fs::write(format!("{top}/home/.sarina/skills.json"), sj).unwrap();
                check.eq(format!("roots {}", c["in"]), &roots_out(&opts), &c["out"]);
            }
            "tools" => {
                std::fs::write(format!("{top}/home/.sarina/skills.json"), format!("{{\"paths\":[\"{top}/jsondir\",\"{top}/envdir1\"]}}"))
                    .unwrap();
                let cat = Arc::new(discover(&opts));
                register_context("status", Arc::new(|task: &str| format!("STATUS about {task}").into_bytes()));
                let emb = c["in"][0].as_str().unwrap();
                let e: Option<Arc<dyn Embedder + Send + Sync>> = match emb {
                    "fake" => Some(Arc::new(FakeEmbedder { fail: false })),
                    "down" => Some(Arc::new(FakeEmbedder { fail: true })),
                    _ => None,
                };
                let state = format!("{top}/state-{emb}");
                let lib = Arc::new(Library::new(cat.clone(), e, &state));
                let mut tools = Vec::new();
                if let Some(t) = skill_tool(cat.clone(), Some(lib.clone())) {
                    tools.push(t);
                }
                tools.extend(library_tools(cat.clone(), lib.clone()));
                let reg = Registry::new(tools);
                let specs: Vec<Value> = reg
                    .specs()
                    .iter()
                    .map(|sp| {
                        let params = gojson::marshal_any(&Any::Object(sp.parameters.clone().unwrap_or_default())).unwrap();
                        json!([sp.name, sp.description, params])
                    })
                    .collect();
                let tc = Context::default();
                let mut out = Vec::new();
                for call in c["in"][1].as_array().unwrap() {
                    let args = gojson::unmarshal_any(call[1].as_str().unwrap().as_bytes()).0.into_object().unwrap();
                    let r = reg.execute(call[0].as_str().unwrap(), Some(args), &tc);
                    let md = gojson::marshal_any(&Any::Object(r.metadata.clone())).unwrap();
                    out.push(json!([r.ok, hide(&r.content), md]));
                }
                lib.start("OPER");
                let args = gojson::unmarshal_any(br#"{"skill":"oper","section":"fire"}"#).0.into_object().unwrap();
                let r = reg.execute("skill_section", Some(args), &tc);
                out.push(json!([r.ok, hide(&r.content), ""]));
                let ep = cat.get("epsilon").unwrap();
                out.push(json!([true, ep.expand_with("check the disk", Some(&lib)), ""]));
                out.push(json!([true, ep.expand_with("", Some(&lib)), ""]));
                if emb == "fake" {
                    let lib2 = Library::new(cat.clone(), Some(Arc::new(FakeEmbedder { fail: false })), &state);
                    let res = lib2.search("long", "fire making", 3);
                    let (found, how) = res.clone().unwrap_or_default();
                    let fo: Vec<Value> =
                        found.iter().map(|p| json!([p.skill, p.section, p.title, sha8(&p.text), format!("{:.6}", p.score)])).collect();
                    let fo = if fo.is_empty() { Value::Null } else { Value::Array(fo) };
                    let entries = std::fs::read_dir(&state).map(|d| d.count()).unwrap_or(0);
                    out.push(json!([res.is_ok(), how, fo]));
                    out.push(json!([entries > 0, "", ""]));
                    let mut dirs: Vec<String> = std::fs::read_dir(&state)
                        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
                        .unwrap_or_default();
                    dirs.sort();
                    let files: Vec<Value> = dirs
                        .iter()
                        .map(|d| {
                            let meta = std::fs::read(format!("{state}/{d}/meta.json")).unwrap_or_default();
                            let vec = std::fs::read(format!("{state}/{d}/passages.vec")).unwrap_or_default();
                            json!([d, String::from_utf8_lossy(&meta), sha8_bytes(&vec)])
                        })
                        .collect();
                    out.push(json!([true, "", files]));
                }
                let want = &c["out"];
                check.eq(format!("tools {emb}: specs"), &Value::Array(specs), &want[0]);
                for (i, (got, w)) in out.iter().zip(want[1].as_array().unwrap()).enumerate() {
                    let what = c["in"][1].get(i).map_or("after the calls".to_owned(), |x| x.to_string());
                    check.eq(format!("tools {emb}: call {i} {what}"), got, w);
                }
                check.eq(format!("tools {emb}: number of answers"), &json!(out.len()), &json!(want[1].as_array().unwrap().len()));
            }
            "headerless" => check.eq(format!("headerless {}", c["in"]), &json!(headerless(c["in"].as_str().unwrap())), &c["out"]),
            "cutParts" => {
                let got = cut_parts(c["in"][0].as_str().unwrap(), c["in"][1].as_u64().unwrap() as usize);
                let got = if got.is_empty() { Value::Null } else { json!(got) };
                check.eq(format!("cutParts {}", c["in"]), &got, &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    std::env::set_current_dir("/").unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("skills");
}
