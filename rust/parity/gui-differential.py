#!/usr/bin/env python3
"""The Go console and sarina-rs's console (`sarina gui`), compared request by request (docs/RUST_PORT.md "The GUI
differential").

Each scenario starts `sarina gui` from the Go build and then from sarina-rs (SARINA_RS_TRY=gui, SARINA_RS_NO_GO=1: a
console sarina-rs did not run itself fails instead of reaching Go), each in a throwaway folder made afresh with the same
made-up home and workspace, the same flags and environment, on a loopback port the kernel picks (--port 0) and with
--no-browser (a scenario that leaves it out finds only its own stand-in xdg-open on PATH: no window is ever opened). The
console prints its launch link with a random token; the steps read it from there and send it as the page does
(X-Sarina-Token, the ?t= query, the cookie). The steps: raw HTTP requests (the page, the static files with ranges and
conditions, the browser guard, every API endpoint, bodies of every form), runs answered as JSON and as an event stream,
the event stream re-attached, approvals of tool calls and of plans answered while a run waits, cancels, a client that
hangs up, signals. Behind it stand the service differential's scripted servers: one loopback HTTP server for the model
(the context-window probes get 404), Heimdall's review API and the backend probe, and Limbo's agent socket. Nothing
leaves the machine and no real model, session, vault or memory is touched.

Compared: every response byte for byte (an event stream's chunk boundaries depend on timing, so its body is compared
de-chunked; the Date header, the port, the launch token, times, run, approval and session ids and a multipart
boundary normalised), the console's stderr and exit status, every request the scripted servers got, and every file left
in the folder.

    python3 -I rust/parity/gui-differential.py --go GO_SARINA --rs SARINA_RS [--out DIR] [--only SUBSTRING] [--show]

Exit status 0 when the two agree on every scenario.
"""

import argparse
import importlib.util
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))


def _load(name, fname):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, fname))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


sd = _load("svcdiff", "service-differential.py")
cd = sd.cd

resp, completion, sse_completion, state, HANGUP = cd.resp, cd.completion, cd.sse_completion, cd.state, cd.HANGUP
session, u, a, call, result = cd.session, cd.u, cd.a, cd.call, cd.result
read_response, readline, readexact, readall, fill_buf, connect = (sd.read_response, sd.readline, sd.readexact, sd.readall,
                                                                  sd.fill, sd.connect)
SESSIONS = cd.SESSIONS


class Backend(sd.Backend):
    """The service differential's model and Heimdall, plus the console's backend probe: a request under /probe/ takes the
    next of `probe`'s answers (anything else not the model's or Heimdall's is a context-window probe: 404)."""

    def reset(self, model, heimdall, probe=()):
        super().reset(model, heimdall)
        with self.lock:
            self.probe = list(probe)

    def serve(self, conn):
        # peek at the target: a /probe/ request is answered from `probe` (the base class answers the rest)
        try:
            conn.settimeout(30)
            head = b""
            while b"\r\n\r\n" not in head:
                d = conn.recv(65536, socket.MSG_PEEK)
                if not d or len(d) == len(head):
                    if not d:
                        conn.close()
                        return
                    time.sleep(0.01)
                head = d
                if len(head) > 1 << 20:
                    break
        except OSError:
            conn.close()
            return
        target = head.split(b" ", 2)[1] if head.count(b" ") >= 2 else b""
        if not target.startswith(b"/probe"):
            return super().serve(conn)
        try:
            data = b""
            while b"\r\n\r\n" not in data:
                d = conn.recv(65536)
                if not d:
                    return
                data += d
            h, _, _ = data.partition(b"\r\n\r\n")
            lines = h.split(b"\r\n")
            hdrs = {}
            for ln in lines[1:]:
                k, _, v = ln.partition(b":")
                hdrs.setdefault(k.decode("latin-1").strip().lower(), v.strip().decode("latin-1"))
            method, tgt = lines[0].split(b" ")[:2]
            with self.lock:
                self.requests.append({"kind": "probe", "method": method.decode("latin-1"), "target": tgt.decode("latin-1"),
                                      "host": hdrs.get("host"), "authorization": hdrs.get("authorization"),
                                      "content-type": hdrs.get("content-type"), "body": ""})
                ans = self.probe.pop(0) if self.probe else resp("no answer scripted", "599 Script Ended", ctype="text/plain")
            if isinstance(ans, tuple) and ans[0] == "slow":
                conn.settimeout(ans[1])
                try:
                    if conn.recv(1) == b"":
                        return
                except socket.timeout:
                    pass
                ans = ans[2]
            if ans == HANGUP:
                return
            conn.sendall(ans)
        except OSError:
            return
        finally:
            try:
                conn.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            conn.close()


# ---- scenarios -------------------------------------------------------------------------------------------------------

KEY = "sk-made-up-model-key-456789"
BASE_ENV = {"OPENAI_BASE_URL": "{H}/v1", "OPENAI_MODEL": "made-up-model", "SARINA_RS_TRY": "gui",
            "SARINA_GO_BINARY": "{G}", "HEIMDALL_REVIEW_URL": "{H}/v1/review", "SARINA_LIMBO_SOCKET": "{S}"}


def S(name, steps, args=(), env=None, files=None, model=(), heimdall=(), probe=(), limbo=(), argv0=None, browser=False,
      stream=False):
    """A scenario. The console streams the model's answers by default; the scenarios script plain JSON answers and start
    it with --stream=false, but for those with `stream` (scripted with sse_completion and sse_calls)."""
    return {"name": name, "steps": list(steps), "args": ([] if stream else ["--stream=false"]) + list(args),
            "env": dict(BASE_ENV, **(env or {})), "files": state() if files is None else files, "model": list(model),
            "heimdall": list(heimdall), "probe": list(probe), "limbo": list(limbo), "argv0": argv0, "browser": browser}


