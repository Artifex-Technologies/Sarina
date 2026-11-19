# CLAUDE.md: Sarina

Guidance for Claude Code (and any AI agent) working in this repository. The standard layout every Artifex repository's
CLAUDE.md follows: what this is, build and test, rules, contracts, open work, know-how.

## 1. What this is

Sarina is a local-model agent built on the Claude Code agent architecture: she runs against local, open-source models over
an OpenAI-compatible API, optional on Labyrinth OS (from Agora) and part of LykanOS. The core is **Rust** (`rust/`, the
binary `sarina-rs`, installed as `/usr/bin/sarina` since 2026-11-18), ported from the clean-room Go core (`go/`), which
stays in the tree as the reference and rollback copy until about 2026-11-25. Also Rust, each in its own crate: the ML and
voice helper (`packaging/helpers`, `sarina-helpers`; its Python original `python/sarina_helpers/` is the rollback copy until
then), the spoken-question programs (`packaging/voice`, `sarina-voice`), the voice service (`packaging/voiced`,
`sarina-voiced`) and `sarina-tone` (a second binary of `rust/`). It is a fork of
[ClawCode](https://github.com/HarnessLab/claw-code-agent); keep that attribution intact. Sarina touches every other Artifex
repository; the integration detail is [`docs/ECOSYSTEM.md`](docs/ECOSYSTEM.md).

## 2. Build and test

The core is Rust (stable, 1.87 or newer), a cargo workspace in `rust/` with one crate, `rust/sarina`, and two binaries:
`sarina-rs` (installed as `sarina`) and `sarina-tone`. Cargo goes through `rust/scripts/nicecargo.sh` (nice 15, so a build
never competes with a game or the local model) and under a memory cap (a test run once took 26 GB):

```bash
CAP="systemd-run --user --scope -q -p MemoryMax=8G -p MemorySwapMax=0"
$CAP rust/scripts/nicecargo.sh build -j4 --release --locked --bin sarina-rs --bin sarina-tone
$CAP rust/scripts/nicecargo.sh test -j4 --locked        # offline: no test may need a network or a model backend
$CAP rust/scripts/nicecargo.sh clippy -j4 --locked --all-targets -- -D warnings
rust/scripts/nicecargo.sh fmt --all --check
bash evals/install-dev.sh                               # the build the evals measure, as ~/.local/bin/sarina
```

Put `CARGO_TARGET_DIR` outside the tree for throwaway builds and delete it afterwards. Never set `RUST_MIN_STACK` around
cargo (rustc reads it too). The other Rust crates build the same way from their folders (`packaging/helpers`,
`packaging/voice`, `packaging/voiced`). CI (`.github/workflows/rust.yml`, `helpers.yml`) runs fmt, clippy, the tests and
the differentials; docs changes do not start it.

**Go stays as the reference until it is deleted (~2026-11-25).** `go/` (Go 1.27+, standard library only) is what the Rust
was proved against: its parity tests write the fixtures in `testdata/parity/` and `rust/sarina/src/tools/defs.json`, and
`rust/parity/*.py` run both builds side by side (docs/RUST_PORT.md "Running the tests"). A bug the port finds is fixed in
both until then (`cd go && go vet ./... && go test ./...`); nothing installed runs the Go build.

Chatting needs an OpenAI-compatible backend (`OPENAI_BASE_URL`, `OPENAI_API_KEY`,
`OPENAI_MODEL`); the tests do not. Ollama's default 4096-token context is too small
for Sarina: she needs at least 8k (16k recommended).

The old Python (`sarina/`, its tests and the top-level
`pyproject.toml`) was deleted on 2026-11-11: nothing installed it, and dead code is
a security risk (`docs/PROVENANCE.md`). The ML and voice helper speaks a JSON-lines protocol
(`python/sarina_helpers/PROTOCOL.md`; the core's side is `rust/sarina/src/helpers.rs`); `SARINA_HELPERS=python` goes back
to the Python package during the rollback week. `tests/` holds only the tests of the voice-training tools in
`tools/voice_clone/` and of `blackarch_compat/` (`python3 -m unittest discover -s tests` from the repository root); the
evals are Python too (`evals/run_evals.py`).

Entry points: `sarina <command>`; the binary also answers to `sarina-gui` and
`sarina-service` (the Arch package installs those as links).

## 3. Rules

- **Push every commit as you make it**; do not ask first. Pull (`git pull --rebase`) before each commit, stage by explicit
  path (never `git add -A`), and check the diff names no host name, network address, SSID, MAC address, account name or
  the owner's name. Docs are written in the first person as the owner ("I decided"), and say "Sarina", not "her".
- **Plans, training notes, owner steps and eval reports** go in `docs/plans/` (private and git-ignored; ask the owner),
  never in the public docs. Public docs say what is built and how to use, build and test it; a short "Known limitations"
  section may describe a limit, never plan it.
- **Tests never touch the owner's real Sarina**: not the real model, vault, sessions, memory, notes, clipboard, voice or
  services (8899, 8765, Heimdall on 8930, the Argus console on 44700, Limbo's sockets). They run offline, with made-up homes
  and workspaces, stand-in programs and loopback servers; the evals run with `SARINA_TEST=1` (the write guard) and a
  made-up owner name. Never raise Sarina's access tier, write `SARINA_ACCESS_TIER`, or change sudoers or Heimdall's
  configuration: only the owner does that, at a terminal.
- **One thing on the graphics card at a time**, and every build or test under the memory cap (section 2).
- **A bug the port finds is fixed in Go and Rust both** while the Go code exists, each with a test that failed before.
- **A change another repository relies on** (a format, port, path, command name or `--json` key, a new run-time dependency)
  updates `docs/CONTRACTS.md` and `docs/ECOSYSTEM.md` in the same commit, is flagged `CONTRACT:` in the commit message, and
  gets a handoff note in `docs/plans/LOG.md` (private). A contract marked **proposed** in ECOSYSTEM.md is not settled: do
  not write code against it until the owning repository agrees.
- **Match the surrounding code.** Look at a neighbouring module first: a module
  doc comment saying what it is (and which Go package it came from) and what it must never do, small public surface,
  errors as values, temporary folders and real loopback servers or sockets in tests
  rather than mocks.
- **Few crates, pinned exactly.** A new crate in `rust/` needs a real justification, an exact version (`=x.y.z`, the
  lock file pins the rest) and preferably the one Heimdall already uses; for the distro it must build from Arch's
  toolchain: flag it `CONTRACT:`. Until the Go code goes, `go/` stays standard library only.
- **Test what you add, offline.** Include the failure and hostile-input cases; this
  codebase treats anything from a model, a guest, a sibling repo or the network as
  untrusted data.
- **Operator-owned config is never read from a workspace** (`tiers.json`,
  `council.json`, policy files): a workspace could otherwise ship its own
  destinations or executables. A broken config must narrow behaviour, never widen it.
- **Sending text off the machine** goes through `tiers.Sanitize` (secrets removed,
  capture dumps refused) and an operator confirmation. A model-callable tool that
  does so must ask on every call through the confirm channel (`tools::registry::Confirm`, Go's
  `tools.RuntimeOperatorConfirm`), never the `Approver` (which can carry a session-long grant), and does nothing online when no
  one can be asked. `fleet-launch` stays CLI-only; a test enforces it.
- **Licence: MPL 2.0 for the current tree (see `LICENSE`); read `NOTICE.md`.** Do not
  copy code from the unlicensed upstream (HarnessLab/claw-code-agent); new code is
  written from the design.
- **Never commit secrets.** No tokens, keys, or engagement/capture data.
- **Hardware writes need a human.** `docs/ECOSYSTEM.md` section 2: Sarina never flashes the Argus hardware and never applies
  firmware patches on her own.
- **Untrusted binaries are opened only inside Limbo** (owner rule, 2026-10-10):
  decompiling, disassembling, running. Neither Sarina nor Heimdall has a
  decompiler on the host.

### Licensing and provenance (read before editing)

The old agent core was derived from ClawCode, which has no license. It has been
replaced by a clean-room Go rewrite and the current tree is MPL 2.0-licensed (chosen
2026-10-06). See [`docs/PROVENANCE.md`](docs/PROVENANCE.md) for the audit. The Rust core (`rust/`, 2026-11) is ported from
that clean-room Go code, which is ours, never from ClawCode.

- **Do not add features to a derived file** (>=25% shared in the table). Put new
  code in a new module, with at most a one-line hook into the derived file.
- **Replacements are clean-room**: written from a behaviour spec, never ported
  or paraphrased from the existing file.
- Do not copy code from ClawCode or the Claude Code npm source into this repo.

## 4. Contracts

- What other repositories rely on from Sarina, one section each: [`docs/CONTRACTS.md`](docs/CONTRACTS.md) (the package's
  inputs, session files, the integration service on 8899, ports, the agent fleet, menu categorisation, the voice socket,
  the spoken-question programs, `sarina.env`, the decompile profile); the full wording in [`docs/ECOSYSTEM.md`](docs/ECOSYSTEM.md).
- What Sarina uses from others: Argus_App's console API (`docs/SARINA_API.md` there), Limbo's agent socket
  (`docs/QUARANTINE.md` there), Heimdall's review, grant, rule-draft, containment and trust surfaces (its docs), and
  Labyrinth_OS's `sarina-git` recipe: the table at the end of `docs/CONTRACTS.md`.

## 5. Open work

docs/plans/BACKLOG.md and docs/plans/LOG.md (private; ask the owner).

## 6. Know-how

### Architecture map

Each Rust module's first doc line names the Go package it was ported from; the Go names below are the same packages.

```
rust/
  sarina/src/bin/    sarina-rs.rs (the entry: names, Heimdall's lock, dispatch), sarina-tone.rs
  sarina/src/cli/    the command table (`COMMANDS`, Go's summaries) and the commands; entry.rs, chat, service, gui, doctor
  agent/ agents.rs   the turn loop (compaction, budgets, plan mode, answer checks); read-only subagents
  llm/               OpenAI-compatible and Anthropic clients, local-model quirks (over gohttp: TLS through rustls)
  tools/             registry, permissions, builtins; defs.json holds every tool definition the model sees
  tiers/             compute tiers: local, then a bigger machine, then Claude; Sanitize; Heimdall's grant gate
  council.rs         several models, compared; the council command and council_ask tool
  guard.rs heimdall.rs   the shared scan gate; the optional Heimdall second opinion and grant requests
  memory/ skills/ mcp/ policy.rs   curated memory, operator skills, MCP (trust-gated), hooks policy
  session/ sessionsearch.rs        session files (Heimdall reads them), search
  limbo.rs           limbo_* tools and commands (validate in a disposable guest)
  chymaera/ fleet/   Argus App console client, firmware advisories, capture analysis; the agent fleet commands
  desktop.rs         application-menu categorisation (reports by default)
  doctor.rs          read-only checkup of the whole setup
  service.rs gui/ runkit.rs   integration API (:8899) and operator GUI (:8765) over gohttpd (Go's HTTP server)
  helpers.rs         bridge to the ML/voice helper (sarina-helpers; the Python one with SARINA_HELPERS=python)
  forge/             agents Sarina writes, run only inside Limbo guests (docs/AGENT_FORGE.md)
  vault/ daily/      her linked Markdown notes; the day's note (Daily/YYYY-MM-DD.md)
  insight/           what she learns about the person she talks with (`sarina reflect`)
  embed.rs vecindex.rs route.rs   local embeddings (Ollama); compressed vector index; sorting a spoken question
  cryptotools/ stego/  cryptography in code (identify, decode, classical ciphers); read-only stego checks
  trust.rs           her behaviour files checked against Heimdall's signed list
  sysinfo/           system_status and crash_detail (read-only), system_fix (named, reversible repairs)
  reverse.rs         reverse-engineering tool (REA with Ghidra, sandboxed; BSim `similar`)
  quantum.rs trachtenberg.rs   exact quantum-circuit simulator; Trachtenberg arithmetic, step by step
  tone.rs            how something was said (pace, pitch) for sarina-tone
  testguard.rs       under SARINA_TEST=1 nothing writes outside the test's folders
  go*.rs, gotext/    Go's behaviour where the answers must match byte for byte (JSON, flags, time, paths, math, ...)
  sarina/tests/      parity_*.rs against testdata/parity/, sarina_rs.rs (the binary), fleet_invariant.rs
  parity/            cli-, service-, gui- and tone-differential.py: Go and Rust side by side
  fuzz/              cargo-fuzz targets (nightly, not part of the normal build)
go/                  the Go core, the reference until ~2026-11-25 (same package names; notes/ and NOTES.md: its design notes)
python/sarina_helpers/   the Python ML and voice helper, kept for the rollback week (goes ~2026-11-25)
packaging/helpers/   sarina-helpers (Rust): the ML and voice helper (serve, memory-search, voice-chat); differential with the Python
packaging/voice/     sarina-voice (Rust): labyrinth-ask-sarina, the check-up, the voice client
packaging/voiced/    sarina-voiced (Rust): the voice service (Chatterbox through ONNX Runtime)
packaging/arch/      source of truth for the distro's systemd unit + sysusers + PKGBUILD
packaging/skills/    the packaged skills (installed in /usr/share/sarina/skills)
packaging/limbo/     the decompile profile's image and wrapper (limbo-decompile)
docs/ECOSYSTEM.md    cross-repo integration contract
docs/RUST_PORT.md    the port: what was proved, the differences on purpose, what goes with the Go deletion
docs/spec/           the clean-room behaviour specs the Go core was written from
```

Adding a CLI command: a function in `rust/sarina/src/cli/` (or the module it belongs to) and a row in `cli::COMMANDS`
(name, summary, function). Adding a tool: its definition in `tools/defs.json` and its handler in the module's `tools()` list; a
tool must declare a `Class`, and the model sees its definition on every request, so keep it small. Until the Go code is
deleted, `defs.json` is written from the Go tools (`SARINA_PARITY_UPDATE=1 go test ./cmd/sarina -run ParityToolDefs`) and
Go's `TestToolDefinitionFootprint` caps the total, so a new tool is added in both.

### Names and gotchas

- Commands and modules named `chymaera` (`chymaera-console`, `chymaera-analyze`, `chymaera-device-info`,
  `chymaera-merge-report`; `rust/sarina/src/chymaera`, `go/chymaera`) are the Argus App and Argus firmware (Flipper Zero)
  tools; the names predate the renames (Chymaera OS is LykanOS since 2026-11-07) and stay, because scripts call them.
- Older notes and history use the old repository names (Chymaera_Sentry is Heimdall, Chymaera_Linux and Chymaera_OS are
  LykanOS, Chymaera_Zero_App and _Firmware are the Argus repositories).
- Tool calls that write files can turn a literal `\u....` escape into the character itself (it happened to a test's
  expected string); build such strings from parts or check them with grep.
- A mutation script that restores a file with `cp` or `mv` leaves it older than the last build, so cargo does not rebuild:
  touch the file after restoring it.
- The timed client fixtures take about a minute of the Go parity run; keep fixtures small and bounded (no input that makes
  Go allocate by a count it reads).
- The service and GUI differentials do not run in a network namespace of their own (their servers take ports from the
  kernel, so nothing reaches 8899 or 8765); a scenario that dials a fixed port needs the CLI differential's `run_case`
  wrapper first.
- Sarina's eval reports and `ladder.json` are written to `Labyrinth_OS/docs/plans/records/evals/` (the owner's private
  plans; `SARINA_EVAL_REPORTS` names another folder).
