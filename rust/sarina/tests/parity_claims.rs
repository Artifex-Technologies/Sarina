//! testdata/parity/claims.json: the answer checks of `go/agent/claims.go` and the secret redactor of `secrets.go`, on every
//! answer of their Go tests (and those changed in case, spacing and length), in every task of those tests. The fixture
//! holds only what a check did not let pass, by the answer's number; a task whose tool output Go may read in another order
//! lists every answer Go gave.
//!
//! One test function: it sets PATH (isCommand looks programs up), which the whole process shares.

mod common;

use std::collections::{BTreeMap, HashMap};

use sarina::agent::claims::*;
use sarina::agent::secrets::{hide_secrets_read, secret_values, tool_texts, StreamRedactor};
use sarina::goany::Any;
use sarina::llm::types::{FunctionCall, WireToolCall};
use sarina::session::Message;
use serde_json::{json, Map, Value};

fn to_any(v: &Value) -> Any {
    match v {
        Value::Null => Any::Null,
        Value::Bool(b) => Any::Bool(*b),
        Value::Number(n) => Any::Number(n.as_f64().unwrap()),
        Value::String(s) => Any::String(s.clone()),
        Value::Array(a) => Any::Array(a.iter().map(to_any).collect()),
        Value::Object(m) => Any::Object(m.iter().map(|(k, x)| (k.clone(), to_any(x))).collect()),
    }
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|x| x.as_str().unwrap().to_owned()).collect()).unwrap_or_default()
}

struct Scen {
    prompt: String,
    msgs: Vec<Message>,
    msg_start: usize,
    hist: Vec<Hist>,
    offered: Vec<String>,
    can_read: bool,
    can_write: bool,
}

