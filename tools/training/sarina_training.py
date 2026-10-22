#!/usr/bin/env python3
"""Turn Sarina's real conversations into training examples, with the owner deciding which ones count. Nothing is trained here.

  python3 sarina_training.py review [--days 14] [--out FILE.md]   write a review sheet: each question and her final answer, unticked
  python3 sarina_training.py export SHEET.md [--out FILE.jsonl]    the ticked examples, as chat JSONL (what Soup and most trainers read)

The review sheet is plain Markdown (open it in Obsidian): tick [x] the answers that are right and sound like her, leave the rest. Only ticked
ones are exported. Left out on purpose: eval runs (their folders are under /tmp/sarina-evals), sessions that held a credential, answers that
were sent back by her honesty checks, and empty answers. The question is stored without what was looked up for it (system reports, guide
notes), so she learns the answer's shape, not old numbers.
"""
import argparse
import datetime
import glob
import json
import os
import re
import sys

SESSIONS = os.environ.get("SARINA_SESSION_DIR", "/var/lib/sarina/sessions")
CRED = re.compile(r"\[REDACTED|AKIA[A-Z0-9]{16}|-----BEGIN [A-Z ]*PRIVATE KEY|(?i:password\s*[=:])")
PREFIX = re.compile(r"(?s)^.*?The question: ", re.S)


def text(m):
    c = m.get("content")
    if isinstance(c, list):
        c = " ".join(x.get("text", "") for x in c if isinstance(x, dict))
    return c or ""


def eval_prompts():
    """The questions of Sarina's evals: those sessions are tests, not conversations, wherever they ran."""
    p = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "evals", "run_evals.py")
    try:
        return open(p, encoding="utf-8").read()
    except OSError:
        return ""


def examples(days):
    cut = datetime.datetime.now().timestamp() - days * 86400
    out = []
    evals = eval_prompts()
    for f in sorted(glob.glob(os.path.join(SESSIONS, "*.json")), key=os.path.getmtime):
        if os.path.getmtime(f) < cut:
            continue
        try:
            d = json.load(open(f))
        except (OSError, ValueError):
            continue
        cwd = str((d.get("runtime_config") or {}).get("cwd", ""))
        if cwd.startswith("/tmp/sarina-evals"):
            continue
        msgs = d.get("messages") or []
        whole = json.dumps(msgs)
        if CRED.search(whole) or "claim_check" in whole or "empty_reply" in whole:
            continue
        users = [m for m in msgs if m.get("role") == "user" and not str(m.get("message_id", "")).startswith(("user_context", "continuation"))]
        finals = [m for m in msgs if m.get("role") == "assistant" and text(m).strip() and not m.get("tool_calls")]
        if not users or not finals:
            continue
        q = text(users[-1])
        q = PREFIX.sub("", q) if "The question: " in q else q
        q = re.sub(r"^\(Style hint[^)]*\)\s*", "", q).strip()
        a = text(finals[-1]).strip()
        if len(q) < 3 or len(a) < 3 or len(q) > 2000:
            continue
        if len(q) > 20 and q[:60] in evals:
            continue
        system = text(msgs[0]) if msgs and msgs[0].get("role") == "system" else ""
        out.append({"id": os.path.basename(f)[:-5], "question": q, "answer": a, "system": system[:4000]})
    return out


def review(a):
    ex = examples(a.days)
    out = a.out or os.path.expanduser("~/Documents/Obsidian/Project Docs/Sarina training review %s.md" % datetime.date.today().isoformat())
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        f.write("# Sarina training review\n\nTick [x] each answer that is right and sounds like her. Only ticked ones become training examples. "
                "Edit an answer below its question if you want it to say something better: the edited text is what she will learn.\n\n")
        for e in ex:
            f.write("## [ ] %s\n\n**You:** %s\n\n**Sarina:**\n\n%s\n\n<!-- id:%s -->\n\n" % (e["id"][:8], e["question"].replace("\n", " "), e["answer"], e["id"]))
    print("%d candidates written to %s" % (len(ex), out))
    return 0


def export(a):
    sheet = open(a.sheet, encoding="utf-8").read()
    by_id = {e["id"]: e for e in examples(3650)}
    rows = []
    for block in re.split(r"\n(?=## \[)", sheet):
        m = re.match(r"## \[(x|X)\]", block)
        idm = re.search(r"<!-- id:([0-9a-f]+) -->", block)
        if not m or not idm:
            continue
        e = by_id.get(idm.group(1))
        if not e:
            continue
        ans = re.search(r"\*\*Sarina:\*\*\n\n(.*?)\n\n<!-- id:", block, re.S)
        answer = ans.group(1).strip() if ans else e["answer"]
        rows.append({"messages": [{"role": "system", "content": e["system"]}, {"role": "user", "content": e["question"]}, {"role": "assistant", "content": answer}]})
    out = a.out or os.path.expanduser("~/.local/share/sarina/training/approved.jsonl")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    print("%d approved examples written to %s" % (len(rows), out))
    return 0


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("review")
    r.add_argument("--days", type=int, default=14)
    r.add_argument("--out")
    e = sub.add_parser("export")
    e.add_argument("sheet")
    e.add_argument("--out")
    a = ap.parse_args(argv[1:])
    return review(a) if a.cmd == "review" else export(a)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
