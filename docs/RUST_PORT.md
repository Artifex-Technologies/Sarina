# The Rust port of Sarina's Go core

Everything of ours that can be Rust becomes Rust: security first, then stability, then speed. Sarina's Go core (`go/`,
48,159 lines in 212 files, 27,665 lines of tests) is the last big Go program in the ecosystem. Heimdall
(`Heimdall/docs/RUST_PORT.md`, `RUST_CLI.md`) and Limbo (`Limbo/docs/RUST_PORT.md`) set the method, and this port follows
it: the Go behaviour is recorded as fixtures, the Rust code is checked against them byte for byte, anything that parses
outside input gets properties and fuzz targets, then whole binaries run side by side, and a piece switches only when it is
equal.

## Status

| Step | What | State |
| --- | --- | --- |
| 0 | this inventory, the order, the crates, the risks, the switch and the proof plan | **done** (2026-11-11) |
| 1 | the workspace (`rust/`), the fixture writer (`go/internal/parity`), the first leaves with fixtures, properties and fuzz targets | **in progress**: shellparse, the bash guards, ValidateArgs, testguard, pyfmt, flagx, version (see "Ported"); CI since 2026-11-11 |
| 2 | the file formats: session files, config loaders, llm message types, vecindex, trust, policy, analysiscache | open |
| 3 | the clients: Heimdall review, embed, llm (OpenAI-compatible and Anthropic), Limbo, MCP, helpers | open |
| 4 | tools, memory, tiers, the agent loop, runkit, service, cli chat/resume, gui | open |
| 5 | the optional packages (crypto, stego, quantum, vault, daily, insight, sysinfo, doctor, forge, ...) | open |
| switch | `sarina-rs` as `/usr/bin/sarina`, the Go build at `/usr/lib/sarina/sarina-go` for what is not ported | open |

## Inventory

Lines are non-test Go. "Outside" is what the package touches beyond memory: files (F), processes (P), network (N), the
model API (M), MCP servers (MCP), the tools it gives the model (T).