def sse_calls(calls):
    """A streamed answer that calls tools (each call's arguments in one delta)."""
    lines = [json.dumps({"id": "c1", "choices": [{"index": 0, "delta": {"tool_calls": [
        {"index": i, "id": f"call_{i}", "type": "function", "function": {"name": n, "arguments": json.dumps(a)}}]}}]})
        for i, (n, a) in enumerate(calls)]
    lines.append(json.dumps({"id": "c1", "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                             "usage": {"prompt_tokens": 9, "completion_tokens": 3, "total_tokens": 12}}))
    return resp("".join(f"data: {x}\n\n" for x in lines) + "data: [DONE]\n\n", ctype="text/event-stream")


def R(method, path, body=None, **k):
    """A request; the launch token goes in X-Sarina-Token unless token=None (or another value) is given."""
    return ("req", method, path, body, k)


def ES(method, path, body=None, **k):
    """A request whose answer is an event stream: read until it ends, or (an open stream) until its "done" event."""
    return ("sse", method, path, body, k)


def BG(step):
    """A step started in the background; its result is recorded at the next ("join",)."""
    return ("bg", step)


JOIN = ("join",)


def AP(n=1):
    """Waits (at most 15 s) until n approvals are pending, then records GET /api/approvals (its ids become {AP0}...)."""
    return ("approvals", n)


def RUNNING():
    """Waits (at most 15 s) until a model request is waiting (the run is in its model call)."""
    return ("model_waiting",)


def post_state(patch, **k):
    return R("POST", "/api/state", patch, **k)


def chat(prompt, **k):
    return R("POST", "/api/chat", {"prompt": prompt} if isinstance(prompt, str) else prompt, **k)


def chat_stream(prompt, **k):
    hdr = dict(k.pop("headers", {}), Accept="text/event-stream")
    return ES("POST", "/api/chat", {"prompt": prompt} if isinstance(prompt, str) else prompt, headers=hdr, **k)


def events(after=0, **k):
    return ES("GET", "/api/events?after=%d" % after, **k)


def here_session(sid, msgs, **extra):
    return session(sid, msgs, runtime_config={"cwd": "{W}/ws", "max_turns": 12}, **extra)


WRITE_OK = {"allow_write": True, "confirm_permissions": True}
SHELL_OK = {"allow_shell": True, "confirm_permissions": True}


def cli_scenarios():
    sc = []
    sc.append(S("help", [], args=["-h"], env={"OPENAI_API_KEY": KEY}))
    sc.append(S("bad flag", [], args=["--bogus"]))
    sc.append(S("bad float", [], args=["--temperature", "warm"]))
    sc.append(S("bad int", [], args=["--max-turns", "1.5"]))
    sc.append(S("extra words", [R("GET", "/api/state")], args=["extra", "words"]))
    sc.append(S("non-loopback", [], args=["--host", "0.0.0.0"]))
    sc.append(S("non-loopback name", [], args=["--host", "example.invalid"]))
    sc.append(S("cwd missing", [], args=["--cwd", "{W}/nowhere"]))
    sc.append(S("cwd a file", [], args=["--cwd", "{W}/ws/notes.txt"], files=state(**{"ws/notes.txt": "x"})))
    sc.append(S("cwd blank", [], args=["--cwd", "  "]))
    sc.append(S("add-dir missing", [], args=["--add-dir", "{W}/empty", "--add-dir", "{W}/nowhere"]))
    sc.append(S("port in use", [], args=["--port", "{BUSY}"]))
    sc.append(S("bad port", [], args=["--port", "70000"]))
    sc.append(S("host localhost", [R("GET", "/api/state", host="localhost:{PORT}")], args=["--host", "localhost"]))
    sc.append(S("host ::1", [("raw6", b"GET /api/state HTTP/1.1\r\nHost: [::1]:{PORT}\r\nX-Sarina-Token: {TOKEN}\r\n\r\n")],
                args=["--host", "::1"]))
    sc.append(S("banner with everything", [R("GET", "/api/state")],
                args=["--model", "m2", "--base-url", "{H}/v9", "--session-dir", "rel/sessions", "--allow-shell",
                      "--allow-write", "--auto-approve", "--append-system-prompt", "be brief", "--add-dir", "{W}/empty",
                      "--add-dir={W}/home", "--escalate-to-claude-code", "--claude-code-cli-path", "/nonexistent/claude",
                      "--max-turns", "3", "--stream=false", "--timeout-seconds", "7.5", "--temperature", "0.25",
                      "--cwd", "{W}/ws/sub"],
                env={"OPENAI_API_KEY": KEY}, files=state(**{"ws/sub/": ""})))
    sc.append(S("api key by flag", [R("GET", "/api/state")], args=["--api-key", "sk-flag-key-1234567"],
                env={"OPENAI_API_KEY": KEY}))
    sc.append(S("only write", [R("GET", "/api/state")], args=["--allow-write"]))
    sc.append(S("only shell", [R("GET", "/api/state")], args=["--allow-shell", "--cwd", "{W}/link"],
                files=state(**{"link@": "{W}/empty"})))
    sc.append(S("started as sarina-gui", [R("GET", "/api/state")], argv0="sarina-gui"))
    sc.append(S("browser stand-in", [("sleep", 0.8)], browser=True,
                files=state(**{"bin/xdg-open*": "#!/usr/bin/bash\nprintf '%s\\n' \"$@\" > {W}/opened\n"})))
    sc.append(S("browser missing", [R("GET", "/api/state")], browser=True))
    sc.append(S("contained", [], files=dict(state(), **{"lock": '{"reason": "a made-up lock"}'}),
                env={"SARINA_CONTAINED_PATH": "{W}/lock"}))
    sc.append(S("SIGINT", [R("GET", "/api/state"), ("signal", signal.SIGINT)]))
    sc.append(S("SIGTERM with an open event stream", [BG(events(wait=2)), ("sleep", 0.5), ("signal", signal.SIGTERM),
                                                      JOIN]))
    for tz in ("Pacific/Kiritimati", "Etc/GMT+12"):
        sc.append(S(f"time zone {tz}", [chat("what day is it")], env={"TZ": tz}, model=[completion("today")]))
    return sc


def page_scenarios():
    sc = []
    p = "127.0.0.1:{PORT}"
    sc.append(S("the page and its token", [
        R("GET", "/", token=None), R("GET", "/?t=wrong", token=None), R("GET", "/?t={TOKEN}", token=None),
        R("HEAD", "/?t={TOKEN}", token=None), R("GET", "/?x=1&t={TOKEN}&t=other", token=None),
        R("GET", "/?t=other&t={TOKEN}", token=None), R("GET", "/?t={TOKEN};x=1", token=None),
        R("GET", "/?t=%7BTOKEN", token=None), R("GET", "/", token=None, headers={"Cookie": "sarina_gui={TOKEN}"}),
        R("GET", "/", token=None, headers={"Cookie": "a=1; sarina_gui=\"{TOKEN}\"; b=2"}),
        R("GET", "/", token=None, headers={"Cookie": "sarina_gui=wrong; sarina_gui={TOKEN}"}),
        R("GET", "/", token=None, headers={"Cookie": "sarina_gui=bad value; sarina_gui={TOKEN}"}),
        R("GET", "/", token=None, headers={"Cookie": "sarina_gui="}),
        R("GET", "/", token=None, headers={"Cookie": "SARINA_GUI={TOKEN}"}),
        R("POST", "/?t={TOKEN}", token=None), R("GET", "/index.html", token=None), R("GET", "/?t={TOKEN}", token="x"),
        R("GET", "//?t={TOKEN}", token=None), R("GET", "/./?t={TOKEN}", token=None),
        R("GET", "/nothing", token=None), R("GET", "/api", token=None), R("GET", "/api/", token=None),
        R("GET", "/api/nothing"), R("GET", "/api/nothing", token=None)]))
    sc.append(S("the static files", [
        R("GET", "/static/app.css", token=None), R("GET", "/static/app.js", token=None),
        R("HEAD", "/static/app.css", token=None), R("GET", "/static/", token=None), R("GET", "/static", token=None),
        R("GET", "/static?x=1", token=None), R("GET", "/static/index.html", token=None),
        R("GET", "/static/index.html?x=1", token=None), R("GET", "/static/nope.css", token=None),
        R("GET", "/static/app.css/", token=None), R("GET", "/static/sub/", token=None),
        R("GET", "/static/../api/state", token=None), R("GET", "/static/%2e%2e/api/state", token=None),
        R("GET", "/static/app%2ecss", token=None), R("GET", "/static//app.css", token=None),
        R("POST", "/static/app.css", token=None), R("PUT", "/static/app.css", "{}", token=None),
        R("GET", "/static/app.css", token=None, headers={"Range": "bytes=0-9"}),
        R("GET", "/static/app.css", token=None, headers={"Range": "bytes=-5"}),
        R("GET", "/static/app.css", token=None, headers={"Range": "bytes=13720-"}),
        R("GET", "/static/app.css", token=None, headers={"Range": "bytes=0-1,4-5"}),
        R("GET", "/static/app.css", token=None, headers={"Range": "bytes=0-1, 1-2"}),
        R("GET", "/static/app.css", token=None, headers={"Range": "bytes=999999-"}),
        R("GET", "/static/app.css", token=None, headers={"Range": "bytes=x"}),
        R("GET", "/static/app.css", token=None, headers={"Range": "items=0-1"}),
        R("GET", "/static/app.css", token=None, headers={"Range": "bytes=5-2"}),
        R("GET", "/static/app.css", token=None, headers={"Range": "bytes=0-0", "If-Range": "\"x\""}),
        R("GET", "/static/app.css", token=None, headers={"Range": "bytes=0-0", "If-Range": "Mon, 02 Jan 2006 15:04:05 GMT"}),
        R("GET", "/static/app.css", token=None, headers={"If-None-Match": "*"}),
        R("HEAD", "/static/app.css", token=None, headers={"If-None-Match": "*"}),
        R("GET", "/static/app.css", token=None, headers={"If-None-Match": "\"abc\""}),
        R("GET", "/static/app.css", token=None, headers={"If-Match": "\"abc\""}),
        R("GET", "/static/app.css", token=None, headers={"If-Match": "*"}),
        R("GET", "/static/app.css", token=None, headers={"If-Modified-Since": "Mon, 02 Jan 2006 15:04:05 GMT"}),
        R("GET", "/static/app.css", token=None, headers={"If-Unmodified-Since": "Mon, 02 Jan 2006 15:04:05 GMT"}),
        R("GET", "/static/app.css", token=None, headers={"Range": "bytes=0-3"}, proto="HTTP/1.0"),
        R("GET", "/static/app.css", token=None, host="evil.example"),
        R("GET", "/static/app.css", token=None, headers={"Sec-Fetch-Site": "cross-site"})]))
    sc.append(S("the guard", [
        R("GET", "/api/state", host="evil.example:{PORT}"), R("GET", "/api/state", host="localhost:{PORT}"),
        R("GET", "/api/state", host="LOCALHOST"), R("GET", "/api/state", host="[::1]:{PORT}"),
        R("GET", "/api/state", host="127.1.2.3"), R("GET", "/api/state", host="::1"),
        R("GET", "/api/state", host="localhost.:{PORT}"), R("GET", "/api/state", host=None, proto="HTTP/1.0"),
        R("GET", "/api/state", headers={"Sec-Fetch-Site": "cross-site"}),
        R("GET", "/api/state", headers={"Sec-Fetch-Site": "same-site"}),
        R("GET", "/api/state", headers={"Sec-Fetch-Site": "same-origin"}),
        R("GET", "/api/state", headers={"Sec-Fetch-Site": "none"}),
        R("GET", "/api/state", headers={"Origin": "http://" + p}),
        R("GET", "/api/state", headers={"Origin": "http://localhost:{PORT}"}),
        R("GET", "/api/state", headers={"Origin": "https://" + p}),
        R("GET", "/api/state", headers={"Origin": "null"}),
        R("GET", "/api/state", headers={"Origin": "http://[::1"}),
        R("GET", "/api/state", headers={"Origin": "http://" + p + "/path"}),
        R("GET", "/api/state", headers={"Origin": "HTTP://" + p}),
        R("GET", "/", token=None, headers={"Origin": "http://evil.example"}),
        R("GET", "/api/state", token=None), R("GET", "/api/state", token="wrong"),
        R("GET", "/api/state", token="{TOKEN}x"), R("GET", "/api/state", token=" {TOKEN}"),
        R("GET", "/api/state", token=None, headers={"x-sarina-token": "{TOKEN}"}),
        R("GET", "/api/state", token=None, headers={"Authorization": "Bearer {TOKEN}"}),
        R("POST", "/api/state", "{}", ctype=None), R("POST", "/api/state", "{}", ctype="text/plain"),
        R("POST", "/api/state", "{}", ctype="application/json; charset=utf-8"),
        R("POST", "/api/state", "{}", ctype="Application/JSON"),
        R("POST", "/api/state", "{}", ctype="application/json; charset"),
        R("POST", "/api/state", "{}", ctype="application/json;"),
        R("POST", "/api/state", "{}", ctype="application/json; a=1; a=2"),
        R("POST", "/api/state", "{}", ctype="application/json; A=1; a=2"),
        R("POST", "/api/state", "{}", ctype=" application/json "),
        R("POST", "/api/state", "{}", ctype="application/json, text/plain"),
        R("POST", "/api/state", "{}", ctype="application/json; q=\"a\\\"b\""),
        R("POST", "/api/state", "{}", ctype="application/jsonx"), R("POST", "/api/state", "{}", ctype="application/"),
        R("DELETE", "/api/sessions/s1", ctype=None), R("DELETE", "/api/sessions/zz", "", ctype="application/json"),
        R("HEAD", "/api/state"), R("PUT", "/api/state", "{}"), R("PATCH", "/api/state", "{}"),
        R("OPTIONS", "/api/state"), ("raw", b"OPTIONS * HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
        ("raw", b"GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\nConnection: close\r\n\r\n"),
        ("conn", [b"GET /api/run HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\n\r\n",
                  b"GET /api/plan HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\n\r\n"]),
        ("raw", b"GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nHost: evil\r\n\r\n"),
        ("raw", b"GET http://evil.example/api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\n\r\n"),
        ("raw", b"BREW /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\nContent-Type: application/json\r\n\r\n"),
        ("raw", b"GET /api/sessions/a%2Fb HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\n\r\n"),
        ("raw", b"GET /api/sessions/%zz HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\n\r\n"),
        ("raw", b"GET /api/sessions/ HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\n\r\n"),
        ("raw", b"GET /api/sessions/s1/ HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\n\r\n"),
        ("raw", b"GET /api/state/ HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\n\r\n"),
        ("raw", b"GET /api//state HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\n\r\n"),
        ("raw", b"GET /api/x/../state?a=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\n\r\n"),
        ("raw", b"CONNECT 127.0.0.1:1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
        ("raw", b"GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\nExpect: nothing\r\n\r\n"),
        ("raw", b"POST /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\nContent-Type: application/json\r\n"
                b"Transfer-Encoding: chunked\r\n\r\n2;x=y\r\n{}\r\n0\r\nT: v\r\n\r\n"),
        ("raw", b"POST /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {TOKEN}\r\nContent-Type: application/json\r\n"
                b"Content-Length: 17\r\nExpect: 100-continue\r\n\r\n{\"model\": \"m-x\"}"),
        ("big", 9 << 20, 9 << 20, "/api/state"), ("big", 300 << 10, 300 << 10, "/api/state"),
        R("GET", "/api/state")]))
    return sc


def state_scenarios():
    sc = []
    sc.append(S("state patches", [
        R("GET", "/api/state"),
        post_state({"model": "  other-model  ", "base_url": " {H}/v2 ", "api_key": "sk-patched-key-98765"}),
        post_state({"model": ""}), post_state({"model": "   "}), post_state({"model": 5}), post_state({"model": None}),
        post_state({"base_url": ""}), post_state({"api_key": None}), post_state({"api_key": ""}), post_state({"api_key": 5}),
        post_state({"cwd": "{W}/empty"}), post_state({"cwd": "empty"}), post_state({"cwd": "  {W}/ws  "}),
        post_state({"cwd": "~"}), post_state({"cwd": "~/"}), post_state({"cwd": "~nobody"}), post_state({"cwd": "{W}/nowhere"}),
        post_state({"cwd": "{W}/ws/notes.txt"}), post_state({"cwd": ""}), post_state({"cwd": None}), post_state({"cwd": 3}),
        post_state({"cwd": "{W}/link"}),
        post_state({"session_directory": "rel/sessions"}), post_state({"cwd": "{W}/ws"}),
        post_state({"session_directory": "  "}), post_state({"session_directory": "{W}/s2"}),
        post_state({"session_directory": None}), post_state({"session_directory": 5}),
        post_state({"allow_shell": True}), post_state({"allow_write": True}), post_state({"allow_write": "yes"}),
        post_state({"allow_write": True, "confirm_permissions": "yes"}),
        post_state({"allow_write": True, "confirm_permissions": True}), post_state({"allow_shell": False}),
        post_state({"approval_mode": "auto"}), post_state({"approval_mode": "auto", "confirm_permissions": True}),
        post_state({"approval_mode": "never"}), post_state({"approval_mode": 1}), post_state({"approval_mode": "ask"}),
        post_state({"allow_write": False}), post_state({"approval_mode": "auto"}),
        post_state({"allow_shell": True, "confirm_permissions": True}), post_state({"allow_shell": False, "allow_write": False}),
        post_state({"temperature": -1}), post_state({"temperature": "x"}), post_state({"temperature": None}),
        post_state({"temperature": 0.7}), post_state({"temperature": 1e300}),
        R("POST", "/api/state", '{"temperature": 1e400}'),
        post_state({"timeout_seconds": 0}), post_state({"timeout_seconds": 2.5}), post_state({"timeout_seconds": None}),
        post_state({"max_turns": 0}), post_state({"max_turns": 1.5}), post_state({"max_turns": 1e7}),
        post_state({"max_turns": 1000000}), post_state({"max_turns": 3}), post_state({"max_turns": "3"}),
        post_state({"stream_model_responses": "no"}), post_state({"stream_model_responses": False}),
        post_state({"max_total_tokens": 0}), post_state({"max_total_tokens": 1.5}), post_state({"max_total_tokens": 1e13}),
        post_state({"max_total_tokens": "5"}), post_state({"max_total_tokens": -2}),
        post_state({"max_total_tokens": 1000, "max_input_tokens": 900, "max_output_tokens": 800, "max_reasoning_tokens": 700,
                    "max_tool_calls": 6, "max_delegated_tasks": 5, "max_model_calls": 4, "max_session_turns": 3,
                    "auto_snip_threshold_tokens": 2000, "auto_compact_threshold_tokens": 3000, "max_total_cost_usd": 0.5}),
        post_state({"max_total_tokens": None, "max_tool_calls": None}), post_state({"max_total_cost_usd": 0}),
        post_state({"max_total_cost_usd": "1"}), post_state({"max_total_cost_usd": None}),
        post_state({"system_prompt": "sys", "append_system_prompt": "app", "override_system_prompt": ""}),
        post_state({"system_prompt": None}), post_state({"append_system_prompt": 5}),
        post_state({"response_schema": {"type": "object", "properties": {"a": {"type": "string"}}}}),
        post_state({"response_schema": []}), post_state({"response_schema": "x"}), post_state({"response_schema": None}),
        post_state({"response_schema_name": "  answer "}), post_state({"response_schema_name": ""}),
        post_state({"response_schema_strict": True}), post_state({"response_schema_strict": 1}),
        post_state({"compact_preserve_messages": -1}), post_state({"compact_preserve_messages": 2.5}),
        post_state({"compact_preserve_messages": 0}), post_state({"disable_claude_md_discovery": True}),
        post_state({"additional_working_directories": ["{W}/empty", " ", "{W}/home"]}),
        post_state({"additional_working_directories": ["{W}/nowhere"]}),
        post_state({"additional_working_directories": "x"}), post_state({"additional_working_directories": [5]}),
        post_state({"additional_working_directories": None}), post_state({"additional_working_directories": []}),
        post_state({"unknown_key": 1, "active_session_id": "s1", "busy": True}),
        R("POST", "/api/state", "[]"), R("POST", "/api/state", "null"), R("POST", "/api/state", ""),
        R("POST", "/api/state", "  "), R("POST", "/api/state", "{bad"), R("POST", "/api/state", '{} x'),
        R("POST", "/api/state", '"x"'), R("POST", "/api/state", '{"model": "a", "model": "b"}'),
        R("POST", "/api/state", '{"Model": "upper"}'), R("POST", "/api/state", '{"model": "caf\\u00e9 <b>&"}'),
        R("POST", "/api/state", b'{"model": "bad \xff byte"}'),
        R("POST", "/api/state", '{"model": "x\\ud800y"}'),
        R("POST", "/api/clear"), R("POST", "/api/clear", "not json"), R("GET", "/api/state")],
        files=state(**{"ws/notes.txt": "x", "link@": "{W}/empty"}), env={"OPENAI_API_KEY": KEY}))
    return sc


def run_scenarios():
    sc = []
    sc.append(S("a run answered as JSON", [
        chat("hello there"), ("sleep", 0.3), R("GET", "/api/state"), R("GET", "/api/run"),
        events(), events(after=2), R("GET", "/api/sessions"), R("GET", "/api/sessions/{S0}"),
        chat("and again"), ("sleep", 0.3), R("GET", "/api/sessions/{S0}"), R("POST", "/api/clear"),
        chat("fresh start"), ("sleep", 0.3), R("GET", "/api/sessions"), R("GET", "/api/run")],
        model=[completion("Hello!"), completion("Hello again."), completion("New session.")]))
    sc.append(S("a run as an event stream", [
        chat_stream("stream it"), R("GET", "/api/run"), events(after=3),
        chat_stream({"prompt": "with tools"}), events(after=0)],
        files=state(**{"ws/notes.txt": "the widget is blue\n"}),
        model=[sse_completion(["Hel", "lo ", "there"]),
               sse_calls([("read_file", {"path": "notes.txt"}), ("list_dir", {"path": "."})]),
               sse_completion(["The widget ", "is blue."])], stream=True))
    sc.append(S("chat checks", [
        chat({}), chat({"prompt": ""}), chat({"prompt": "   "}), chat({"prompt": 5}), chat({"prompt": None}),
        chat({"prompt": "x", "resume_session_id": 5}), chat({"prompt": "x", "pasted_contents": []}),
        chat({"prompt": "x", "pasted_contents": {"1": "y"}}), R("POST", "/api/chat", "{"), R("POST", "/api/chat", "[]"),
        chat({"prompt": "hi", "resume_session_id": "../etc"}), chat({"prompt": "hi", "resume_session_id": "nope"}),
        chat({"prompt": "hi", "resume_session_id": "x" * 65}), chat({"prompt": "hi", "resume_session_id": ""}),
        chat("/help"), chat("  /HELP me  "), chat("/plan"), chat({"prompt": "/help", "resume_session_id": "s2"}),
        ("sleep", 0.2), R("GET", "/api/run"), R("GET", "/api/state"),
        chat({"prompt": "[Pasted text #1 +2 lines] and [Pasted text #2] and [Pasted text #3 x]",
              "pasted_contents": {"1": {"type": "text", "content": "first\nsecond"}, "2": {"content": ""},
                                  "3": {"type": "text", "content": "third"}}}),
        chat({"prompt": "[Pasted text #1]", "pasted_contents": {"1": {"content": "   "}}}),
        chat({"prompt": "hi", "resume_session_id": "s1"}), ("sleep", 0.3), R("GET", "/api/state"),
        R("GET", "/api/chat"), R("POST", "/api/chat/cancel"), R("POST", "/api/chat/cancel", "x")],
        model=[completion("Pasted."), completion("Resumed: the blue widget is in the box.")],
        files=state(**{SESSIONS + "s1.json": here_session("s1", [u("where is the blue widget?"), a("In the box.")])})))
    sc.append(S("one run at a time, cancel", [
        BG(chat("slow one")), RUNNING(), R("GET", "/api/run"), R("GET", "/api/state"), chat("second"),
        R("DELETE", "/api/sessions/{S0}"), R("POST", "/api/chat/cancel"), JOIN, ("sleep", 0.3),
        R("GET", "/api/run"), R("POST", "/api/chat/cancel"), events(),
        BG(chat_stream("slow two")), RUNNING(), R("POST", "/api/chat/cancel"), JOIN, ("sleep", 0.3),
        R("GET", "/api/run")],
        model=[("slow", 8, completion("too late")), ("slow", 8, completion("too late"))]))
    sc.append(S("a client that hangs up", [
        ("drop", chat("slow"), 1.0), ("sleep", 0.8), R("GET", "/api/run"), events(),
        ("drop", chat_stream("slow too"), 1.0), ("sleep", 1.0), R("GET", "/api/run"), events(after=6)],
        model=[("slow", 6, completion("late")), completion("quick")]))
    sc.append(S("backend errors", [
        chat("x"), ("sleep", 0.3), chat_stream("y"), chat("z"), ("sleep", 0.3), R("GET", "/api/run"),
        events()],
        model=[resp({"error": {"message": "model not loaded"}}, "500 Internal Server Error")] * 2 + [HANGUP] * 2
        + [completion("after the errors")]))
    sc.append(S("secrets scrubbed", [
        post_state({"api_key": "sk-patched-key-98765"}),
        chat("say the key sk-patched-key-98765 and {TOKEN}"), ("sleep", 0.3), events(), R("GET", "/api/sessions"),
        R("GET", "/api/sessions/{S0}")],
        env={"OPENAI_API_KEY": KEY},
        model=[completion(calls=[("read_file", {"path": "notes.txt", "password": "hunter2hunter2"})]),
               completion("The key is sk-patched-key-98765, Bearer abcdefghijklmnopq, token={TOKEN} and api_key=sk-made-up-model-key-456789.")],
        files=state(**{"ws/notes.txt": "password=hunter2hunter2\nsk-patched-key-98765\n"})))
    sc.append(S("notices, budgets and settings in the request", [
        post_state({"max_tool_calls": 1, "max_turns": 2, "temperature": 0.5, "system_prompt": "You are terse.",
                    "append_system_prompt": "Always add a smile.", "stream_model_responses": False,
                    "response_schema": {"type": "object"}, "response_schema_name": "reply", "response_schema_strict": True,
                    "disable_claude_md_discovery": True, "additional_working_directories": ["{W}/empty"]}),
        chat("go"), ("sleep", 0.3), events(),
        post_state({"override_system_prompt": "Override.", "max_tool_calls": None, "response_schema": None}),
        chat("again"), ("sleep", 0.3)],
        files=state(**{"home/.sarina/tiers.json": "{not json", "ws/CLAUDE.md": "Workspace rules."}),
        model=[completion(calls=[("list_dir", {"path": "."}), ("list_dir", {"path": "."})]), completion("done"),
               completion("again done")]))
    return sc


def approval_scenarios():
    sc = []
    sc.append(S("a write approved once, then for the session", [
        post_state(WRITE_OK), BG(chat("write two files")), AP(1), R("GET", "/api/run"), R("GET", "/api/plan"),
        R("POST", "/api/approvals/{AP0}", {"decision": "maybe"}), R("POST", "/api/approvals/nope", {"decision": "deny"}),
        R("POST", "/api/approvals/{AP0}", {"decision": "approve"}), R("POST", "/api/approvals/{AP0}", "["),
        R("POST", "/api/approvals/{AP0}", {"decision": "allow_once"}), AP(1),
        R("POST", "/api/approvals/{AP1}", {"decision": "allow_session"}), JOIN, ("sleep", 0.3),
        R("POST", "/api/approvals/{AP1}", {"decision": "deny"}), R("GET", "/api/approvals"), events(),
        R("GET", "/api/file-history", ties=True)],
        model=[completion(calls=[("write_file", {"path": "a.txt", "content": "one\n"})]),
               completion(calls=[("write_file", {"path": "b.txt", "content": "two\n"})], first_id=1),
               completion(calls=[("write_file", {"path": "c.txt", "content": "three\n"})], first_id=2),
               completion("Wrote three files.")]))
    sc.append(S("a shell command denied, an approval cancelled", [
        post_state(SHELL_OK), BG(chat_stream("clean up")), AP(1),
        R("POST", "/api/approvals/{AP0}", {"decision": "deny", "reason": "not now"}), JOIN, ("sleep", 0.3),
        BG(chat("run it")), AP(1), R("GET", "/api/run"), R("POST", "/api/chat/cancel"), JOIN, ("sleep", 0.3),
        R("GET", "/api/run"), events(after=5)],
        model=[completion(calls=[("bash", {"command": "rm -rf build && echo api_key=sk-abcdefghijk"})]),
               completion("Understood."),
               completion(calls=[("bash", {"command": "echo hi"})]), completion("unused")]))
    sc.append(S("auto-approve", [
        post_state({"allow_write": True, "allow_shell": True, "approval_mode": "auto", "confirm_permissions": True}),
        chat("write and run"), ("sleep", 0.3), events()],
        model=[completion(calls=[("write_file", {"path": "out.txt", "content": "made by the model\n"}),
                                 ("bash", {"command": "cat out.txt"})]), completion("Wrote and ran.")]))
    sc.append(S("a plan approved, revised, rejected", [
        BG(chat("plan it")), AP(1), R("GET", "/api/plan"), R("GET", "/api/approvals"),
        R("POST", "/api/approvals/{AP0}", {"decision": "allow_once"}),
        R("POST", "/api/approvals/{AP0}", {"decision": "revise"}),
        R("POST", "/api/approvals/{AP0}", {"decision": "revise", "reason": "   "}),
        R("POST", "/api/approvals/{AP0}", {"decision": "revise", "reason": "smaller steps"}), AP(1),
        R("POST", "/api/approvals/{AP1}", {"decision": "approve"}), JOIN, ("sleep", 0.3), R("GET", "/api/plan"),
        BG(chat("plan again")), AP(1), R("POST", "/api/approvals/{AP2}", {"decision": "reject", "reason": "no"}), JOIN,
        ("sleep", 0.3), R("GET", "/api/plan"), R("GET", "/api/sessions/{S0}"), events()],
        model=[completion(calls=[("enter_plan_mode", {})]),
               completion(calls=[("exit_plan_mode", {"plan": "1. read\n2. write"})], first_id=1),
               completion(calls=[("exit_plan_mode", {"plan": "1a. read half"})], first_id=2), completion("Proceeding."),
               completion(calls=[("enter_plan_mode", {})]), completion(calls=[("exit_plan_mode", {"plan": "1. again"})], first_id=1),
               completion("Rejected, stopping.")]))
    return sc


TASKS = json.dumps({"tasks": [{"task_id": "t1", "title": "read", "status": "completed"},
                              {"task_id": "t2", "title": "write token=abcdef123456", "status": "pending"},
                              {"task_id": "t3", "title": "x", "status": "pending"}, {"task_id": "t4"}]})
FH = [{"path": "{W}/ws/a.txt", "action": "write", "timestamp": "2026-01-02T03:04:05Z", "note": "password=hunter2hunter2"},
      {"path": "{W}/ws/b.txt", "action": "edit", "timestamp": "2026-01-03T03:04:05Z"},
      {"path": "{W}/ws/c.txt", "action": "write"}]


def api_scenarios():
    sc = []
    files = state(**{
        SESSIONS + "s1.json": here_session("s1", [u("where is the blue widget?"), a("The blue widget is in the box.")],
                                           file_history=FH[:2]),
        SESSIONS + "s3.json": here_session("s3", [u("<system-reminder>x</system-reminder>"), u("  Many   spaces\tand\nlines " * 20),
                                                  a("ok")], file_history=FH[2:],
                                           plugin_state={"plan_mode": {"mode": "plan", "pending": "1. a\n2. b token=abcdef123456"}}),
        SESSIONS + "bad.json": "{not json",
        SESSIONS + "a b.json": here_session("a b", [u("bad name")]),
        SESSIONS + "dir.json/": "",
        SESSIONS + "notes.txt": "x",
        SESSIONS + "s1.notes.md": "notes",
        "ws/.port_sessions/task_runtime.json": TASKS,
    })
    sc.append(S("sessions, history, tasks, plan", [
        R("GET", "/api/sessions"), R("GET", "/api/sessions?q=widget"), R("GET", "/api/sessions?q=BLUE"),
        R("GET", "/api/sessions?q=s3"), R("GET", "/api/sessions?q=%20many%20%20spaces"), R("GET", "/api/sessions?q=nothing"),
        R("GET", "/api/sessions/s1"), R("GET", "/api/sessions/s3"), R("GET", "/api/sessions/bad"),
        R("GET", "/api/sessions/nope"), R("GET", "/api/sessions/a%20b"), R("GET", "/api/sessions/" + "x" * 65),
        R("GET", "/api/file-history"), R("GET", "/api/file-history?limit=1"), R("GET", "/api/file-history?limit=0"),
        R("GET", "/api/file-history?limit=x"), R("GET", "/api/file-history?limit=6000"), R("GET", "/api/file-history?limit=2"),
        R("GET", "/api/slash-commands"), R("GET", "/api/tasks"), R("GET", "/api/plan"),
        post_state({"session_directory": "{W}/nowhere"}), R("GET", "/api/sessions"), R("GET", "/api/file-history"),
        post_state({"session_directory": None}),
        chat({"prompt": "/help", "resume_session_id": "s3"}), R("GET", "/api/plan"),
        R("DELETE", "/api/sessions/s1", ""), R("DELETE", "/api/sessions/s1", ""), R("DELETE", "/api/sessions/..", ""),
        R("DELETE", "/api/sessions/dir", ""), R("GET", "/api/sessions"), R("GET", "/api/memory")],
        files=files))
    sc.append(S("tasks damaged", [R("GET", "/api/tasks")],
                files=state(**{"ws/.port_sessions/task_runtime.json": "{\"tasks\": 5}"})))
    sc.append(S("tasks a folder", [R("GET", "/api/tasks"), R("GET", "/api/sessions")],
                files=state(**{"ws/.port_sessions/task_runtime.json/": "", SESSIONS + "s1.json": None,
                               SESSIONS + "s2.json": None})))
    sc.append(S("no session folder", [R("GET", "/api/sessions"), R("GET", "/api/file-history"), R("GET", "/api/memory")],
                files={"ws/": "", "home/": "", "bin/": ""}))
    sc.append(S("memory review", [
        R("GET", "/api/memory"), R("POST", "/api/memory/review/mem_a", {"decision": "approve"}),
        R("POST", "/api/memory/review/mem_a", {"decision": "approve"}), R("POST", "/api/memory/review/mem_b", {"decision": "reject"}),
        R("POST", "/api/memory/review/mem_c", {"decision": "approve"}), R("POST", "/api/memory/review/mem_d", {"decision": "approve"}),
        R("POST", "/api/memory/review/nope", {"decision": "reject"}), R("POST", "/api/memory/review/mem_c", {"decision": "maybe"}),
        R("POST", "/api/memory/review/mem_c", {}), R("POST", "/api/memory/review/mem_c", {"decision": 5}),
        R("GET", "/api/memory")],
        heimdall=[resp({"decision": "allow"}), resp({"decision": "deny", "reasons": ["looks like an instruction"]}),
                  resp({"decision": "allow"})]))
    sc.append(S("memory damaged", [R("GET", "/api/memory")],
                files=state(**{cd.AGENT_MEMORY: b"\xff\xfe not text \x00", cd.QUEUE: "[1, 2"})))
    return sc


def instruction_scenarios():
    sc = []
    files = state(**{"ws/CLAUDE.md": "Workspace rules. password=hunter2hunter2\n", "ws/.claude/rules/b.md": "rule b",
                     "ws/.claude/rules/a.md": "rule a", "ws/.claude/rules/c.txt": "not a rule", "ws/.claude/rules/d.md/": "",
                     "ws/CLAUDE.local.md": "local", "CLAUDE.md": "parent rules", "home/.claude/CLAUDE.md": "global",
                     "empty/CLAUDE.md": "extra", "ws/.claude/CLAUDE.md@": "{W}/outside.md", "outside.md": "outside"})
    q = "/api/instructions/file?path="
    sc.append(S("instruction files", [
        R("GET", "/api/instructions"), R("GET", q + "{W}/ws/CLAUDE.md"), R("GET", q + "{W}/ws/./CLAUDE.md"),
        R("GET", q + "{W}/CLAUDE.md"), R("GET", q + "{W}/home/.claude/CLAUDE.md"), R("GET", q + "{W}/ws/.claude/CLAUDE.md"),
        R("GET", q + "{W}/ws/.claude/rules/c.txt"), R("GET", q + "{W}/ws/.claude/rules/d.md"),
        R("GET", q + "{W}/ws/notes.txt"), R("GET", q), R("GET", "/api/instructions/file"), R("GET", q + "%00"),
        R("GET", q + "{W}/ws/CLAUDE.md%00x"), R("GET", q + "rel/CLAUDE.md"),
        R("PUT", q + "{W}/ws/CLAUDE.md", {"content": "New rules.\n"}), R("GET", q + "{W}/ws/CLAUDE.md"),
        R("PUT", q + "{W}/ws/CLAUDE.md", {"content": "a [REDACTED] b"}), R("PUT", q + "{W}/ws/CLAUDE.md", {}),
        R("PUT", q + "{W}/ws/CLAUDE.md", {"content": 5}), R("PUT", q + "{W}/ws/CLAUDE.md", {"content": None}),
        R("PUT", q + "{W}/ws/CLAUDE.md", {"content": "x" * (200 << 10)}),
        R("PUT", q + "{W}/ws/CLAUDE.md", {"content": "x" * ((200 << 10) + 1)}),
        R("PUT", q + "{W}/CLAUDE.md", {"content": "no"}), R("PUT", q + "{W}/ws/.claude/CLAUDE.md", {"content": "link"}),
        R("PUT", q + "{W}/ws/.claude/rules/new-rule.md", {"content": "new rule"}),
        R("PUT", q + "{W}/ws/.claude/rules/.hidden.md", {"content": "no"}),
        R("PUT", q + "{W}/ws/.claude/rules/bad name.md", {"content": "no"}),
        R("PUT", q + "{W}/ws/.claude/rules/sub/x.md", {"content": "no"}),
        R("PUT", q + "{W}/empty/.claude/rules/x.md", {"content": "extra rule"}),
        R("PUT", q + "{W}/home/.claude/CLAUDE.md", {"content": "new global"}),
        R("PUT", q + "{W}/ws/CLAUDE.local.md", "not json"),
        R("PUT", q + "{W}/ws/CLAUDE.md", {"content": "x"}, ctype="text/plain"),
        R("GET", "/api/instructions"),
        R("DELETE", q + "{W}/ws/.claude/rules/new-rule.md", ""), R("DELETE", q + "{W}/ws/.claude/rules/new-rule.md", ""),
        R("DELETE", q + "{W}/CLAUDE.md", ""), R("DELETE", q + "{W}/ws/.claude/CLAUDE.md", ""),
        R("DELETE", q + "{W}/ws/.claude/rules/zz.md", ""), R("GET", "/api/instructions")],
        args=["--add-dir", "{W}/empty"], files=files))
    big = "x" * (400 << 10) + "password=hunter2hunter2 and the end\n"
    sc.append(S("planted pipes, folders and a long file", [
        R("GET", "/api/tasks"), R("GET", "/api/instructions"), R("GET", q + "{W}/ws/CLAUDE.md"),
        R("GET", q + "{W}/ws/CLAUDE.local.md"), R("GET", q + "{W}/ws/.claude/CLAUDE.md"),
        R("DELETE", q + "{W}/ws/.claude/CLAUDE.md", ""), R("DELETE", q + "{W}/ws/CLAUDE.md", ""),
        R("GET", "/api/instructions")],
        files=state(**{"ws/.port_sessions/task_runtime.json|": "", "ws/CLAUDE.md|": "", "ws/CLAUDE.local.md": big,
                       "ws/.claude/CLAUDE.md/": ""})))
    sc.append(S("no instruction files", [R("GET", "/api/instructions"),
                                         R("PUT", "/api/instructions/file?path={W}/ws/CLAUDE.md", {"content": "first"}),
                                         R("GET", "/api/instructions")]))
    return sc


TIERS2 = json.dumps({"tiers": [{"name": "local", "kind": "openai", "base_url": "{H}/v1", "model": "made-up-model"},
                               {"name": "big", "kind": "openai", "base_url": "{H}/big", "model": "big-model",
                                "api_key_env": "BIG_KEY", "allow_data": True}]})


def backend_scenarios():
    sc = []
    sc.append(S("tiers and the backend probe", [
        R("GET", "/api/tiers"), R("GET", "/api/backend"),
        post_state({"base_url": "{H}/probe/v1", "api_key": "sk-probe-key-7654321"}), R("GET", "/api/backend"),
        R("GET", "/api/backend"), R("GET", "/api/backend"), R("GET", "/api/backend"),
        post_state({"base_url": "http://127.0.0.1:1/v1"}), R("GET", "/api/backend"),
        post_state({"base_url": "ftp://127.0.0.1/v1"}), R("GET", "/api/backend"),
        post_state({"base_url": "http://[::1"}), R("GET", "/api/backend"),
        post_state({"base_url": "{H}/probe/"}), R("GET", "/api/backend")],
        probe=[resp({"data": [{"id": "m"}]}), resp("down", "503 Service Unavailable", ctype="text/plain"), HANGUP,
               resp("x", "204 No Content", ctype=None), resp("[]")]))
    sc.append(S("the backend probe times out", [post_state({"base_url": "{H}/probe"}), R("GET", "/api/backend")],
                probe=[("slow", 6, resp("late"))]))
    sc.append(S("tiers configured", [R("GET", "/api/tiers")],
                files=state(**{"home/.sarina/tiers.json": TIERS2}), env={"BIG_KEY": "sk-big-key-11112222"}))
    sc.append(S("tiers damaged", [R("GET", "/api/tiers")], files=state(**{"home/.sarina/tiers.json": "{not json"})))
    sc.append(S("tiers with escalation", [R("GET", "/api/tiers"), post_state(WRITE_OK), R("GET", "/api/tiers")],
                args=["--escalate-to-claude-code", "--claude-code-cli-path", "/nonexistent/claude"]))
    return sc


def slow_scenarios():
    """Each waits out a deadline: the 30 s body deadline, the event stream's 15 s keep-alive."""
    return [
        S("a body that never arrives", [("big", 100, 10, "/api/state"), R("GET", "/api/state")]),
        S("the keep-alive comment", [events(wait=16.5, until=": keep-alive")]),
    ]


def scenarios():
    return (cli_scenarios() + page_scenarios() + state_scenarios() + run_scenarios() + approval_scenarios()
            + api_scenarios() + instruction_scenarios() + backend_scenarios() + slow_scenarios())


# ---- talking to the console --------------------------------------------------------------------------------------------

def request(method, path, body, headers, host, proto, ctype):
    h = []
    if host is not None:
        h.append(("Host", host))
    if body is not None:
        b = body if isinstance(body, bytes) else body.encode("utf-8", "surrogateescape")
        if ctype is not None:
            h.append(("Content-Type", ctype))
        h.append(("Content-Length", str(len(b))))
    else:
        b = b""
        if ctype is not None and method not in ("GET", "HEAD"):
            h.append(("Content-Type", ctype))
    for k, v in (headers or {}).items():
        h.append((k, v))
    head = f"{method} {path} {proto}\r\n" + "".join(f"{k}: {v}\r\n" for k, v in h) + "\r\n"
    return head.encode("utf-8", "surrogateescape") + b


def dechunk(body):
    out, rest = b"", body
    while rest:
        sz, _, rest = rest.partition(b"\r\n")
        try:
            n = int(sz.split(b";")[0], 16)
        except ValueError:
            return out + b"[BAD CHUNK]" + sz + rest
        if n == 0:
            return out + (b"[END]" if rest.startswith(b"\r\n") else b"[END " + rest + b"]")
        out += rest[:n]
        rest = rest[n + 2:]
    return out


def read_stream(f, deadline, until):
    """An event stream's answer: the head, then the body (de-chunked) until it ends, `until` has come with the blank line
    after it, or the deadline."""
    head = b""
    while not head.endswith(b"\r\n\r\n"):
        line = readline(f, deadline)
        if not line:
            return head.decode("utf-8", "backslashreplace") + "[NO HEAD END]"
        head += line
    chunked = b"transfer-encoding: chunked" in head.lower()
    body = b""
    seen = False
    while True:
        if chunked:
            if b"0\r\n\r\n" in body and dechunk(body).endswith(b"[END]"):
                break
        text = dechunk(body) if chunked else body
        i = text.find(until.encode())
        if i >= 0 and b"\n\n" in text[i:] and not seen:
            # a stream that ends after it (a run's own stream) gets half a second to show its end
            seen = True
            deadline = min(deadline, time.monotonic() + 0.5)
        if not fill_buf(f, deadline):
            break
        body += f["buf"]
        f["buf"] = b""
    text = dechunk(body) if chunked else body
    if until and until.encode() in text:
        j = text.find(until.encode())
        k = text.find(b"\n\n", j)
        if k >= 0 and not text[k + 2:].startswith(b"[END"):
            text = text[:k + 2] + b"[CUT]"
    status = "ENDED" if f["eof"] or text.endswith(b"[END]") else "OPEN"
    return head.decode("utf-8", "backslashreplace") + text.decode("utf-8", "backslashreplace") + " " + status


# ---- running ---------------------------------------------------------------------------------------------------------

def make_files(work, files, sub):
    """The CLI differential's files (a name ending in "|" is a named pipe), then each regular file's times set to fixed,
    increasing values (the session list shows and sorts by them)."""
    cd.make_files(work, {k: v for k, v in files.items() if not k.endswith("|")}, sub)
    for name in files:
        if name.endswith("|"):
            p = os.path.join(work, name[:-1])
            os.makedirs(os.path.dirname(p), exist_ok=True)
            os.mkfifo(p)
    t = 1_767_225_600  # 2026-01-01
    for i, name in enumerate(sorted(files)):
        p = os.path.join(work, name.rstrip("/@*%"))
        if os.path.isfile(p) and not os.path.islink(p):
            os.utime(p, (t + i, t + i))


def run_scenario(binary, sc, base, backend, limbo, gobin, busy_port):
    work = os.path.join(base, "w")
    if os.path.exists(work):
        shutil.rmtree(work)
    os.makedirs(work)
    sockp = os.path.join(base, "agent.sock")
    ctx = {"W": work, "S": sockp, "H": backend.url, "G": gobin, "BUSY": str(busy_port), "PORT": "0", "TOKEN": "NOTOKEN"}

    def sub(s):
        if isinstance(s, bytes):
            for k, v in ctx.items():
                s = s.replace(("{%s}" % k).encode(), str(v).encode())
            return s
        for k, v in ctx.items():
            s = s.replace("{%s}" % k, str(v))
        return s

    make_files(work, sc["files"], sub)
    env = {"PATH": os.path.join(work, "bin"), "LANG": "C.UTF-8", "HOME": os.path.join(work, "home"), "USER": "tester",
           "SARINA_CONTAINED_PATH": os.path.join(work, "no-lock"), "SARINA_RS_NO_GO": "1"}
    for k, v in sc["env"].items():
        env[k] = sub(v)
    backend.reset(sc["model"], [sub(x) if isinstance(x, str) else x for x in sc["heimdall"]], sc["probe"])
    limbo.reset(sc["limbo"])
    if sc["argv0"]:
        prog = os.path.join(base, sc["argv0"])
        if os.path.lexists(prog):
            os.unlink(prog)
        os.symlink(binary, prog)
        argv = [prog]
    else:
        argv = [binary, "gui"]
    # the kernel picks the port (a scenario's own --port comes later and wins); never the real console's 8765
    argv += ["--port", "0"] + ([] if sc["browser"] else ["--no-browser"]) + [sub(x) for x in sc["args"]]
    p = subprocess.Popen(argv, cwd=os.path.join(work, "ws"), env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE)
    errbuf = []
    ready = threading.Event()

    def pump():
        for line in iter(p.stderr.readline, b""):
            errbuf.append(line)
            if b"keep it to yourself" in line:
                ready.set()
        ready.set()

    th = threading.Thread(target=pump, daemon=True)
    th.start()
    ready.wait(30)
    port, token = 0, ""
    m = re.search(rb"sarina gui: http://(?:\[[^\]]*\]|[^:\s]*):(\d+)/\?t=([0-9a-f]+)", b"".join(errbuf))
    if m:
        port, token = int(m.group(1)), m.group(2).decode()
    ctx["PORT"], ctx["TOKEN"] = str(port), token or "NOTOKEN"
    out = []
    ids = {"S": [], "AP": []}
    bg = []

    def fill(s):
        s = sub(s)
        for k in ("S", "AP"):
            for i, v in enumerate(ids[k]):
                key = "{%s%d}" % (k, i)
                s = s.replace(key, v) if isinstance(s, str) else s.replace(key.encode(), v.encode())
        return s

    def note_ids(text):
        for sid in re.findall(r'"session_id":"([0-9a-f]{32})"', text):
            if sid not in ids["S"]:
                ids["S"].append(sid)
        for aid in re.findall(r'"approval_id":"(a\d+-[0-9a-f]{8})"', text):
            if aid not in ids["AP"]:
                ids["AP"].append(aid)

    def build(step):
        _, method, path, body, k = step
        path = fill(path)
        if isinstance(body, (dict, list)) or body is None and False:
            body = fill(json.dumps(body))
        elif isinstance(body, (str, bytes)):
            body = fill(body)
        elif body is not None:
            body = fill(json.dumps(body))
        hdrs = {}
        tok = k.get("token", "{TOKEN}")
        if tok is not None:
            hdrs["X-Sarina-Token"] = fill(tok)
        for kk, v in (k.get("headers") or {}).items():
            hdrs[kk] = fill(v)
        host = k.get("host", "127.0.0.1:{PORT}")
        ctype = k.get("ctype", "application/json")
        return request(method, path, body, hdrs, None if host is None else fill(host), k.get("proto", "HTTP/1.1"), ctype)

    def do_req(step):
        r = sd.exchange(port, [build(step)])
        note_ids(r[0])
        if step[4].get("ties"):
            r = [history_by_set(r[0], work)]
        return r

    def do_sse(step):
        k = step[4]
        f = connect(port)
        try:
            f["sock"].sendall(build(step))
            text = read_stream(f, time.monotonic() + k.get("wait", 20), k.get("until", "event: done"))
        except OSError as e:
            text = "SEND FAILED: %s" % type(e).__name__
        finally:
            f["sock"].close()
        note_ids(text)
        return [text]

    def run_step(step):
        kind = step[0]
        if kind == "req":
            return do_req(step)
        if kind == "sse":
            return do_sse(step)
        raise ValueError(kind)

    for step in sc["steps"]:
        if p.poll() is not None or not port:
            out.append("CONSOLE NOT RUNNING")
            break
        kind = step[0]
        if kind in ("req", "sse"):
            out.append(run_step(step))
        elif kind == "bg":
            res = {}

            def go(st=step[1], res=res):
                res["r"] = run_step(st)
            t = threading.Thread(target=go, daemon=True)
            t.start()
            bg.append((t, res))
            time.sleep(0.1)
        elif kind == "join":
            got = []
            for t, res in bg:
                t.join(30)
                got.append(res.get("r", "NOT DONE"))
            bg.clear()
            out.append(got)
        elif kind == "approvals":
            deadline = time.monotonic() + 15
            r = None
            while time.monotonic() < deadline:
                r = sd.exchange(port, [build(R("GET", "/api/approvals"))])[0]
                body = r.split("\r\n\r\n", 1)[-1]
                if body.count('"approval_id"') >= step[1]:
                    break
                time.sleep(0.1)
            note_ids(r or "")
            out.append(r)
        elif kind == "model_waiting":
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline and backend.count("model") < 1:
                time.sleep(0.05)
            time.sleep(0.2)
            out.append("MODEL CALLED" if backend.count("model") else "MODEL NOT CALLED")
        elif kind in ("raw", "raw6"):
            if kind == "raw6":
                s = socket.create_connection(("::1", port), timeout=10)
                s.sendall(fill(step[1]))
                f = {"sock": s, "buf": b"", "eof": False}
                r, _ = read_response(f, "GET", time.monotonic() + 10)
                s.close()
                out.append([r.decode("utf-8", "backslashreplace") if r else "NO RESPONSE"])
            else:
                out.append(sd.exchange(port, [fill(step[1])], wait=5, quiet=0.3))
        elif kind == "conn":
            out.append(sd.exchange(port, [fill(x) for x in step[1]], wait=5, quiet=0.3))
        elif kind == "big":
            declared, send, path = step[1], step[2], step[3]
            head = (f"POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Sarina-Token: {token}\r\nContent-Type: application/json\r\n"
                    f"Content-Length: {declared}\r\n\r\n").encode()
            f = connect(port)
            body = b'{"model": "' + b"x" * (send - 13) + b'"}'
            res = ""
            try:
                f["sock"].sendall(head)
                f["sock"].sendall(body[:send])
            except OSError as e:
                res = "SEND FAILED: %s " % type(e).__name__
            r, _ = read_response(f, "POST", time.monotonic() + 40)
            rest = readall(f, time.monotonic() + 1.5)
            f["sock"].close()
            out.append([res + (r.decode("utf-8", "backslashreplace") if r else "NO RESPONSE"),
                        "CLOSED" if f["eof"] else ("AFTER " + rest.decode("latin-1")) if rest else "OPEN"])
        elif kind == "drop":
            inner = step[1]
            s = socket.create_connection(("127.0.0.1", port), timeout=10)
            s.sendall(build(inner))
            time.sleep(step[2])
            s.close()
            out.append("DROPPED")
        elif kind == "sleep":
            time.sleep(step[1])
        elif kind == "signal":
            p.send_signal(step[1])
            # a stream may still be open: its result is joined at the next step
    if bg:
        got = []
        for t, res in bg:
            t.join(30)
            got.append(res.get("r", "NOT DONE"))
        out.append(got)
    if p.poll() is None:
        if not any(s[0] == "signal" for s in sc["steps"]):
            p.send_signal(signal.SIGTERM)
    try:
        code = p.wait(timeout=20)
    except subprocess.TimeoutExpired:
        p.kill()
        code = "KILLED"
    th.join(5)
    stdout = p.stdout.read()
    p.stdout.close()
    p.stderr.close()
    time.sleep(0.2)
    with backend.lock:
        reqs = list(backend.requests)
    with limbo.lock:
        lreqs = list(limbo.requests)
    res = {"responses": out, "stderr": b"".join(errbuf).decode("utf-8", "backslashreplace"),
           "stdout": stdout.decode("utf-8", "backslashreplace"), "exit": code, "requests": reqs, "limbo": lreqs,
           "files": sd.snapshot(work)}
    return normalize(res, port, token, work)


def history_by_set(raw, work):
    """A file-history answer whose entries' times are whole seconds made a run's own (item 5 of "Resume here"): the
    console sorts newest first and keeps the session's order among entries of one second, so a run whose writes straddle a
    second lists them in another order than one whose writes fall in one second, in either build. The real checks are
    made on this run's own times: newest first, and each second's entries in the order the session file holds them; then
    the entries are listed as a set (sorted), the rest of the answer (status, headers, total) as it came."""
    head, sep, body = raw.partition("\r\n\r\n")
    if "Transfer-Encoding: chunked" in head:
        b = dechunk(body.encode("utf-8", "surrogateescape"))
        body = b[:-len(b"[END]")].decode("utf-8", "surrogateescape") if b.endswith(b"[END]") else ""
    try:
        doc = json.loads(body)
        es = doc["entries"]
    except (ValueError, KeyError, TypeError):
        return raw
    ts = [e.get("timestamp", "") for e in es]
    newest_first = all(ts[i] >= ts[i + 1] for i in range(len(ts) - 1))
    order = {}
    for root, _, names in os.walk(work):
        for n in names:
            if n.endswith(".json"):
                try:
                    with open(os.path.join(root, n), "rb") as f:
                        sess = json.loads(f.read())
                    for i, h in enumerate(sess.get("file_history") or []):
                        order.setdefault((sess.get("session_id"), json.dumps(h, sort_keys=True)), i)
                except (ValueError, OSError, AttributeError):
                    pass
    def pos(e):
        k = {x: y for x, y in e.items() if x != "session_id"}
        return order.get((e.get("session_id"), json.dumps(k, sort_keys=True)), -1)
    ties_in_file_order = all(pos(es[i]) < pos(es[i + 1]) for i in range(len(es) - 1)
                             if ts[i] == ts[i + 1] and es[i].get("session_id") == es[i + 1].get("session_id"))
    doc["entries"] = sorted(es, key=lambda e: json.dumps(e, sort_keys=True))
    doc["newest first"], doc["a second's entries in the session's order"] = newest_first, ties_in_file_order
    return head + sep + json.dumps(doc, sort_keys=True, ensure_ascii=False)


NORM = [(r"[0-9a-f]{60}", "BOUNDARY"), (r"mem_[0-9a-f]{12}", "mem_ID"),
        (r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})", "TIME"),
        (r"Date: [A-Z][a-z]{2}, \d{2} [A-Z][a-z]{2} \d{4} \d{2}:\d{2}:\d{2} GMT", "Date: DATE"),
        (r"Today's date is [^.]*\.", "Today's date is DAY."),
        (r'(\\?"modified_at\\?": ?)17[0-9]{8}', r"\g<1>MTIME"),
        (r"(a\d+-)[0-9a-f]{8}", r"\g<1>APPROVAL")]


def normalize(res, port, token, work):
    """The port, the launch token, Date headers, times, a multipart boundary, run ids (12 hex digits after "run_id") and
    random ids (32 hex digits: numbered in the order they first appear in the responses, then the requests, the output
    and the files, so both runs number alike)."""
    text = json.dumps(res, ensure_ascii=False, sort_keys=True)
    if token:
        text = text.replace(token, "TOKEN")
    if port:
        text = text.replace(":%d" % port, ":PORT")
    seq = json.dumps([res["responses"], res["requests"], res["limbo"], res["stderr"], res["stdout"]], ensure_ascii=False)
    seq += json.dumps(sorted(res["files"].items()), ensure_ascii=False)
    if token:
        seq = seq.replace(token, "TOKEN")
    runs = {}
    for m in re.finditer(r'run_id\\*":\\*"([0-9a-f]{12})', seq):
        runs.setdefault(m.group(1), "RUN%d" % len(runs))
    for rid, name in runs.items():
        text = text.replace(rid, name)
    for pat, rep in NORM[:1]:
        text = re.sub(pat, rep, text)
    order = {}
    for m in re.finditer(r"[0-9a-f]{32}", seq):
        order.setdefault(m.group(0), "ID%d" % len(order))
    text = re.sub(r"[0-9a-f]{32}", lambda m: order.get(m.group(0), "ID?"), text)
    for pat, rep in NORM[1:]:
        text = re.sub(pat, rep, text)
    out = json.loads(text)
    out["files"] = dict(sorted(out["files"].items()))
    return out


def as_go_1_27_4(res):
    """Go 1.27.3 and older put a body-too-large answer's `Connection: close` among the handler's own headers; 1.27.4
    writes it after the length, as sarina-rs does: an older Go build's answers are moved to the newer form."""
    def fix(x):
        if isinstance(x, list):
            return [fix(y) for y in x]
        if isinstance(x, str) and x.startswith("HTTP/1.1 413"):
            head, _, body = x.partition("\r\n\r\n")
            lines = head.split("\r\n")
            if "Connection: close" in lines[:-1]:
                lines = [ln for ln in lines if ln != "Connection: close"] + ["Connection: close"]
            return "\r\n".join(lines) + "\r\n\r\n" + body
        return x
    res["responses"] = fix(res["responses"])
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--go", required=True)
    ap.add_argument("--rs", required=True)
    ap.add_argument("--out", help="scratch folder (short: a socket lives in it); default a new one in the temp folder")
    ap.add_argument("--only", default="", help="run only the scenarios whose name contains this")
    ap.add_argument("--skip-slow", action="store_true", help="leave out the scenarios that wait out a deadline")
    ap.add_argument("--show", action="store_true", help="print each scenario's Go result (and sarina-rs's when they differ)")
    o = ap.parse_args()
    go, rs = os.path.abspath(o.go), os.path.abspath(o.rs)
    base = o.out or tempfile.mkdtemp(prefix="sgd")
    os.makedirs(base, exist_ok=True)
    if len(os.path.join(base, "agent.sock")) >= 100:
        sys.exit(f"{base}: too long for a socket path")
    os.umask(0o022)
    us = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sockp = os.path.join(base, "agent.sock")
    if os.path.exists(sockp):
        os.unlink(sockp)
    us.bind(sockp)
    limbo = cd.Server(us)
    backend = Backend()
    busy = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    busy.bind(("127.0.0.1", 0))
    busy.listen(1)
    busy_port = busy.getsockname()[1]
    slow = {s["name"] for s in slow_scenarios()}
    scs = [s for s in scenarios() if o.only in s["name"] and not (o.skip_slow and s["name"] in slow)]
    keys = ("responses", "stderr", "stdout", "exit", "requests", "limbo", "files")
    bad = 0
    gov = sd.go_minor_patch(go)
    old_go = gov is not None and gov < (27, 4)
    for s in scs:
        g = run_scenario(go, s, base, backend, limbo, go, busy_port)
        if old_go:
            g = as_go_1_27_4(g)
        r = run_scenario(rs, s, base, backend, limbo, go, busy_port)
        diffs = [k for k in keys if g[k] != r[k]]
        if o.show:
            print(f"== {s['name']}: {json.dumps(g, indent=1, ensure_ascii=False)}")
            if g != r:
                print(f"== rs {s['name']}: {json.dumps(r, indent=1, ensure_ascii=False)}")
        if diffs:
            bad += 1
            print(f"DIFF {s['name']}")
            for k in diffs:
                if k == "responses":
                    for i, (x, y) in enumerate(zip(g[k], r[k])):
                        if x != y:
                            xs, ys = json.dumps(x, ensure_ascii=False), json.dumps(y, ensure_ascii=False)
                            j = next((n for n in range(min(len(xs), len(ys))) if xs[n] != ys[n]), min(len(xs), len(ys)))
                            lo = max(0, j - 400)
                            print(f"  step {i} (at {j}):\n    go: ...{xs[lo:j + 700]}\n    rs: ...{ys[lo:j + 700]}")
                    if len(g[k]) != len(r[k]):
                        print(f"  {len(g[k])} steps against {len(r[k])}")
                    continue
                a_, b_ = json.dumps(g[k], ensure_ascii=False), json.dumps(r[k], ensure_ascii=False)
                i = next((j for j in range(min(len(a_), len(b_))) if a_[j] != b_[j]), min(len(a_), len(b_)))
                lo = max(0, i - 300)
                print(f"  {k} (first difference at {i}):\n    go: ...{a_[lo:i + 600]}\n    rs: ...{b_[lo:i + 600]}")
        sys.stdout.flush()
    w = os.path.join(base, "w")
    if os.path.exists(w):
        shutil.rmtree(w)
    os.unlink(sockp)
    if not o.out:
        shutil.rmtree(base, ignore_errors=True)
    print(f"{len(scs)} scenarios, {sum(len(s['steps']) for s in scs)} steps, {len(scs) * len(keys)} comparisons "
          "(responses, stderr, stdout, exit status, model/Heimdall/probe requests, Limbo requests, files): "
          + ("NO DIFFERENCE" if bad == 0 else f"{bad} scenarios differ"))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
