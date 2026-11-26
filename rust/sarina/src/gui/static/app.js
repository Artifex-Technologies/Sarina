"use strict";
(function () {
  const TOKEN = document.querySelector('meta[name="sarina-token"]').content;
  const $ = (id) => document.getElementById(id);

  // ---- small helpers -----------------------------------------------------------------

  function el(tag, props, ...kids) {
    const n = document.createElement(tag);
    if (props) {
      for (const k of Object.keys(props)) {
        const v = props[k];
        if (k === "class") n.className = v;
        else if (k === "text") n.textContent = v;
        else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
        else if (v !== false && v !== null && v !== undefined) n.setAttribute(k, v === true ? "" : v);
      }
    }
    for (const kid of kids) {
      if (kid === null || kid === undefined || kid === false) continue;
      n.appendChild(typeof kid === "string" ? document.createTextNode(kid) : kid);
    }
    return n;
  }

  async function api(method, path, body) {
    const opts = { method, headers: { "X-Sarina-Token": TOKEN, "Content-Type": "application/json" } };
    if (body !== undefined) opts.body = JSON.stringify(body);
    let resp;
    try {
      resp = await fetch(path, opts);
    } catch (e) {
      throw new Error("Cannot reach the console server. Is it still running?");
    }
    let data = null;
    try { data = await resp.json(); } catch (e) { data = null; }
    if (!resp.ok) {
      let msg = "Request failed (" + resp.status + ")";
      if (data && typeof data.detail === "string") msg = data.detail;
      else if (data && Array.isArray(data.detail) && data.detail[0]) msg = data.detail[0].msg || msg;
      else if (data && data.error) msg = (data.error_type ? data.error_type + ": " : "") + data.error;
      const err = new Error(msg);
      err.status = resp.status;
      throw err;
    }
    return data;
  }

  let toastTimer = 0;
  function toast(msg) {
    const t = $("toast");
    t.textContent = msg;
    t.classList.remove("hidden");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => t.classList.add("hidden"), 3500);
  }

  function ago(unix) {
    const s = Math.max(0, Date.now() / 1000 - unix);
    if (s < 60) return "just now";
    if (s < 3600) return Math.floor(s / 60) + " min ago";
    if (s < 86400) return Math.floor(s / 3600) + " h ago";
    return Math.floor(s / 86400) + " d ago";
  }

  function fmtNum(n) {
    return typeof n === "number" ? n.toLocaleString() : String(n);
  }

  function clipText(s, n) {
    s = String(s == null ? "" : s);
    return s.length > n ? s.slice(0, n) + "…" : s;
  }

  // ---- markdown, built as DOM nodes (model text is never parsed as markup) --------------

  function safeHref(u) {
    return /^(https?:\/\/|mailto:)/i.test(u) ? u : null;
  }

  function inline(text, parent) {
    // order: code spans, links, bold, italic
    const re = /(`+)([\s\S]+?)\1|\[([^\]]+)\]\(([^)\s]+)\)|\*\*([^*]+)\*\*|\*([^*\s][^*]*)\*/g;
    let last = 0, m;
    while ((m = re.exec(text)) !== null) {
      if (m.index > last) parent.appendChild(document.createTextNode(text.slice(last, m.index)));
      if (m[1]) parent.appendChild(el("code", { text: m[2] }));
      else if (m[3] !== undefined) {
        const href = safeHref(m[4]);
        if (href) parent.appendChild(el("a", { href, target: "_blank", rel: "noopener noreferrer nofollow", text: m[3] }));
        else parent.appendChild(document.createTextNode(m[0]));
      } else if (m[5] !== undefined) {
        parent.appendChild(el("strong", { text: m[5] }));
      } else parent.appendChild(el("em", { text: m[6] }));
      last = re.lastIndex;
    }
    if (last < text.length) parent.appendChild(document.createTextNode(text.slice(last)));
  }

  function codeBlock(lang, code) {
    const copy = el("button", { class: "copy", type: "button", text: "Copy" });
    copy.addEventListener("click", () => {
      const done = () => { copy.textContent = "Copied"; setTimeout(() => (copy.textContent = "Copy"), 1200); };
      if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(code).then(done, () => toast("Copy failed"));
      else toast("Copy is not available here");
    });
    return el("div", { class: "codeblock" },
      el("div", { class: "codehead" }, el("span", { text: lang || "text" }), copy),
      el("pre", null, el("code", { text: code })));
  }

  function renderMarkdown(text, root) {
    root.textContent = "";
    const lines = String(text).replace(/\r\n?/g, "\n").split("\n");
    let i = 0;
    let para = [];
    const flush = () => {
      if (!para.length) return;
      const p = el("p");
      para.forEach((ln, idx) => {
        if (idx) p.appendChild(document.createElement("br"));
        inline(ln, p);
      });
      root.appendChild(p);
      para = [];
    };
    while (i < lines.length) {
      const line = lines[i];
      let m = /^\s*(`{3,}|~{3,})\s*([\w+#.-]*)\s*$/.exec(line);
      if (m) {
        flush();
        const fence = m[1], lang = m[2], buf = [];
        i++;
        while (i < lines.length && !lines[i].trim().startsWith(fence)) buf.push(lines[i++]);
        i++;
        root.appendChild(codeBlock(lang, buf.join("\n")));
        continue;
      }
      if ((m = /^(#{1,6})\s+(.*)$/.exec(line))) {
        flush();
        const h = el("h" + Math.min(5, m[1].length + 2));
        inline(m[2], h);
        root.appendChild(h);
        i++;
        continue;
      }
      if (/^\s*([-*_])\s*\1\s*\1[\s\-*_]*$/.test(line)) { flush(); root.appendChild(el("hr")); i++; continue; }
      if (/^\s*>\s?/.test(line)) {
        flush();
        const buf = [];
        while (i < lines.length && /^\s*>\s?/.test(lines[i])) buf.push(lines[i++].replace(/^\s*>\s?/, ""));
        const q = el("blockquote");
        inline(buf.join(" "), q);
        root.appendChild(q);
        continue;
      }
      if ((m = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/.exec(line))) {
        flush();
        const ordered = /\d/.test(m[2]);
        const list = el(ordered ? "ol" : "ul");
        while (i < lines.length && (m = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/.exec(lines[i]))) {
          const li = el("li");
          inline(m[3], li);
          list.appendChild(li);
          i++;
        }
        root.appendChild(list);
        continue;
      }
      if (line.trim() === "") { flush(); i++; continue; }
      para.push(line);
      i++;
    }
    flush();
  }

  // ---- state --------------------------------------------------------------------------------

  const S = {
    settings: null,
    run: { state: "idle", id: null },
    resumeId: null,       // the session the next prompt continues
    pastes: {},           // id -> text
    pasteN: 0,
    cur: null,            // live bubble state for the run being shown
    tools: {},            // tool_call_id -> card
    seenRuns: new Set(),
    streamAbort: null,
    slash: null,
  };

  // ---- chat stream rendering ----------------------------------------------------------------

  const streamEl = $("stream");

  function scrollDown(force) {
    const near = streamEl.scrollHeight - streamEl.scrollTop - streamEl.clientHeight < 140;
    if (force || near) streamEl.scrollTop = streamEl.scrollHeight;
  }

  function hideWelcome() { $("welcome").classList.add("hidden"); }

  function addUser(text) {
    hideWelcome();
    const b = el("div", { class: "bubble", text });
    streamEl.appendChild(el("div", { class: "msg user" }, el("div", { class: "who", text: "You" }), b));
    scrollDown(true);
  }

  function newAssistant() {
    hideWelcome();
    const body = el("div", { class: "bubble typing" });
    const wrap = el("div", { class: "msg assistant" }, el("div", { class: "who", text: "Sarina" }), body);
    streamEl.appendChild(wrap);
    return { wrap, body, text: "", raf: 0 };
  }

  function paintAssistant(a, final) {
    cancelAnimationFrame(a.raf);
    const draw = () => {
      renderMarkdown(a.text, a.body);
      if (final) a.body.classList.remove("typing");
      scrollDown(false);
    };
    if (final) draw(); else a.raf = requestAnimationFrame(draw);
  }

  function addNote(text, warn) {
    streamEl.appendChild(el("div", { class: "note" + (warn ? " warn" : ""), text }));
    scrollDown(false);
  }

  function addError(title, text) {
    hideWelcome();
    streamEl.appendChild(el("div", { class: "msg error" }, el("div", { class: "who", text: title }), el("div", { class: "bubble", text })));
    scrollDown(true);
  }

  function argPreview(args) {
    if (!args || typeof args !== "object") return "";
    if (typeof args.command === "string") return args.command;
    if (typeof args.path === "string") return args.path;
    const first = Object.keys(args)[0];
    return first ? first + "=" + clipText(JSON.stringify(args[first]), 60) : "";
  }

  function makeToolCard(id, name, effect, args) {
    const status = el("span", { class: "badge", text: "running" });
    const badges = [];
    if (effect === "write" || effect === "shell") badges.push(el("span", { class: "badge " + effect, text: effect }));
    const argsPre = el("pre", { text: args === undefined ? "" : JSON.stringify(args, null, 2) });
    const outPre = el("pre", { text: "" });
    const outWrap = el("div", null, el("h4", { text: "Result" }), outPre);
    const more = el("button", { class: "btn small hidden", type: "button", text: "Show everything" });
    const body = el("div", { class: "tbody" }, el("h4", { text: "Arguments" }), argsPre, outWrap, more);
    const card = el("details", { class: "tool" },
      el("summary", null, el("span", { class: "tname", text: name }), ...badges,
        el("span", { class: "targs", text: clipText(argPreview(args), 120) }), status),
      body);
    const t = { card, status, outPre, more, full: "", id };
    more.addEventListener("click", () => { outPre.textContent = t.full; more.classList.add("hidden"); });
    streamEl.appendChild(card);
    S.tools[id] = t;
    scrollDown(false);
    return t;
  }

  function setToolOutput(t, text) {
    t.full = text;
    const LIM = 4000;
    if (text.length > LIM) {
      t.outPre.textContent = text.slice(0, LIM) + "\n… (" + fmtNum(text.length - LIM) + " more characters)";
      t.more.classList.remove("hidden");
    } else {
      t.outPre.textContent = text;
      t.more.classList.add("hidden");
    }
  }

  function finishTool(t, ok, text) {
    t.status.textContent = ok ? "done" : "failed";
    t.status.className = "badge " + (ok ? "ok" : "err");
    setToolOutput(t, text);
    if (!ok) t.card.open = true;
  }

  // ---- approvals ----------------------------------------------------------------------------

  const approvalsEl = $("approvals");

  function approvalCard(ap) {
    if (document.getElementById("ap-" + ap.approval_id)) return;
    const card = el("div", { class: "approval", id: "ap-" + ap.approval_id, role: "group" });
    const answer = async (decision, reason) => {
      card.querySelectorAll("button").forEach((b) => (b.disabled = true));
      try {
        await api("POST", "/api/approvals/" + encodeURIComponent(ap.approval_id), { decision, reason: reason || "" });
      } catch (e) {
        card.querySelectorAll("button").forEach((b) => (b.disabled = false));
        toast(e.message);
      }
    };
    const reason = el("input", { class: "field", type: "text", placeholder: ap.kind === "plan" ? "Feedback (needed to ask for changes)" : "Reason for denying (optional)", "aria-label": "Reason" });
    if (ap.kind === "plan") {
      card.appendChild(el("h3", { text: "The agent proposes a plan" }));
      card.appendChild(el("pre", { text: ap.plan || "" }));
      card.appendChild(reason);
      card.appendChild(el("div", { class: "actions" },
        el("button", { class: "btn primary", type: "button", text: "Approve plan", onclick: () => answer("approve") }),
        el("button", { class: "btn", type: "button", text: "Ask for changes", onclick: () => {
          if (!reason.value.trim()) { reason.focus(); toast("Say what should change first."); return; }
          answer("revise", reason.value);
        } }),
        el("button", { class: "btn danger", type: "button", text: "Reject", onclick: () => answer("reject", reason.value) })));
    } else {
      card.appendChild(el("h3", { text: "Allow " + ap.tool_name + "?" }));
      card.appendChild(el("p", { class: "risk", text: ap.risk || "" }));
      card.appendChild(el("p", { class: "small", text: ap.summary || "" }));
      card.appendChild(el("pre", { text: JSON.stringify(ap.arguments || {}, null, 2) }));
      card.appendChild(reason);
      card.appendChild(el("div", { class: "actions" },
        el("button", { class: "btn primary", type: "button", text: "Allow once", onclick: () => answer("allow_once") }),
        el("button", { class: "btn", type: "button", text: "Allow for this run", onclick: () => answer("allow_session") }),
        el("button", { class: "btn danger", type: "button", text: "Deny", onclick: () => answer("deny", reason.value) })));
    }
    approvalsEl.appendChild(card);
    const first = card.querySelector("button");
    if (first) first.focus({ preventScroll: true });
    card.scrollIntoView({ block: "nearest" });
  }

  function dropApproval(id) {
    const n = document.getElementById("ap-" + id);
    if (n) n.remove();
  }

  // ---- run state and events -----------------------------------------------------------------

  function setRunState(state) {
    S.run.state = state;
    const pill = $("run-pill");
    pill.textContent = state === "waiting_approval" ? "needs your answer" : state === "finished" ? "idle" : state;
    pill.className = "pill" + (state === "running" ? " running" : state === "waiting_approval" ? " waiting" : "");
    const busy = state === "running" || state === "waiting_approval";
    $("send").classList.toggle("hidden", busy);
    $("stop").classList.toggle("hidden", !busy);
    $("prompt").disabled = busy;
    $("settings-busy").classList.toggle("hidden", !busy);
  }

  function stopHint(reason) {
    switch (reason) {
      case "backend_error": return "The model backend did not answer. Check the base URL and that the server is running (Settings, or the badge at the top).";
      case "max_turns": return "Stopped at the turn limit. Raise Max turns in Settings, or send a follow-up to continue.";
      case "budget_exceeded": return "Stopped at a budget limit. Raise it in Settings to continue.";
      case "prompt_too_long": return "The conversation no longer fits the model's context window. Start a new chat or lower the compaction threshold.";
      case "plan_pending": return "The plan is still waiting for your decision (see the Plan tab).";
      case "interrupted": return "Stopped.";
    }
    return "";
  }

  function onEvent(e) {
    if (!S.run.id || e.run_id !== S.run.id) {
      if (e.type === "run_start") { S.run.id = e.run_id; } else return;
    }
    switch (e.type) {
      case "run_start":
        if (!S.shownPrompt) addUser(e.prompt);
        S.shownPrompt = false;
        setRunState("running");
        break;
      case "turn_start":
        S.cur = newAssistant();
        break;
      case "text_delta":
        if (!S.cur) S.cur = newAssistant();
        S.cur.text += e.delta;
        paintAssistant(S.cur, false);
        break;
      case "assistant_message":
        if (!S.cur) S.cur = newAssistant();
        if (e.content) { S.cur.text = e.content; paintAssistant(S.cur, true); }
        else if (!S.cur.text) S.cur.wrap.remove();
        else paintAssistant(S.cur, true);
        S.cur = null;
        break;
      case "tool_call":
        makeToolCard(e.tool_call_id, e.name, e.effect, e.arguments);
        break;
      case "tool_delta": {
        const t = S.tools[e.tool_call_id];
        if (t) { setToolOutput(t, t.full + e.delta); }
        break;
      }
      case "tool_result": {
        const t = S.tools[e.tool_call_id] || makeToolCard(e.tool_call_id, e.name, "read", undefined);
        finishTool(t, e.ok === true, e.content || "");
        break;
      }
      case "approval_request":
        approvalCard(e.approval);
        setRunState("waiting_approval");
        break;
      case "approval_resolved":
        dropApproval(e.approval_id);
        if (S.run.state === "waiting_approval" && !approvalsEl.children.length) setRunState("running");
        break;
      case "info":
        if (e.text) addNote(e.text, e.name === "tier_refused");
        break;
      case "usage":
        showUsage(e);
        break;
      case "error":
        addError(e.error_type || "Error", e.error);
        break;
      case "done":
        finishRun(e.result);
        break;
    }
  }

  function finishRun(result) {
    if (S.cur) { S.cur.body.classList.remove("typing"); S.cur = null; }
    approvalsEl.textContent = "";
    setRunState("idle");
    if (result) {
      S.resumeId = result.session_id || S.resumeId;
      const hint = stopHint(result.stop_reason);
      if (result.stop_reason && result.stop_reason !== "stop" && result.stop_reason !== "tool_calls") {
        if (result.stop_reason === "backend_error" && result.final_output) addError("Backend error", result.final_output + "\n\n" + hint);
        else addNote("Stopped: " + result.stop_reason + (hint ? ". " + hint : ""), result.stop_reason !== "interrupted");
      }
    }
    $("prompt").focus();
    refreshSessions();
    refreshState();
  }

  function showUsage(u) {
    const us = u.usage || {};
    const total = us.total_tokens !== undefined ? us.total_tokens : (us.input_tokens || 0) + (us.output_tokens || 0);
    let s = fmtNum(total) + " tokens";
    if (u.tool_calls !== undefined) s += " · " + u.tool_calls + " tool call" + (u.tool_calls === 1 ? "" : "s");
    if (u.total_cost_usd) s += " · $" + Number(u.total_cost_usd).toFixed(4);
    $("sb-usage").textContent = s;
  }

  async function readSSE(resp, onEv, signal) {
    const reader = resp.body.getReader();
    const dec = new TextDecoder();
    let buf = "";
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let i;
      while ((i = buf.indexOf("\n\n")) >= 0) {
        const chunk = buf.slice(0, i);
        buf = buf.slice(i + 2);
        const data = chunk.split("\n").filter((l) => l.startsWith("data: ")).map((l) => l.slice(6)).join("\n");
        if (!data) continue;
        let ev;
        try { ev = JSON.parse(data); } catch (err) { continue; }
        onEv(ev);
        if (ev.type === "done") { try { await reader.cancel(); } catch (err) { /* already closed */ } return; }
      }
      if (signal && signal.aborted) return;
    }
  }

  async function send(promptText) {
    promptText = promptText.trim();
    if (!promptText || S.run.state === "running" || S.run.state === "waiting_approval") return;
    const pasted = {};
    for (const id of Object.keys(S.pastes)) {
      if (promptText.indexOf("[Pasted text #" + id) >= 0) pasted[id] = { type: "text", content: S.pastes[id] };
    }
    addUser(promptText);
    S.shownPrompt = true;
    S.run = { state: "running", id: null };
    S.tools = {};
    setRunState("running");
    const body = { prompt: promptText, resume_session_id: S.resumeId };
    if (Object.keys(pasted).length) body.pasted_contents = pasted;
    let resp;
    try {
      resp = await fetch("/api/chat", {
        method: "POST",
        headers: { "X-Sarina-Token": TOKEN, "Content-Type": "application/json", Accept: "text/event-stream" },
        body: JSON.stringify(body),
      });
    } catch (e) {
      addError("Connection", "Cannot reach the console server.");
      setRunState("idle");
      return;
    }
    if (!resp.ok) {
      let msg = "Request failed (" + resp.status + ")";
      try { const d = await resp.json(); if (typeof d.detail === "string") msg = d.detail; else if (d.detail && d.detail[0]) msg = d.detail[0].msg; } catch (e) { /* keep default */ }
      addError("Could not start", msg);
      setRunState("idle");
      return;
    }
    S.pastes = {}; S.pasteN = 0; $("chips").textContent = "";
    try {
      await readSSE(resp, onEvent);
    } catch (e) {
      addError("Connection", "The event stream was interrupted. Reload the page to reattach to the run.");
    }
    if (S.run.state !== "idle") setRunState("idle");
  }

  async function attachToRun(run) {
    S.run = { state: run.state, id: run.run_id };
    S.tools = {};
    setRunState(run.state);
    try {
      const resp = await fetch("/api/events?after=0", { headers: { "X-Sarina-Token": TOKEN, Accept: "text/event-stream" } });
      if (resp.ok) await readSSE(resp, onEvent);
    } catch (e) { toast("Lost the event stream."); }
  }

  async function stopRun() {
    try { await api("POST", "/api/chat/cancel", {}); } catch (e) { toast(e.message); }
  }

  // ---- sessions sidebar ---------------------------------------------------------------------

  let searchTimer = 0;
  async function refreshSessions() {
    const q = $("session-search").value.trim();
    let list = [];
    try { list = await api("GET", "/api/sessions" + (q ? "?q=" + encodeURIComponent(q) : "")); } catch (e) { return; }
    const ul = $("session-list");
    ul.textContent = "";
    $("session-empty").classList.toggle("hidden", list.length > 0);
    for (const s of list) {
      const li = el("li", { class: "session-item" + (s.session_id === S.resumeId ? " active" : "") });
      const open = el("button", { class: "session-main", type: "button" },
        el("span", { class: "session-prev", text: s.preview || "(no prompt)" }),
        el("span", { class: "session-meta", text: ago(s.modified_at) + " · " + s.turns + " turn" + (s.turns === 1 ? "" : "s") + " · " + s.tool_calls + " tool" + (s.tool_calls === 1 ? "" : "s") }));
      open.addEventListener("click", () => { openSession(s.session_id); closeSide(); });
      const del = el("button", { class: "session-del", type: "button", "aria-label": "Delete this session", title: "Delete", text: "×" });
      del.addEventListener("click", async (ev) => {
        ev.stopPropagation();
        if (!confirm("Delete this saved session? This cannot be undone.")) return;
        try {
          await api("DELETE", "/api/sessions/" + encodeURIComponent(s.session_id));
          if (S.resumeId === s.session_id) newChat();
          refreshSessions();
        } catch (e) { toast(e.message); }
      });
      li.append(open, del);
      ul.appendChild(li);
    }
  }

  function clearStream() {
    for (const n of Array.from(streamEl.children)) if (n.id !== "welcome") n.remove();
    $("welcome").classList.remove("hidden");
    approvalsEl.textContent = "";
    S.cur = null; S.tools = {};
  }

  function newChat() {
    if (S.run.state === "running" || S.run.state === "waiting_approval") { toast("Stop the current run first."); return; }
    S.resumeId = null;
    clearStream();
    $("sb-usage").textContent = "";
    api("POST", "/api/clear", {}).catch(() => {});
    showTab("chat");
    refreshSessions();
    $("prompt").focus();
  }

  function parseToolMessage(m) {
    try {
      const b = JSON.parse(m.content);
      return { ok: b.ok === true, text: String(b.content === undefined ? "" : b.content) };
    } catch (e) {
      return { ok: !(m.metadata && m.metadata.ok === false), text: m.content || "" };
    }
  }

  async function openSession(id) {
    if (S.run.state === "running" || S.run.state === "waiting_approval") { toast("Stop the current run first."); return; }
    let s;
    try { s = await api("GET", "/api/sessions/" + encodeURIComponent(id)); } catch (e) { toast(e.message); return; }
    clearStream();
    S.resumeId = s.session_id;
    const pending = {};
    for (const m of s.messages) {
      if (m.role === "system") continue;
      if (m.role === "user") {
        if (/^\s*<system-reminder>/.test(m.content)) continue;
        addUser(m.content);
      } else if (m.role === "assistant") {
        if (m.content) { const a = newAssistant(); a.text = m.content; paintAssistant(a, true); }
        for (const c of m.tool_calls || []) {
          let args; try { args = JSON.parse(c.function.arguments); } catch (e) { args = { arguments: c.function.arguments }; }
          pending[c.id] = makeToolCard(c.id, c.function.name, "read", args);
        }
      } else if (m.role === "tool") {
        const t = pending[m.tool_call_id];
        if (t) { const r = parseToolMessage(m); finishTool(t, r.ok, r.text); t.card.open = false; }
      }
    }
    const us = s.usage || {};
    showUsage({ usage: us, tool_calls: s.tool_calls, total_cost_usd: s.total_cost_usd });
    streamEl.scrollTop = streamEl.scrollHeight;
    showTab("chat");
    refreshSessions();
  }

  // ---- tabs ---------------------------------------------------------------------------------

  const loaders = { plan: loadPlan, memory: loadMemory, history: loadHistory, tiers: loadTiers, settings: fillSettings };

  function showTab(name) {
    document.querySelectorAll(".tab").forEach((t) => t.setAttribute("aria-selected", String(t.dataset.tab === name)));
    document.querySelectorAll(".view").forEach((v) => v.classList.toggle("hidden", v.dataset.view !== name));
    if (loaders[name]) loaders[name]();
  }

  // ---- plan and tasks -----------------------------------------------------------------------

  async function loadPlan() {
    const box = $("plan-box"), tb = $("tasks-box");
    try {
      const p = await api("GET", "/api/plan");
      box.textContent = "";
      if (p.pending) {
        const card = el("div", { class: "card" }, el("p", { class: "small muted", text: p.approval_id ? "Waiting for your decision." : "Submitted plan." }), el("pre", { text: p.pending }));
        if (p.approval_id) card.appendChild(el("p", { class: "small", text: "Answer it in the Chat tab, where the approval card is." }));
        box.appendChild(card);
      } else if (p.note) {
        box.appendChild(el("div", { class: "card" }, el("p", { class: "small muted", text: "Approved plan" }), el("pre", { text: p.note })));
      } else {
        box.appendChild(el("p", { class: "muted", text: p.mode === "plan" || p.mode === "active" ? "Plan mode is on; no plan submitted yet." : "No plan. The agent proposes one when it enters plan mode." }));
      }
    } catch (e) { box.textContent = e.message; }
    try {
      const t = await api("GET", "/api/tasks");
      tb.textContent = "";
      if (!t.tasks.length) tb.appendChild(el("p", { class: "muted", text: "No tasks." }));
      for (const k of t.tasks) {
        tb.appendChild(el("div", { class: "card" },
          el("div", { class: "row" }, el("strong", { text: k.title || k.task_id }), el("span", { class: "badge", text: k.status || "" })),
          k.description ? el("p", { class: "small muted", text: k.description }) : null));
      }
    } catch (e) { tb.textContent = e.message; }
  }

  // ---- memory -------------------------------------------------------------------------------

  let editingPath = null;

  async function loadMemory() {
    const rb = $("review-box"), mb = $("memory-box");
    try {
      const m = await api("GET", "/api/memory");
      rb.textContent = "";
      if (!m.pending_review.length) rb.appendChild(el("p", { class: "muted", text: "Nothing waiting for review." }));
      for (const p of m.pending_review) {
        const decide = async (decision) => {
          try { await api("POST", "/api/memory/review/" + encodeURIComponent(p.entry_id), { decision }); toast(decision === "approve" ? "Saved to memory." : "Rejected."); loadMemory(); }
          catch (e) { toast(e.message); }
        };
        const body = [];
        if (p.old_text) body.push(el("p", { class: "small muted", text: "Replaces:" }), el("pre", { text: p.old_text }));
        if (p.content) body.push(el("pre", { text: p.content }));
        rb.appendChild(el("div", { class: "card" },
          el("div", { class: "row" }, el("strong", { text: p.action + " · " + p.scope + " memory" })),
          ...body,
          el("p", { class: "small muted", text: p.rationale || "" }),
          el("div", { class: "actions" },
            el("button", { class: "btn primary small", type: "button", text: "Approve", onclick: () => decide("approve") }),
            el("button", { class: "btn small", type: "button", text: "Reject", onclick: () => decide("reject") }))));
      }
      mb.textContent = "";
      for (const scope of Object.keys(m.scopes)) {
        const sc = m.scopes[scope];
        mb.appendChild(el("h3", { text: scope + " memory", class: "small muted" }));
        if (!sc.entries.length) mb.appendChild(el("p", { class: "muted small", text: "Empty." }));
        sc.entries.forEach((t) => mb.appendChild(el("div", { class: "card" }, el("pre", { text: t }))));
      }
    } catch (e) { rb.textContent = e.message; }
    loadInstructions();
  }

  async function loadInstructions() {
    const list = $("instr-list");
    try {
      const r = await api("GET", "/api/instructions");
      list.textContent = "";
      for (const f of r.files) {
        const label = f.path.length > 60 ? "…" + f.path.slice(-58) : f.path;
        list.appendChild(el("button", { class: "btn small", type: "button", title: f.path, text: label + (f.exists ? "" : " (new)") + (f.writable ? "" : " (read only)"), onclick: () => openInstruction(f) }));
      }
    } catch (e) { list.textContent = e.message; }
  }

  async function openInstruction(f) {
    $("instr-msg").textContent = "";
    let text = "", redacted = false;
    if (f.exists) {
      try {
        const r = await api("GET", "/api/instructions/file?path=" + encodeURIComponent(f.path));
        text = r.content; redacted = r.redacted;
      } catch (e) { toast(e.message); return; }
    }
    editingPath = f.path;
    $("instr-editor").classList.remove("hidden");
    $("instr-path").textContent = f.path;
    $("instr-text").value = text;
    $("instr-text").readOnly = !f.writable || redacted;
    $("instr-save").disabled = !f.writable || redacted;
    $("instr-delete").disabled = !f.writable || !f.exists;
    if (redacted) $("instr-msg").textContent = "This file contains something that looks like a secret, so it is shown redacted and cannot be saved here.";
    else if (!f.writable) $("instr-msg").textContent = "Read only: outside the workspace.";
  }

  async function saveInstruction() {
    try {
      await api("PUT", "/api/instructions/file?path=" + encodeURIComponent(editingPath), { content: $("instr-text").value });
      $("instr-msg").textContent = "Saved.";
      loadInstructions();
    } catch (e) { $("instr-msg").textContent = e.message; }
  }

  async function deleteInstruction() {
    if (!confirm("Delete " + editingPath + "?")) return;
    try {
      await api("DELETE", "/api/instructions/file?path=" + encodeURIComponent(editingPath));
      $("instr-editor").classList.add("hidden");
      loadInstructions();
    } catch (e) { toast(e.message); }
  }

  // ---- history and tiers --------------------------------------------------------------------

  async function loadHistory() {
    const box = $("history-box");
    try {
      const h = await api("GET", "/api/file-history?limit=200");
      box.textContent = "";
      if (!h.entries.length) { box.appendChild(el("p", { class: "muted", text: "Nothing recorded yet." })); return; }
      const rows = h.entries.map((e) => el("tr", null,
        el("td", { text: (e.timestamp || "").replace("T", " ").replace("Z", "") }),
        el("td", { text: e.tool_name || e.history_kind || "" }),
        el("td", { class: "mono", text: e.path || e.command || (e.changed_paths || []).join(", ") || "" }),
        el("td", { text: e.ok === false ? "failed" : "ok" })));
      box.appendChild(el("p", { class: "muted small", text: "Showing " + h.returned + " of " + h.total }));
      box.appendChild(el("div", { class: "tscroll" }, el("table", { class: "tbl" },
        el("thead", null, el("tr", null, el("th", { text: "When (UTC)" }), el("th", { text: "Tool" }), el("th", { text: "Path or command" }), el("th", { text: "Result" }))),
        el("tbody", null, ...rows))));
    } catch (e) { box.textContent = e.message; }
  }

  async function loadTiers() {
    const box = $("tiers-box");
    box.textContent = "Checking…";
    try {
      const t = await api("GET", "/api/tiers");
      box.textContent = "";
      if (!t.configured) {
        box.appendChild(el("p", { class: "muted", text: "Only the local backend is used. To add an operator desktop or Claude as later tiers, define them in ~/.sarina/tiers.json." }));
        return;
      }
      if (t.error) box.appendChild(el("p", { class: "err-text", text: t.error }));
      if (t.pressure) box.appendChild(el("p", { class: "note warn", text: "Resource pressure: " + t.pressure }));
      for (const r of t.tiers) {
        const dl = el("dl", { class: "kv" });
        const add = (k, v) => { dl.appendChild(el("dt", { text: k })); dl.appendChild(el("dd", { text: String(v) })); };
        add("Endpoint", r.base_url || "(default)");
        add("Model", r.model || "(default)");
        add("Data allowed", r.allow_data);
        if (r.api_key_env) add("Key variable", r.api_key_env + (r.api_key_set ? " (set)" : " (not set)"));
        if (r.why) add("Note", r.why);
        box.appendChild(el("div", { class: "card" },
          el("div", { class: "row" }, el("strong", { text: r.name + (r.current ? " (current)" : "") }),
            el("span", { class: "badge " + (r.healthy ? "ok" : "err"), text: !r.enabled ? "disabled" : r.healthy ? "reachable" : "down" })),
          dl));
      }
    } catch (e) { box.textContent = e.message; }
  }

  // ---- settings -----------------------------------------------------------------------------

  const form = $("settings-form");
  const NUM = ["max_turns", "temperature", "timeout_seconds", "compact_preserve_messages"];
  const OPTNUM = ["max_total_tokens", "max_input_tokens", "max_output_tokens", "max_reasoning_tokens", "max_tool_calls",
    "max_delegated_tasks", "max_model_calls", "max_session_turns", "max_total_cost_usd",
    "auto_snip_threshold_tokens", "auto_compact_threshold_tokens"];
  const BOOL = ["allow_write", "allow_shell", "stream_model_responses", "disable_claude_md_discovery", "response_schema_strict"];
  const TEXT = ["model", "base_url", "cwd", "approval_mode", "response_schema_name"];
  const OPTTEXT = ["system_prompt", "append_system_prompt", "override_system_prompt"];

  function applyIndicators() {
    const s = S.settings;
    if (!s) return;
    const perm = $("perm-pill");
    const mode = s.permission_mode;
    perm.textContent = mode + (mode === "read-only" ? "" : s.approval_mode === "auto" ? " · auto-approve" : " · asks first");
    perm.className = "pill perm" + (mode === "read-only" ? "" : mode === "write" ? " write" : mode === "shell" ? " shell" : " both");
    $("sb-cwd").textContent = s.cwd;
    $("sb-cwd").title = s.cwd;
    $("sb-model").textContent = s.model;
  }

  async function refreshState() {
    try { S.settings = await api("GET", "/api/state"); applyIndicators(); } catch (e) { /* shown elsewhere */ }
  }

  async function checkBackend() {
    const pill = $("conn-pill");
    try {
      const b = await api("GET", "/api/backend");
      pill.textContent = b.ok ? "backend ok" : "backend down";
      pill.className = "pill conn " + (b.ok ? "ok" : "bad");
      pill.title = b.ok ? b.base_url : (b.error || "") + " (" + b.base_url + ")";
    } catch (e) {
      pill.textContent = "console offline";
      pill.className = "pill conn bad";
    }
  }

  async function fillSettings() {
    await refreshState();
    const s = S.settings;
    if (!s) return;
    for (const k of NUM) form.elements[k].value = s[k] === null ? "" : s[k];
    for (const k of OPTNUM) form.elements[k].value = s[k] === null ? "" : s[k];
    for (const k of BOOL) form.elements[k].checked = !!s[k];
    for (const k of TEXT) form.elements[k].value = s[k] || "";
    for (const k of OPTTEXT) form.elements[k].value = s[k] || "";
    form.elements.additional_working_directories.value = (s.additional_working_directories || []).join("\n");
    form.elements.response_schema.value = s.response_schema ? JSON.stringify(s.response_schema, null, 2) : "";
    form.elements.api_key.value = "";
    $("settings-msg").textContent = "";
  }

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const msg = $("settings-msg");
    msg.className = "small";
    msg.textContent = "";
    const body = {};
    const f = form.elements;
    for (const k of NUM) { if (f[k].value !== "") body[k] = Number(f[k].value); }
    for (const k of OPTNUM) body[k] = f[k].value === "" ? null : Number(f[k].value);
    for (const k of BOOL) body[k] = f[k].checked;
    for (const k of TEXT) body[k] = f[k].value;
    for (const k of OPTTEXT) body[k] = f[k].value === "" ? null : f[k].value;
    body.additional_working_directories = f.additional_working_directories.value.split("\n").map((x) => x.trim()).filter(Boolean);
    const schema = f.response_schema.value.trim();
    if (schema) {
      try { body.response_schema = JSON.parse(schema); } catch (e) { msg.className = "small err-text"; msg.textContent = "The response schema is not valid JSON."; return; }
    } else body.response_schema = null;
    if (f.api_key.value) body.api_key = f.api_key.value;

    const was = S.settings || {};
    const widen = (body.allow_shell && !was.allow_shell) || (body.allow_write && !was.allow_write) ||
      (body.approval_mode === "auto" && was.approval_mode !== "auto" && (body.allow_shell || body.allow_write));
    if (widen) {
      const what = [];
      if (body.allow_write && !was.allow_write) what.push("change files");
      if (body.allow_shell && !was.allow_shell) what.push("run shell commands");
      if (body.approval_mode === "auto" && was.approval_mode !== "auto") what.push("act without asking you first");
      if (!confirm("This lets the agent " + what.join(" and ") + " in " + (body.cwd || was.cwd) + ". Continue?")) return;
      body.confirm_permissions = true;
    }
    try {
      S.settings = await api("POST", "/api/state", body);
      applyIndicators();
      msg.className = "small ok-text";
      msg.textContent = "Saved.";
      checkBackend();
      refreshSessions();
    } catch (e) {
      msg.className = "small err-text";
      msg.textContent = e.message;
    }
  });

  // ---- composer -----------------------------------------------------------------------------

  const promptEl = $("prompt");

  function autoGrow() {
    promptEl.style.height = "auto";
    promptEl.style.height = Math.min(promptEl.scrollHeight, 200) + "px";
  }

  function addChip(id, label) {
    const chip = el("span", { class: "chip", "data-id": id }, el("span", { text: label }));
    const x = el("button", { type: "button", "aria-label": "Remove pasted text", text: "×" });
    x.addEventListener("click", () => {
      delete S.pastes[id];
      promptEl.value = promptEl.value.replace(new RegExp("\\[Pasted text #" + id + "[^\\]]*\\]\\s?"), "");
      chip.remove();
      autoGrow();
    });
    chip.appendChild(x);
    $("chips").appendChild(chip);
  }

  promptEl.addEventListener("paste", (ev) => {
    const text = ev.clipboardData && ev.clipboardData.getData("text/plain");
    if (!text || text.length <= 500) return;
    ev.preventDefault();
    const id = ++S.pasteN;
    S.pastes[id] = text;
    const lines = text.split("\n").length;
    const token = "[Pasted text #" + id + " +" + lines + " lines]";
    const a = promptEl.selectionStart, b = promptEl.selectionEnd;
    promptEl.value = promptEl.value.slice(0, a) + token + " " + promptEl.value.slice(b);
    promptEl.selectionStart = promptEl.selectionEnd = a + token.length + 1;
    addChip(id, "Pasted text #" + id + " (" + fmtNum(text.length) + " chars)");
    autoGrow();
  });

  promptEl.addEventListener("input", () => {
    autoGrow();
    if (promptEl.value === "/") openSlash(); else if (!promptEl.value.startsWith("/")) closeSlash();
  });
  promptEl.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && !ev.shiftKey && !ev.isComposing) {
      ev.preventDefault();
      $("composer").requestSubmit();
    } else if (ev.key === "Escape") closeSlash();
  });
  $("composer").addEventListener("submit", (ev) => {
    ev.preventDefault();
    const text = promptEl.value;
    if (!text.trim()) return;
    promptEl.value = "";
    autoGrow();
    closeSlash();
    send(text);
  });
  $("stop").addEventListener("click", stopRun);

  // ---- slash palette ------------------------------------------------------------------------

  async function openSlash() {
    const menu = $("slash-menu");
    if (!S.slash) { try { S.slash = await api("GET", "/api/slash-commands"); } catch (e) { S.slash = []; } }
    menu.textContent = "";
    if (!S.slash.length) menu.appendChild(el("li", { class: "muted small", text: "No commands." }));
    for (const c of S.slash) {
      const b = el("button", { type: "button", role: "option" }, el("span", { class: "sname", text: "/" + c.primary }), el("span", { class: "sdesc", text: c.description }));
      b.addEventListener("click", () => { promptEl.value = "/" + c.primary + " "; closeSlash(); promptEl.focus(); autoGrow(); });
      menu.appendChild(el("li", null, b));
    }
    menu.classList.remove("hidden");
  }
  function closeSlash() { $("slash-menu").classList.add("hidden"); }
  $("slash-btn").addEventListener("click", () => { if ($("slash-menu").classList.contains("hidden")) openSlash(); else closeSlash(); });

  // ---- sidebar and boot ---------------------------------------------------------------------

  function openSide() { $("sidebar").classList.add("open"); $("scrim").classList.remove("hidden"); }
  function closeSide() { $("sidebar").classList.remove("open"); $("scrim").classList.add("hidden"); }
  $("open-side").addEventListener("click", openSide);
  $("close-side").addEventListener("click", closeSide);
  $("scrim").addEventListener("click", closeSide);
  $("new-chat").addEventListener("click", () => { newChat(); closeSide(); });
  $("session-search").addEventListener("input", () => { clearTimeout(searchTimer); searchTimer = setTimeout(refreshSessions, 200); });
  document.querySelectorAll(".tab").forEach((t) => t.addEventListener("click", () => showTab(t.dataset.tab)));
  document.querySelectorAll("[data-refresh]").forEach((b) => b.addEventListener("click", () => loaders[b.dataset.refresh]()));
  $("instr-save").addEventListener("click", saveInstruction);
  $("instr-delete").addEventListener("click", deleteInstruction);
  document.addEventListener("keydown", (ev) => { if (ev.key === "Escape") closeSide(); });

  async function boot() {
    // the token travels in the page, so the address bar can be tidied
    if (location.search) history.replaceState(null, "", location.pathname);
    await refreshState();
    checkBackend();
    setInterval(checkBackend, 30000);
    refreshSessions();
    setRunState("idle");
    try {
      const run = await api("GET", "/api/run");
      if (run.state === "running" || run.state === "waiting_approval") {
        if (S.settings && S.settings.active_session_id) S.resumeId = S.settings.active_session_id;
        attachToRun(run);
      } else if (S.settings && S.settings.active_session_id) {
        openSession(S.settings.active_session_id);
      }
    } catch (e) { addError("Console", e.message); }
  }

  boot();
})();