fn scen(v: &Value) -> Scen {
    let msgs = v["msgs"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|m| Message {
                    role: m["role"].as_str().unwrap().into(),
                    content: m["content"].as_str().unwrap().into(),
                    name: m["name"].as_str().unwrap().into(),
                    tool_call_id: m["tool_call_id"].as_str().unwrap().into(),
                    message_id: m["message_id"].as_str().unwrap().into(),
                    tool_calls: m["calls"]
                        .as_array()
                        .map(|cs| {
                            cs.iter()
                                .map(|c| WireToolCall {
                                    id: c[0].as_str().unwrap().into(),
                                    kind: "function".into(),
                                    function: FunctionCall {
                                        name: c[1].as_str().unwrap().into(),
                                        arguments: c[2].as_str().unwrap().into(),
                                    },
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    metadata: m["meta"].as_object().map(|o| o.iter().map(|(k, x)| (k.clone(), to_any(x))).collect()).unwrap_or_default(),
                    ..Message::default()
                })
                .collect()
        })
        .unwrap_or_default();
    let hist = v["hist"]
        .as_array()
        .map(|a| a.iter().map(|e| e.as_object().unwrap().iter().map(|(k, x)| (k.clone(), to_any(x))).collect()).collect())
        .unwrap_or_default();
    Scen {
        prompt: v["prompt"].as_str().unwrap().into(),
        msgs,
        msg_start: v["msg_start"].as_u64().unwrap() as usize,
        hist,
        offered: strs(&v["offered"]),
        can_read: v["can_read"].as_bool().unwrap(),
        can_write: v["can_write"].as_bool().unwrap(),
    }
}

/// Go's json.Marshal of a []string: HTML characters escaped, nil as null.
fn go_strs(v: &[&str]) -> String {
    if v.is_empty() {
        return "null".into();
    }
    let mut out = String::from("[");
    for (i, s) in v.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        sarina::gojson::write_str(&mut out, s, true);
    }
    out + "]"
}

fn b(x: bool) -> String {
    if x {
        "true".into()
    } else {
        String::new()
    }
}

/// The checks of an answer alone, as the Go test runs them.
fn only(name: &str, a: &str) -> Value {
    let s = |x: String| if x.is_empty() { Value::Null } else { Value::String(x) };
    let t = |x: bool| if x { Value::Bool(true) } else { Value::Null };
    match name {
        "suggestsSecurityOff" => s(suggests_security_off(a)),
        "announcedNotDone" => t(announced_not_done(a)),
        "unwrapReminder" => {
            let u = unwrap_reminder(a);
            if u == a {
                Value::Null
            } else {
                s(u)
            }
        }
        "exfilInAnswer" => s(exfil_in_answer(a)),
        "notAQuote" => t(not_a_quote(&sarina::gotext::to_lower(a))),
        "markedExample" => t(marked_example(a)),
        "isCommand" => {
            let f = sarina::gotext::fields(a);
            t(!(f.first().is_some_and(|w| w.contains('/'))) && is_command(a))
        }
        "ownToolNamed" => t(own_tool_named(a, &["system_status".into(), "read_file".into(), "crypto_hash".into()])),
        "upToDateUnseen" => t(up_to_date_unseen(a, "crash records")),
        "defaultOutput" => t(default_output_for_one_app(a, "Firefox plays no sound but Spotify does")),
        "sentences" => s(go_strs(&sentences(a))),
        "sentenceAt" => {
            let out: Vec<&str> = (0..=a.len()).step_by(17).map(|i| sentence_at(a, i)).collect();
            s(go_strs(&out))
        }
        "closingAdvice" => {
            let mut out = Vec::new();
            for i in (0..=a.len()).step_by(11) {
                if a.is_char_boundary(i) {
                    let st = sentence_around(a, i, i);
                    if closing_advice(a, i, st) {
                        out.push(format!("{i}:{st}"));
                    }
                }
            }
            s(go_strs(&out.iter().map(String::as_str).collect::<Vec<_>>()))
        }
        "asPrompt" => {
            let r = json!([
                claims_tool_work("I checked it.", a, &[]),
                unreproduced_failure(a, &[], &["check_code".into()]),
                fix_described_not_made(a, "The fix: change a to b.", true, &[]),
                default_output_for_one_app("`wpctl set-default 52`", a),
                unsourced_command("Run `sudo pacman -Syu lua51` and `sudo pacman -Syu`.", a, &[]),
                digest_not_one_way("The hash is a digest.", a, &[]),
            ]);
            s(r.to_string())
        }
        _ => panic!("unknown check {name}"),
    }
}

fn check_in_task(name: &str, a: &str, sc: &Scen, ws: &str) -> String {
    let msgs = &sc.msgs[sc.msg_start.min(sc.msgs.len())..];
    let with_rem = format!("{}\n{}", sc.prompt, reminders(&sc.msgs));
    let pair = |x: String, y: String| if x.is_empty() && y.is_empty() { String::new() } else { format!("{x}\0{y}") };
    match name {
        "unbackedFileClaim" => unbacked_file_claim(a, &sc.hist),
        "unreportedChange" => unreported_change(a, &sc.hist),
        "ungroundedQuote" => ungrounded_quote(a, &sc.prompt, msgs),
        "claimsToolWork" => b(claims_tool_work(a, &sc.prompt, msgs)),
        "unsourcedCommand" => unsourced_command(a, &sc.prompt, msgs),
        "inventedHistory" => invented_history(a, &sc.msgs, sc.msg_start),
        "upToDateUnseen" => b(up_to_date_unseen(a, &evidence_of(&with_rem, msgs).0)),
        "ungroundedIdentifier" => ungrounded_identifier(a, &with_rem, msgs),
        "digestNotOneWay" => b(digest_not_one_way(a, &with_rem, msgs)),
        "checkedUnseenFile" => checked_unseen_file(a, msgs),
        "toolErrorIgnored" => b(tool_error_ignored(a, msgs)),
        "codeWorksUnverified" => code_works_unverified(a, &sc.hist),
        "deniedReadable" => denied_readable(a, &sc.prompt, ws, msgs, sc.can_read).replace(ws, "$W"),
        "fixDescribedNotMade" => b(fix_described_not_made(&sc.prompt, a, sc.can_write, &sc.hist)),
        "inventedCode" => {
            let (l, o) = invented_code(a, &sc.prompt, msgs);
            pair(l, o)
        }
        "decompileClaimed" => b(decompile_claimed(a, msgs)),
        "unreadDefinition" => {
            let (n, l) = unread_definition(a, msgs);
            if n.is_empty() {
                n
            } else {
                format!("{n}:{l}")
            }
        }
        "undecompiledFunction" => {
            let (n, h) = undecompiled_function(a, msgs);
            if n.is_empty() {
                n
            } else {
                format!("{n}\0{h}")
            }
        }
        "claimsNoTools" => b(claims_no_tools(a, msgs, &sc.offered)),
        "fixPending" => fix_pending_after_failed_check(a, &sc.hist),
        "failedWriteHidden" => failed_write_hidden(a, msgs),
        "toolNamedNotCalled" => tool_named_not_called(a, msgs, &sc.offered),
        "hideSecretsRead" => {
            let (o, ch) = hide_secrets_read(a, msgs);
            if ch {
                o
            } else {
                String::new()
            }
        }
        _ => panic!("unknown check {name}"),
    }
}

fn task(sc: &Scen, ws: &str) -> Value {
    let msgs = &sc.msgs[sc.msg_start.min(sc.msgs.len())..];
    let mut texts: Vec<String> = msgs.iter().filter(|m| m.role == "tool").flat_map(|m| tool_texts(&m.content)).collect();
    texts.sort();
    let mut vals = secret_values(&texts);
    vals.sort_by(|x, y| y.len().cmp(&x.len()).then(x.cmp(y)));
    let mut redacted: Vec<String> = Vec::new();
    if let Some(mut red) = StreamRedactor::new(msgs) {
        for step in [1, 3, 7] {
            for a in [
                "The password is hunter2-correct-horse-9 and the key abcd1234efgh, token bcde, pass: wxyz; é",
                "plain text without secrets",
            ] {
                let r: Vec<char> = a.chars().collect();
                let mut parts: Vec<String> = r.chunks(step).map(|c| red.push(&c.iter().collect::<String>())).collect();
                parts.push(red.flush());
                redacted.push(parts.join("|"));
            }
        }
    }
    json!({
        "taskInsteadOfFile": task_instead_of_file(&sc.prompt, ws, msgs, &sc.hist),
        "writeRefused": write_refused(msgs),
        "editLoopStopped": edit_loop_stopped(msgs),
        "reminders": reminders(&sc.msgs),
        "unreproducedFailure": unreproduced_failure(&sc.prompt, msgs, &sc.offered),
        "toolTexts": if texts.is_empty() { Value::Null } else { json!(texts) },
        "secretValues": vals,
        "redacted": redacted,
    })
}

#[test]
fn claims_match_go() {
    let cases = common::cases("claims");
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-claims-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let ws = tmp.to_str().unwrap().to_owned();
    for (name, body) in [
        ("config.env", "LOG_LEVEL=info\n"),
        ("todo.md", "- [ ] milk\n"),
        ("greeting.txt", "Hello wrold\n"),
        ("crashy.c", "int main(){}\n"),
        ("notes.txt", "Team sync.\n"),
        ("calc.go", "package calc\n"),
    ] {
        std::fs::write(tmp.join(name), body).unwrap();
    }
    std::fs::create_dir(tmp.join("dir.md")).unwrap();
    std::fs::create_dir(tmp.join("docs")).unwrap();
    std::os::unix::fs::symlink("notes.txt", tmp.join("link.txt")).unwrap();
    std::os::unix::fs::symlink("missing.md", tmp.join("dangling.md")).unwrap();
    let bin = tmp.join("bin");
    std::fs::create_dir(&bin).unwrap();
    for p in ["pacman", "chmod", "wpctl", "ls", "gpg", "systemctl", "go", "python3"] {
        std::fs::write(bin.join(p), "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join(p), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(bin.join("paru"), "x").unwrap();
    std::fs::create_dir(bin.join("yay")).unwrap();
    std::env::set_var("PATH", &bin);

    let head = &cases[0];
    assert_eq!(head["fn"], "answers");
    let answers: Vec<String> = head["out"].as_array().unwrap().iter().map(|v| String::from_utf8(common::bytes(v)).unwrap()).collect();
    let lits = head["in"]["literals"].as_u64().unwrap() as usize;
    let base = head["in"]["base"].as_u64().unwrap() as usize;
    let scens: HashMap<String, Scen> =
        cases.iter().filter(|c| c["fn"] == "task").map(|c| (c["in"]["name"].as_str().unwrap().to_owned(), scen(&c["in"]))).collect();
    let mut evaluated = 0usize;
    for c in &cases[1..] {
        let f = c["fn"].as_str().unwrap();
        match f {
            "task" => {
                let sc = &scens[c["in"]["name"].as_str().unwrap()];
                check.eq(format!("task {}", c["in"]["name"]), &task(sc, &ws), &c["out"]);
            }
            "texts" => {
                let got = json!([
                    claim_check_text("a.md"),
                    unreported_text("b.md"),
                    ungrounded_text("q"),
                    task_instead_of_file_text("t.md"),
                    security_off_text("ufw"),
                    unsourced_text("c"),
                    invented_history_text("h"),
                    ungrounded_identifier_text("1.2.3"),
                    checked_unseen_text("f.txt"),
                    code_works_text("why"),
                    denied_readable_text("/w/x/y.txt"),
                    invented_code_text("l", ""),
                    invented_code_text("l", "o"),
                    unread_definition_text("fn", 520),
                    undecompiled_function_text("n", "how"),
                    claims_no_tools_text(&["a".into(), "b".into()]),
                    fix_pending_text("f.py"),
                    failed_write_hidden_text("why"),
                    TOOL_WORK_TEXT,
                    ONE_APP_SILENT_TEXT,
                    DIGEST_NOT_ONE_WAY_TEXT,
                    TOOL_ERROR_IGNORED_TEXT,
                    FIX_DESCRIBED_TEXT,
                    DECOMPILE_CLAIMED_TEXT,
                    ANNOUNCED_TEXT,
                    UP_TO_DATE_TEXT,
                    EXFIL_IN_ANSWER_TEXT,
                    UNREPRODUCED_FAILURE_TEXT,
                    clip_quote("ééééé", 3),
                    clip_quote("abc", 3),
                ]);
                check.eq("texts", &got, &c["out"]);
            }
            _ if c["in"].is_null() => {
                let limit = if f == "sentenceAt" || f == "closingAdvice" { lits } else { answers.len() };
                let mut got = Map::new();
                for (i, a) in answers[..limit].iter().enumerate() {
                    let v = only(f, a);
                    evaluated += 1;
                    if !v.is_null() {
                        got.insert(i.to_string(), v);
                    }
                }
                check.eq(f, &Value::Object(got), &c["out"]);
            }
            _ => {
                let name = c["in"].as_str().unwrap();
                let sc = &scens[name];
                let want = c["out"].as_object().unwrap();
                let mut diffs = BTreeMap::new();
                for (i, a) in answers[..base].iter().enumerate() {
                    evaluated += 1;
                    let got = check_in_task(f, a, sc, &ws);
                    let alts: Vec<String> = want.get(&i.to_string()).map_or_else(|| vec![String::new()], strs);
                    if !alts.contains(&got) {
                        diffs.insert(i, (got, alts));
                    }
                }
                let shown: Vec<String> =
                    diffs.iter().take(3).map(|(i, (g, w))| format!("#{i} {:?}: rust {g:?}, go {w:?}", answers[*i])).collect();
                check.eq(format!("{f} in {name}"), &json!(shown), &json!([]));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    eprintln!("claims: {evaluated} answers checked");
    check.done("claims");
}