| Package | Lines | What it does | Imports (ours) | Outside |
| --- | ---: | --- | --- | --- |
| `internal/version` | 7 | the version string (= `VERSION`) | | |
| `tools/shellparse` | 781 | splits a shell command line into its simple commands (for the bash guards) | | |
| `flagx` | 37 | flags anywhere among positionals (over Go's `flag`) | | |
| `testguard` | 60 | under `SARINA_TEST=1`, refuses writes outside the test's folders | | F (lstat, readlink) |
| `chymaera/internal/pyfmt` | 138 | Python's `repr`/`str`/truthiness for JSON values | | |
| `trust` | 167 | behaviour files against Heimdall's signed list (ed25519) | | F |
| `vecindex` | 397 | 2/4-bit compressed vector index (TurboQuant), own file format | | F |
| `heimdall` | 284 | `POST /v1/review` second opinion; the grants cap file | | N, F |
| `embed` | 74 | texts to vectors through Ollama | | N |
| `voice` | 262 | Piper TTS: espeak-ng phonemes, ONNX model | | P, F, onnxruntime |
| `config` | 569 | model and runtime config, env handling, tolerant loaders for saved sessions | heimdall | F (env) |
| `guard` | 115 | the one text gate: invisible Unicode, override phrases, credential shapes | heimdall | N (opt-in) |
| `tone` | 540 | how something was said (pace, pitch), running averages | testguard | F |
| `chymaera/analysis`, `console`, `firmware` | 820 | Argus capture statistics, the console's NDJSON client, firmware advisories | pyfmt | N (loopback), P (git) |
| `llm` | 1,790 | OpenAI-compatible and Anthropic clients, local-model quirks, repair and normalising | config | M |
| `policy` | 370 | operator tool policy: deny-list, trust switch, budgets | config | F |
| `agents` | 345 | read-only sub-agent profiles (built-in and markdown) | guard | F |
| `session` | 962 | the message model, the secret redactor, the session file Heimdall reads, migrations | config, llm | F |
| `tools` | 6,049 | registry, permissions, the bash guards, file tools, exec, procs, verify, todo, plan | config, llm, shellparse | F, P, T |
| `analysiscache` | 97 | content-hash cache on disk | session | F |
| `sessionsearch` | 249 | BM25 phrase search over saved sessions | session | F |
| `skills` | 1,382 | named prompts: format, discovery, the skill tool, long-skill search | guard, llm, tools, vecindex | F, T |
| `mcp` | 1,228 | MCP client for stdio servers, trust-gated | guard, version, llm, tools | P, MCP, T |
| `reverse` | 534 | REA + Ghidra in bubblewrap | heimdall, llm, tools | P, T |
| `tiers` | 2,133 | compute tiers (local, desktop, Claude), sanitising, hand-offs | config, heimdall, llm, session, tools | N, P, M |
| `memory` | 943 | curated memory, every write through the gate | guard, llm, session, sessionsearch, testguard, tools | F, T |
| `agent` | 4,868 | the turn loop: model calls, tool dispatch, budgets, compaction, plan mode, resume | agents, config, llm, memory, session, skills, tools | M, F, P |
| `runkit` | 326 | agent from settings, run results, secret scrubbing (service and gui) | agent, agents, config, llm, policy, session, tiers, tools, trust | |
| `cli` | 1,955 | the command table, chat, resume | most of the above | all |
| `service` | 719 | the integration API on :8899 (Heimdall's advisory, scripts) | agent, cli, config, runkit, session, tiers | N (listen) |
| `gui` | 2,214 | the operator console on :8765 | agent, cli, config, memory, runkit, service, session, tiers, tools | N (listen), P |
| `council` | 962 | several models compared | cli, config, flagx, llm, tiers, tools | M, N |
| `limbo` | 729 | validation runs through Limbo's agent socket | cli, config, llm, tools | N (unix), T |
| `forge` | 835 | agents she writes, run only in Limbo guests | cli, limbo, llm, tools | F, T |
| `helpers` | 254 | the Python ML/voice helper (JSON lines) | cli | P |
| `vault`, `daily`, `insight`, `route` | 3,541 | linked Markdown notes, the day's note, what she learns about me, sorting a spoken question | cli, embed, guard, llm, sysinfo, testguard, tools, vecindex | F, N (embed), M |
| `sysinfo` | 1,252 | `system_status`: fixed read-only probes | cli, embed, heimdall, llm, route, skills, tools | P, N |
| `doctor` | 894 | read-only check-up of the whole setup | cli, config, heimdall, limbo, llm, mcp, policy, session, skills, tiers, tools, trust | F, N, P |
| `desktop` | 1,386 | application-menu categorisation | analysiscache, cli, helpers | F, P |
| `cryptotools`, `stego` | 5,137 | crypto and hidden-data checks in code (opt-in) | cli, llm, skills, tools | F, P |
| `quantum`, `trachtenberg` | 845 | exact circuits, Trachtenberg arithmetic (opt-in) | cli, llm, tools | |
| `chymaera`, `chymaera/fleet` | 1,572 | the chymaera-* and fleet-* commands | cli, helpers, the three above | F, P, N |
| `cmd/sarina`, `cmd/sarina-speak`, `cmd/sarina-tone` | 208 | entry points (the `sarina-service`/`sarina-gui` names, Heimdall's containment lock first) | everything | |
| `helpers/fakehelper`, `internal/fakeclaude` | 129 | test stand-ins (not ported: the Rust tests get their own) | | |

### Dependency levels (leaves first)

```
0  version shellparse flagx testguard pyfmt trust vecindex heimdall embed voice
1  config(heimdall) guard(heimdall) tone(testguard) chymaera/{analysis,console,firmware}(pyfmt)
2  llm(config) policy(config) agents(guard)
3  session(config,llm) tools(config,llm,shellparse)
4  analysiscache sessionsearch skills mcp reverse tiers
5  memory
6  agent
7  runkit
8  cli
9  service council limbo vault helpers cryptotools stego quantum trachtenberg chymaera/fleet
10 gui doctor forge desktop route chymaera
11 sysinfo
12 daily insight
13 cmd/sarina
```

### Third-party code

Go: one module, `github.com/yalue/onnxruntime_go` (MIT), for `voice` only. Everything else is Go's standard library, so the
Rust side needs crates where Rust's std has nothing. Exact versions, one lock file, cargo-audit in CI, the same crates the
siblings already carry where possible:

| Need (Go std) | Crate | Licence | Where |
| --- | --- | --- | --- |
| `regexp` (RE2) | `regex` with Go's ASCII `\b \w \d \s` written out (Heimdall's `rules.rs` method) | MIT/Apache-2.0 | guard, tools, skills, vault, many |
| `encoding/json` | none: our own `gojson` (Heimdall/Limbo), Go's errors, escaping and field order; `serde_json` in tests only | | everywhere |
| `crypto/sha256`, ed25519, `crypto/rand` | `sha2`; `ring`; getrandom through `rustix`/`libc` | MIT/Apache; ISC-style | trust, session ids, caches |
| `net/http` client and server, TLS | our own HTTP/1.1 (Limbo's `http.rs`, Heimdall's API); `rustls` (ring, TLS 1.3/1.2) with the system's roots for the online tiers | Apache/MIT/ISC | llm, tiers, council, service, gui |
| `os/exec`, `os/signal` | std + Limbo's `goexec` (Go's error texts), signalfd via `libc` | | tools, mcp, helpers, forge |
| `compress/zlib`, `archive/tar` | `flate2` (rust backend); a small own tar writer | MIT/Apache | stego, forge |
| `image/png|jpeg|gif` | `png`, `jpeg-decoder` (or `zune-jpeg`), `gif` | MIT/Apache | stego (opt-in) |
| `math/big`, `math/cmplx` | own (only add and multiply are used), own complex type | | trachtenberg, quantum, cryptotools |
| `math/rand` (seeded) | none: Go's generator reproduced exactly (the 607-value table from a fixture) | | **vecindex** (rotation from the saved seed), quantum shots, a cracker |
| hashes and certificates in cryptotools (md5, sha1, sha3, hmac, rsa, ecdsa, x509, pem) | RustCrypto (`md-5`, `sha1`, `sha3`, `hmac`, `rsa`, `p256`/`p384`, `x509-parser`) or stay Go | MIT/Apache | cryptotools only (opt-in): decide when it comes up |
| onnxruntime | `ort` is heavy; a dlopen of `libonnxruntime.so` with the few C calls used | | voice (last) |

## Port order

Leaves first, and within a level what is pure before what touches the outside, so every step can be proved by fixtures
before anything with side effects is attempted:

1. **Pure leaves** (step 1): `internal/version`, `tools/shellparse`, the pure checks in `tools` (`CheckDestructive`,
   `CheckPrivileged`, `ValidateArgs`), `pyfmt`, `flagx`, `testguard`. The bash guards are the security-critical part of
   the tool sandbox, so they go first and get the heaviest proof.
2. **Formats** (step 2): llm message types, `config` (its tolerant loaders read saved sessions), `session` (the file Heimdall
   reads), `session/redact`, `vecindex` (with Go's `math/rand`), `trust`, `policy`, `analysiscache`, `guard`'s detectors,
   `agents` and `skills` file formats, `sessionsearch`, `llm`'s pure parts (`repair.go`, `normalize.go`).
3. **Clients** (step 3): `heimdall`, `embed`, `llm` (streaming), `limbo`, `mcp`, `helpers`.
4. **The core** (step 4): `tools` (registry, file tools, exec), `memory`, `tiers`, `agent`, `runkit`, `service`, `cli`,
   `gui`. `service` is the always-on part (`sarina-service.service`), so it is the first binary-level switch.
5. **Optional packages** (step 5), each behind its own switch already: `cryptotools`, `stego`, `quantum`, `trachtenberg`,
   `vault`/`daily`/`insight`/`route`, `sysinfo`, `doctor`, `desktop`, `forge`, `reverse`, `chymaera`, `council`,
   `voice` last.

## The risky parts

- **The agent loop** (`agent`, 4,868 lines): budgets, compaction, plan mode, the "carry on" nudge, voice rules, tool
  dispatch and resume interact. It changes often (it is being edited the night this was written), so it is ported last of
  the core and proved by **differential runs against a scripted fake model** (the `integration-sarina.sh` idea, without
  Limbo): the same scripted model answers, the same tool results, both binaries; compared are every request body sent to
  the model, the session file after each turn and the output. Never against the real model.
- **Tool sandboxing**: `CheckDestructive`/`CheckPrivileged` decide what the bash tool refuses at every tier;
  `ResolveInRoot`, `secretGuard`, `privateGuard`, `IsSecretPath` decide what the file tools may touch. A difference here is
  a hole, not a cosmetic bug: they get fixtures on every Go test input plus generated commands, a fuzz target each, and the
  property that the Rust check never allows what Go refuses.
- **The permission tiers / access ladder**: `SARINA_ACCESS_TIER` (capped by Heimdall's root-only `grants.json`),
  `--allow-write`/`--allow-shell`, `AllowPrivileged` (never set), `RuntimeOperatorConfirm` for anything leaving the
  machine. The Rust side must reach the same `Permissions` from the same env, files and flags: one fixture of every
  combination (tier 0-4, the cap missing, damaged, lower, higher; the flags), and a test that no flag or env raises past
  the cap.
- **Session file compatibility**: `/var/lib/sarina/sessions/*.json` is read by Heimdall (its detectors) and resumed by
  `sarina resume`. The Rust writer must write what Go writes byte for byte (field order, `OrderedMap`, Go's escaping,
  `null` against `[]`, the redactor, migrations of retired tools), and read every file Go reads (the tolerant loaders).
  Proof: Go fixtures of made-up sessions, then a read-only run over a **copy** of real sessions (never committed: they
  are private) where both binaries load and re-marshal each file and the bytes are compared.
- **Go's `math/rand`** for `vecindex`: the rotation of a saved index comes from its seed; a different generator would make
  every saved index return nonsense without an error.
- **Text rules** (`guard`, the redactor, the voice rules): Go's regexp is ASCII for `\b \w \d \s`; Heimdall's regex
  decision applies (keep Go's meaning, differential corpus).
- **Containment**: `cmd/sarina` checks Heimdall's lock before anything else; `sarina-rs` must do the same check before
  it runs or hands over any command.

## The switch

Limbo's way: one Rust binary, `sarina-rs`, installed as `/usr/bin/sarina` (and its `sarina-service`, `sarina-gui` names);
the commands it has ported it runs itself, **every other command is handed unchanged to the Go build** at
`/usr/lib/sarina/sarina-go` (or `$SARINA_GO_BINARY`), with the containment check first in both. A command line that is
not UTF-8 goes to Go. Session files, the vault and every state file are shared, in the same formats, so going back is
reinstalling the previous package. `sarina-service.service` is switched by the package (as Limbo's), once its
differential run shows no difference. Commands move from Go to Rust one at a time; the Go build is dropped when nothing
reaches it any more (dead code is a risk).

## The proof plan

1. **Go fixtures**: each ported package gets a `parity_test.go` (new test files only; the Go code is not changed) that
   records what Go answers on the Go tests' own inputs plus generated ones into `testdata/parity/<name>.json`
   (`go/internal/parity`). `SARINA_PARITY_UPDATE=1 go test ./... -run Parity` writes them; without it the Go test fails
   when a fixture is out of date, so they cannot drift from the Go code. The Rust tests (`rust/sarina/tests/parity_*.rs`)
   run the same files and compare byte for byte (texts that are not UTF-8 travel as hex).
2. **Properties and fuzzing**: every parser of outside input (shell commands, tool arguments, session files, MCP and model
   answers, vecindex files) gets a cargo-fuzz target (`rust/fuzz`, nightly) whose body also runs on stable in
   `tests/properties.rs`, plus proptest properties (no panic, the guards never weaker than their parts).
3. **Differential runs** of whole binaries: the CLI commands on the same arguments, env and files; the service with a
   scripted fake model on the same requests; `sarina resume` and the session writer on copies of sessions.
4. **Switch** only when equal; the differences kept on purpose are listed below as they come.

## Ported

| Rust (`rust/sarina/src`) | Go | Fixture | Cases |
| --- | --- | --- | ---: |
| `shellparse.rs` | `tools/shellparse` | `shellparse.json` | 7,475 |
| `tools/destructive.rs`, `gopath.rs` | `tools/destructive.go`, `realRoot` and `within` (`fsutil.go`), `filepath.Rel` | `destructive.json` | 10,154 (about 46,000 verdicts) |
| `tools/schema.rs`, `goany.rs`, `gotext/parsefloat.rs` | `tools/schema.go` (`ValidateArgs`), `strconv.ParseFloat` | `schema.json` | 2,073 |
| `testguard.rs`, `gopath.rs` | `testguard`, `filepath.EvalSymlinks`, `Abs`, `Clean`, `Join`, `Dir`, `Base` (on a made-up folder tree) | `testguard.json` | 405 |
| `pyfmt.rs` | `chymaera/internal/pyfmt` | `pyfmt.json` | 681 |
| `flagx.rs`, `goflag.rs` | `flagx` and Go's `flag` package (syntax, values, error texts, the usage on an error) | `flagx.json` | 1,513 |
| `version.rs` | `internal/version` (VERSION read at build time; a test checks the Go constant) | | |
| `gotext/` | Go's `%q`, `%v` of a float64, `strings.ToLower`, `TrimSpace`, `Atoi`, `IsPrint` (Unicode 17 tables from Heimdall's generator) | through the above | |

How each corpus is made (the Go side, `go/internal/parity` and each `*parity_test.go`):

- **Shell corpus** (`parity.ShellCorpus`): every string literal of `shellparse_test.go`, `destructive_test.go`, `bash_test.go`
  and `exfil_test.go` (read with go/parser, so a new test input joins the fixture by itself), hand-written hostile lines
  (bytes that are not UTF-8, NUL, every unterminated form, ANSI-C escapes running past their width), every prefix of four
  long lines, the nesting limit (40) from both sides for each way of nesting, here-documents whose body is read inside a
  substitution for a command of an outer frame, and generated lines (fixed seeds).
- **Guard corpus** (`parity.GuardCorpus`): lines from the guards' own vocabulary: chains of wrappers with their options,
  shells with every flag form, interpreters with their code flags and risky or harmless code, git's subcommands and
  flags, the script depth limit (6) and the wrapper depth limit (8) from both sides, fork bombs and downloads into shells
  by pipeline, and short-flag lookups on words with a run-time part.
- `CheckDestructive` is recorded under five workspace roots: none, a folder that exists on no machine (so the fixture is
  the same everywhere), the same with a trailing slash, a relative one, and `/`.
- **Mutation runs**: deliberate changes to the Rust code (a limit off by one, an escape's width, a flag letter, a missing
  rounding bit, ...) must make the fixture fail. Shell parser 11 of 12, guards 13 of 14, schema and ParseFloat 15 of 17
  caught (after the corpus was widened for the ones that first got through); each change left was checked to be
  equivalent (it cannot change any answer). One gap is known: the guards' fixture uses workspace roots that do not
  exist, so dropping the `realRoot` half of `outsideRoot` (a workspace reached through a link) is not caught there; the
  `EvalSymlinks` fixture on a made-up tree (next, with `testguard`) covers it.
- **Properties and fuzzing**: `rust/fuzz` targets `shellparse`, `bash_guards`, `parse_float` (cargo-fuzz, nightly; not run on
  this PC, which has no nightly toolchain); their bodies run on stable in `tests/properties.rs` over random bytes,
  shell-shaped lines and every fixture line with bytes changed. What must hold: no panic; a line that does not parse is
  refused by both guards; a privileged or destructive program, or one built at run time, is refused wherever it sits;
  `sudo` before anything is refused; ParseFloat agrees with Rust's parser on plain decimals and round-trips every finite
  float; ValidateArgs never adds a key and is idempotent.

## Differences from Go, on purpose or accepted

- **Map order.** Go walks the pipelines of a command line (`CheckDestructive`) and an object's properties
  (`ValidateArgs`) in map order, which is random: when two pipelines are refused for different reasons, or two properties
  are wrong, Go names either one. The port names the first (source order; sorted property names). The fixture runs such
  lines 400 times and accepts any reason Go gave. Whether something is refused never depends on the order.
- **Found on the way** (not Sarina's): Limbo's port of the flag package (`rust/limbo/src/cli/flags.rs`, where `goflag.rs`
  comes from) kept a bool flag's old value after an invalid one (`-w=x`); Go stores false before it reports the error.
  Harmless there (the command stops on the error); fixed here.
- **Paths**: `realRoot` follows links with a port of Go's `EvalSymlinks` (not `std::fs::canonicalize`, which differs for
  relative paths and missing parts).

## Running the tests

```bash
cd rust
scripts/nicecargo.sh test --offline          # every fixture, the properties, the fuzz bodies
scripts/nicecargo.sh clippy --all-targets -- -D warnings
cd ../go && go test ./... -run Parity         # the fixtures are current with the Go code
SARINA_PARITY_UPDATE=1 go test ./... -run Parity   # after a deliberate change: rewrite them, review the diff
```

## Resume here

Done (2026-11-11 night): phase 0; the workspace and the fixture writer; `shellparse`, the bash guards and `ValidateArgs`
with ParseFloat, `testguard` (and Go's filepath), `pyfmt`, `flagx` (and Go's flag package), `version`, each with its
fixture and a mutation run, properties and fuzz targets for the parsers; the CI job (`.github/workflows/rust.yml`:
the Go fixtures current, fmt, clippy `-D warnings`, the tests). No existing Go file was changed.

Next, in this order:

1. (Done: `pyfmt`, `testguard` with Go's filepath on a made-up tree, `version`, `flagx` with Go's flag package.)
2. The other pure checks in `tools`: `IsSecretPath`/`secretName` (`secret.go`), `ExfilRisk`/`PrivateReference`
   (`exfil.go`), the private-folder list (`private.go`, on a made-up home).
3. Step 2's formats, starting with what Heimdall reads: llm's message types, `config`'s data structures and tolerant
   loaders, `session` (writer byte for byte, reader of every file Go reads), `session/redact.go`. This needs a `gojson`
   module (Go's decoder and encoder: take Limbo's `gojson.rs`, which already has Go's error texts and escaping).
