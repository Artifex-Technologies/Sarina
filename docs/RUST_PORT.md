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
| 1 | the workspace (`rust/`), the fixture writer (`go/internal/parity`), the first leaves with fixtures, properties and fuzz targets | **in progress**: shellparse, the bash guards, ValidateArgs, the file tools' guards, testguard, pyfmt, flagx, version (see "Ported"); CI since 2026-11-11 |
| 2 | the file formats: session files, config loaders, llm message types, vecindex, trust, policy, analysiscache | **done** (2026-11-12) but skills' discovery: gojson, the session file (and Save, MigrateDir), config's loaders and AccessTierFromEnv with Heimdall's grants cap, llm's types, repair and normaliser, the permission gate and registry, the tool definitions, vecindex, trust, policy, analysiscache, guard's detectors, agents, the skill file format, sessionsearch |
| 3 | the clients: Heimdall review, embed, llm (OpenAI-compatible and Anthropic), Limbo, MCP, helpers | **done** (2026-11-12) but MCP's definitions loader and manager (with step 4's tools): `gohttp` (a small Go-like HTTP/1.1 client, TLS through rustls), Heimdall's review (`Consult`), `embed`, `helpers`, `limbo`'s client, llm's wire format, transport (`Complete`), Anthropic client and context-window discovery, the MCP stdio client |
| 4 | tools, memory, tiers, the agent loop, runkit, service, cli chat/resume, gui | **in progress**: the file tools (read, list, write, edit, glob, grep), bash with background processes, git_read, the plan-mode tools, the to-do and plan tools, the checks after a code change and check_code; skills' discovery, the skill tool and the long-skill library, MCP's loader and manager, the limbo tools, memory with its review queue and the memory commands, compute tiers (the router, the claude CLI hand-off, wake, Heimdall's grant gate), StateDir; the agent loop (its answer checks, secret redactor and pure helpers, and the loop itself with notes, summaries, plan decisions, resume and sub-agents, against a scripted model); runkit, cli chat/resume, the service with Go's HTTP server (`gohttpd`); next gui |
| 5 | the optional packages (crypto, stego, quantum, vault, daily, insight, sysinfo, doctor, forge, ...) | open |
| switch | `sarina-rs` as `/usr/bin/sarina`, the Go build at `/usr/lib/sarina/sarina` for what is not ported | **begun** (2026-11-12): the `sarina-rs` binary (names, Heimdall's lock, unported commands handed to the Go build by execve); the commands without a model run in Rust (memory-*, session-search, session-migrate, limbo-profiles, limbo-validate, tiers status; proved by `rust/parity/cli-differential.py`); chat, resume, skill, memory-review and service ported and proving (run in Rust only under `SARINA_RS_TRY`; the service proved by `rust/parity/service-differential.py`); packaged on Labyrinth's branch `sarina-rs-switch` (not on main, not installed) |

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
`/usr/lib/sarina/sarina` (or `$SARINA_GO_BINARY`), with the containment check first in both. A command line that is
not UTF-8 goes to Go. Session files, the vault and every state file are shared, in the same formats, so going back is
reinstalling the previous package. `sarina-service.service` is switched by the package (as Limbo's), once its
differential run shows no difference. Commands move from Go to Rust one at a time; the Go build is dropped when nothing
reaches it any more (dead code is a risk).

### The plan for `sarina-rs` (written 2026-11-12; the binary is built once the agent loop is equal)

1. **Start, in Go's order** (`cmd/sarina/main.go`): the name it was started as picks the command (`sarina-service` is
   `service`, `sarina-gui` is `gui`: the lower-cased base name, `.exe` cut); then **Heimdall's containment lock**
   (`trust::contained`, already ported): while it is set, everything but `version`, `help` and `--help` prints Go's
   message and exits 3, before any command runs or is handed over, so a contained Sarina starts neither build.
2. **Dispatch**: an argument that is not UTF-8, or a command not in the Rust table, is handed over; no command, `help`,
   `-h` and `--help` are handed over too until the whole table is Rust (the usage lists every command, and only the Go
   build knows them all). Go's `SARINA_CRYPTO=auto` and math switches are decided from the words given, as in `cli.Main`,
   before a ported command runs.
3. **The hand-over is an `execve`, not a child**: the Go build replaces the process, so its exit status, signals, stdio,
   working folder and environment are exactly the caller's, and nothing of the Rust process stays around. `argv[0]` is
   passed as it came (Go's alias rule then sees `sarina-service` itself) and the arguments unchanged. The program is
   `$SARINA_GO_BINARY` when set to an absolute path, else `/usr/lib/sarina/sarina`; it is never searched for on PATH.
   The file is called `sarina`, not `sarina-go` (changed 2026-11-12 before anything was installed): after the `execve`
   the process's name (`/proc/PID/comm`) and executable are the Go build's, and Heimdall recognises Sarina's processes by
   the name `sarina`: the grant gate refuses a decision with a `sarina`/`sarina-service` ancestor (exe base name, else
   argv[0]), the network source treats commands below a `sarina` process (comm) as the agent's, containment finds
   `sarina`/`sarina-*` processes. With `sarina-go` the first two would no longer see a Go-run Sarina (the GUI started as
   `sarina-gui`, for one, would pass the grant gate's check).
   A missing Go build is exit 127 with "sarina: <command> is not in the Rust build yet, and the Go build (<path>) is
   missing".
4. **The order commands move in**: first those without a model, each already proved (memory-status,
   memory-show, session-search, memory-review-list/-approve/-reject; limbo-profiles and limbo-validate; session-migrate;
   tiers status: **moved 2026-11-12**, `cli::ported` is the table; `version` stays in Go, whose answer is its own build's
   VCS stamp), then `chat` and `resume` once the agent loop is equal against the scripted model, then `service` (the
   always-on unit) after a differential run of both binaries' services on the same requests, then `gui`, then the
   optional packages (step 5). `doctor` moves last of the core: it checks everything.
5. **Proof before a command moves**: its fixture, then a differential run of both binaries on the same arguments,
   environment and files (`rust/parity/cli-differential.py`, as Limbo's: stdout, stderr, exit status and every file
   written compared byte for byte; time and random ids normalised; never the real model, the real vault or real
   sessions). sarina-rs runs there with `SARINA_RS_NO_GO=1`: a command it has not ported fails ("sarina-rs: X is not
   ported", exit 2) instead of being handed to Go, so a case can never pass by running Go twice.
6. **Packaging** (`packaging/arch/PKGBUILD`, and Labyrinth's `sarina-git`; CONTRACT: cargo is already a makedepend for
   the voice programs; done 2026-11-12: Sarina's PKGBUILD on main, Labyrinth's recipes on the branch `sarina-rs-switch`,
   because Labyrinth's publish builds main and merging is the switch): both builds; `/usr/bin/sarina` is sarina-rs, `sarina-gui` and `sarina-service` are links to it,
   the Go build is `/usr/lib/sarina/sarina`. `sarina-service.service` stays `ExecStart=/usr/bin/sarina-service`: until
   `service` is ported that is one extra exec at start (about a millisecond). Going back is the previous package.

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
| `tools/secret.rs`, `tools/exfil.rs`, `tools/private.rs`, `goregexp.rs` | `secret.go` (`secretName`, `IsSecretPath`, `secretGuard` with hard links), `exfil.go` (`ExfilRisk`, `PrivateReference`), `private.go` (`IsPrivate`, `privateGuard`, the credential stores with their globs), on a made-up home folder | `secrets.json` | 1,621 |
| `tools/fsutil.rs`, `goerr.rs` | `fsutil.go` (`ResolveInRoot`, `ResolveForWrite`, `relPath`, `expandHome`, `TruncateOutput`, `preview`), with Go's error texts (`lstat ...: not a directory`; errno table from Heimdall's generator), on a made-up tree for six workspace roots | `fsutil.json` | 139 (1,404 answers) |
| `testguard.rs`, `gopath.rs` | `testguard`, `filepath.EvalSymlinks`, `Abs`, `Clean`, `Join`, `Dir`, `Base` (on a made-up folder tree) | `testguard.json` | 405 |
| `pyfmt.rs` | `chymaera/internal/pyfmt` | `pyfmt.json` | 681 |
| `flagx.rs`, `goflag.rs` | `flagx` and Go's `flag` package (syntax, values, error texts, the usage on an error) | `flagx.json` | 1,513 |
| `version.rs` | `internal/version` (VERSION read at build time; a test checks the Go constant) | | |
| `gojson.rs` | Go 1.27's `encoding/json` (the v1 API on the v2 engine): Unmarshal and Decoder into `any` with Go's error texts, Marshal, an Encoder without HTML escaping, MarshalIndent's layout, float64 and string writing; iterative (10,000 levels on a 256 KiB stack) | `gojson.json` | 1,689 |
| `session/`, `config.rs`, `llm/types.rs` | `session` (Parse, Marshal byte for byte, MessageFromMap, the retired-tool migration, plan state, RepairPairing, NotRunMessage, ToolResultBody, Mutate, Redact, FileMode), `config` (ModelFromMap, RuntimeFromMap, ModelFromEnv, ContextConfig.Settings, AsInt/AsFloat with amd64's float to int), `llm/types.go` (Usage both ways, UsageFromProvider, wire messages, replayed arguments, ToolSpec) | `session.json`, `config.json`, `llmtypes.json` | 3,505 + 135 + 338 |
| `tools/registry.rs` | `permission.go`, `registry.go`, `ForMode` (the gate, needsApproval, deny, hidden tools, Filter, the Builder's order, Execute's order) | `registry.json` | 3,716 |
| `tools/defs.rs`, `tools/defs.json` | the 35 tool definitions the model sees (written from the Go tools by go/cmd/sarina's parity test) | `tooldefs.json` | 61 definitions |
| `vecindex.rs`, `gorand.rs`, `gomath.rs` | `vecindex` (rotation, levels, Add, Remove, Search with container/heap's tie order, the SVEC1 file), `math/rand` with a seeded source (table generated from the Go source), amd64 `math.Exp` on both its paths | `vecindex.json` | 95 (1,031 comparisons) |
| `trust.rs`, `gostruct.rs` | `trust` (Go's base64, `ed25519.Verify` through `ring` with small-order and non-canonical keys and S+L, the signed list read once, `Refuse` on a made-up tree, `Contained`), Go 1.27's `json.Unmarshal` into a struct (case-insensitive keys with Go's two special folds, merging duplicates, null, the first type error) | `trust.json` | 134 (47 trust folders x 21 paths) |
| `heimdall.rs`, `gotime.rs`, `config.rs` | `heimdall/grants.go` (`LoadGrants`: link, owner, others' write, the folder, unreadable, a folder, a path under a file; `TierCap`, `EscalationAllowed`), `config.AccessTierFromEnv` (19 values of SARINA_ACCESS_TIER against 66 grants states), Go's `time.Parse(time.RFC3339)` as JSON reads a time, with its error texts | `accesstier.json` | 131 (1,584 answers) |
| `policy.rs` | `policy` (`Load`: operator files, SARINA_POLICY_FILE's list with duplicates, missing files, folders, unreadable and oversized files, the tighten-only workspace file, the trust check; `Denied`, `DenyRules`, `ApplyBudget` on three starting budgets); each case run 60 times in Go, every distinct answer kept (map order) | `policy.json` | 93 (25 tools, 3 budgets each) |
| `guard.rs`, `gotext/cf.rs` | `guard`'s local checks (`Gate.Scan` before the Heimdall call): invisible and format characters (Go's `unicode.Cf` table, written by the Go test), the leading BOM, the 15 instruction patterns with the match quoted, the 12 credential shapes; on the Go tests' inputs of guard, memory, skills and the vault, seeds changed in case, spacing and length, every gap at its limit, bytes that are not UTF-8 | `guard.json` | 3,347 (two gates each) |
| `analysiscache.rs`, `goos.rs` | `analysiscache` (`New` over files of every shape, `Get`, `Set`'s file byte for byte, its mode, its errors: `MkdirAll`, `CreateTemp`) | `analysiscache.json` | 39 |
| `llm/repair.rs`, `llm/normalize.rs` | `llm/repair.go` (`RepairJSON`: fences, trailing commas, single quotes, raw control characters, closing what was cut off; `ParseArguments`) and `normalize.go` (`<think>` split, `<tool_call>` blocks, the python tag, bare JSON calls, cut-off arguments on a length stop, unique ids), on the Go tests' inputs, tool calls in every shape, every cut of each, generated damage | `repair.json` | 4,021 |
| `session/save.rs`, `goos.rs` | `session.Save` and `writeAtomic` (the id check, the folder made, the temporary file, the mode from SARINA_SESSION_FILE_MODE, the rename; 14 sessions with odd ids x 6 modes x 4 folders) and `MigrateDir` (dry and real runs twice, backups, skipped and failed files, a link, a `.bak` folder, an id with a slash, a folder it cannot write), compared as whole folder listings | `save.json` | 351 |
| `sessionsearch.rs`, `gomath.rs` (`log`), `gotext/letters.rs` | `sessionsearch` (`Search`: files newest first, the lenient reading of messages, the phrase BM25 and the word count, snippets), Go's amd64 `math.Log` (math/log_amd64.s, 1,700 values), Go's letter and digit tables (written by the Go test) | `sessionsearch.json` | 174 |
| `agents.rs` | `agents` (`Parse`: the front matter, its keys, quotes, lists, switches and limits, each document 30 times for Go's map order; `Load`: operator folders, SARINA_AGENTS_DIR, overrides of the built-ins, duplicates, links, folders, oversized and unreadable files, the trust check, the text gate; `Get` with Go's case folding) | `agents.json` | 162 |
| `skills/parse.rs` | `skills/parse.go` (`Parse`: the front matter with scalars, quotes as `strconv.Unquote` reads them, inline, block and comma lists, the switches, the name rule; `ValidName`, `Expand`) | `skillparse.json` | 215 |
| `gohttp.rs`, `heimdall.rs` (`consult`, `url`) | `heimdall/heimdall.go` (`Consult`, `URL`): the request byte for byte (method, URI with query, Content-Type, body) and the verdict made of 43 scripted raw answers from a loopback server (deny, allow, odd reasons, statuses, redirects, chunked with extensions and trailers, cut short, too big, not HTTP, odd status lines and versions, bare `\n` lines, duplicate and bad lengths, unsupported encodings, silent past the 2 s deadline), and the settings | `consult.json` | 52 |
| `embed.rs`, `gostruct.rs` (float32) | `embed` (`Default`'s settings; `Embed`: the request, vectors as float32 bits, Ollama's errors, counts that differ, type errors, the first value of a longer body, a body cut short after a whole value, chunk errors, a refused connection with Go's text; the answer limit just under, at and over it) | `embed.json` | 59 |
| `guard.rs` (`scan`) | `guard`'s full `Gate.Scan`: the local checks first (no request then), Heimdall's verdict for a gate with a kind (memory, a session id), "Heimdall flagged this independently: ..." with the reasons, failing open on an absent or broken Heimdall; the default gate never asks | `guardconsult.json` | 145 |
| `helpers.rs` | `helpers` (`Python`, `Command`, `Available`, `Passthrough`, the `serve` client: ping and protocol, classify, group, the helper's errors, damaged, exact and over-long answer lines, a helper that dies or speaks last after closing its output, Close's exit status, the requests byte for byte), against one shell stand-in for the interpreter that both tests run | `helpers.json` | 18 |
| `limbo.rs`, `gohttp.rs` (unix sockets, any method) | `limbo/limbo.go` (the agent-socket client: `ListProfiles`, `DescribeProfilesAndSkipped` with the capped skipped list, `Validate` and `ValidateBundle` with polling, `RunResult` and its text for the model, the cleaning of guest output, the socket-name rule, Limbo's errors, damaged and over-long answers), against a scripted unix socket (`RawUnixServer`) | `limbo.json` | 32 |
| `llm/openai.rs` | `llm/client.go`'s wire format: `BuildBody` (every request shape, the encoder keeping a tool's own HTML-escaped JSON, `/no_think`), `parseResponse` (content parts, reasoning, finish reasons of any kind, usage, tool calls and the old function_call, the server's errors), `readStream` (server-sent events: data lines, comments, `[DONE]`, tool-call fragments merged by index and id, cumulative usage, every event handed on) and its bounds at, under and over | `openai.json` | 74 |
| `llm/client.rs`, `gohttp.rs` (TLS, bodies as they come), `goctx.rs` | `llm/client.go`'s `Complete`: the one retry (connections, 429, 5xx), the fallback models (after those, a 404 or a timeout; the turn names the model that answered), the timer (the whole call, a second more per 40 tokens allowed when not streamed; reset by every streamed line), cancellation (also during the retry's wait) and a context deadline, the error texts (`cannot reach backend at`, `timed out after`), the answer's bounds, a header value with control characters refused before sending; 45 scripted servers (the same answers and pauses in both tests), TLS against an authority each test makes (trusted, another authority, a wrong address, names only) | `transport.json` | 45 |
| `llm/anthropic.rs` | `llm/anthropic.go`: `BuildBody` (system texts joined, roles merged, a user turn first, tool calls with their arguments read as Go reads them into a map, tool ids in the API's characters, the schema in the system text, tools and tool_choice, the default max_tokens, temperature only above 0), `parseAnthropic` (blocks, thinking, tool use, stop reasons, usage, errors), the endpoints, and `Complete` on 22 scripted servers (the retry, the key scrubbed from every error, the timer not extended by max_tokens, cancellation, the replay as events, TLS) | `anthropic.json` | 80 |
| `llm/discover.rs`, `gourl.rs` | `llm/discover.go`: `serverRoot` (Go 1.27's `url.Parse` and `String` as far as it uses them: schemes, user information, IPv6 hosts, ports and the strict colons, escapes kept or rewritten), `sameModel`, `ContextWindow` on 35 scripted servers (the four probes in order, Go's struct reading of each answer, the num_ctx line, the 4 MiB bound just past, at and with junk, a transport failure ending the round, the cache and its TTL, a cancelled caller not cached) | `discover.json` | 76 |
| `mcp/client.rs` | `mcp/client.go`: `Spawn` (Go's LookPath on the parent's PATH, the overlay environment, the folder, its own process group, Go's start errors), the handshake, `ListTools` and `ListResources` with pages, `CallTool` and `ReadResource` rendered as text (binary named, not inlined; structured content indented), raw calls (the result as the server wrote it), JSON-RPC errors, the server's own requests answered (ping) or refused, noise and odd ids, timeouts and cancels with notifications/cancelled, a dying server's last stderr lines, the 8 MiB line bound at, under and over, a flood of requests, `Close` (a stubborn server's group killed after a second); against one shell stand-in server both tests run (10 scenarios, 44 calls) | `mcp.json` | 19 |
| `tools/files.rs`, `tools/search.rs`, `gopath.rs` (`read_dir`, `walk_dir`, `path_match`) | `readfile.go`, `writefile.go`, `search.go`: `read_file` (whole, line ranges, the same-name hint, the cut with the outline of what was left out), `list_dir`, `write_file` and `edit_file` with every refusal (secret and private paths, the edit loop, summary churn, unread content, the same step, tool-output markers, exfiltration, binary files, erasing, repeated changes, the nearest lines), `glob_search` (Go's `path.Match` per segment) and `grep_search` (bufio.Scanner's 1 MiB lines), the file state; Go's `filepath.WalkDir` and `os.ReadDir`; through the registry on a made-up tree with links, a hard link to `.env`, a private folder and a named pipe, the workspace compared after each scenario | `files.json` | 178 calls in 6 scenarios, 35 patterns x 22 names |
| `tools/exec.rs` | `bash.go`, `exec.go`, `procs.go`, `proc_unix.go`: the bash tool (the exfiltration, privileged and destructive guards before anything runs, the child's environment without secret-looking names in its three layers, its own process group killed on the timeout or a cancel, the output kept to its last 2 MiB, streaming line by line, the files she wrote named), background processes (one pipe for both streams, as Go does when both writers are one) and TaskOutput and TaskStop; Go's `Duration.String`. The fixture runs bash's builtins only, with nothing but bash on PATH and a temporary workspace, so a refused command (always an external program) could find nothing to run | `bash.json` | 46 calls in 2 scenarios, 3 environments |
| `tools/git.rs`, `tools/exec.rs` (`run_program`) | `git.go`: `git_read` (the ref and path checks, the settings that keep a repository's config from running programs, the arguments for each command, the variables, git's failures and empty answers, the bounds) and `GitConfigRisk`; against a stand-in git (bash's builtins only, written from the fixture) that prints what it was given and leaves a mark when rev-parse runs | `git.json` | 40 calls in 11 workspaces |
| `tools/plan.rs` | `plan.go`'s tools: `enter_plan_mode` and `exit_plan_mode` (plan mode unknown, on and off; plans at, under and over 20,000 characters) | `plantools.json` | 3 modes x 7 calls |
| `tools/todo.rs` | `todo.go`: `task_list`, `task_create`, `task_update`, `todo_write`, `plan_get`, `update_plan`; the store under `.port_sessions/` (read as Go 1.27 reads it into the struct: keys in any case, a damaged file as empty; written as MarshalIndent writes it, through a temporary file), the dependency rules, cycles (walked with a loop, so a planted chain cannot exhaust the stack), the plan merged by id, pruning, Go's `%v` of metadata; time, content hashes and random ids written as placeholders | `todo.json` | 77 calls in 16 scenarios |
| `tools/verify.rs`, `tools/exec.rs` (`run_program`, `HeadTail`) | `verify.go`, `localise.go`, `checkcode.go`: the check after a code change (go vet / go test with the failed test first, gofmt without a module, py_compile and unittest, bash -n and shellcheck, node --check, cargo check and test; the shell lint; "not installed"; cargo refused without trust or sandbox; the sandbox copy and its failure, vetOnly), `verifyMode`, the bubblewrap command line (`sandboxArgs`, recorded as data), the localiser (Go failures, panics, Python tracebacks, the function under test), `around`, `check_code`; against one stand-in (bash builtins) for go, gofmt, python3, bash, cargo, shellcheck, cp and bwrap that prints its arguments, folder and variables | `checks.json` | 64 verifyMode, 3 sandboxArgs, 57 runs and calls |
| `skills/discover.rs`, `library.rs`, `recall.rs`, `tool.rs` | `skills`: `Roots` (operator folders, SARINA_SKILLS_DIR, skills.json read as Go 1.27 reads it, the system folder, the workspace's only with the opt-in), `Discover` (name.md and name/SKILL.md, links refused, the scan gate and the owner's signed list first, files over 4 MiB, a SKILL.md without front matter, collisions, notices), long skills (sections from headings or `sections/*.md`, parts, the table of contents and its cut), the `skill` tool, `skill_section`, `skill_search` by words (Go's amd64 `math.Log`) and by meaning (an index per skill, saved and loaded: the folder name, `meta.json` and `passages.vec` byte for byte the same as Go's), `ExpandWith` with prefetched context; with a made-up embedder and trust list in both tests | `skills.json` | 122 comparisons |
| `mcp/config.rs`, `mcp/manager.rs` | `mcp/config.go` (`Load`: ~/.sarina/mcp.json and SARINA_MCP_FILE's files, the workspace's only with the opt-in, the owner's signed list, the 256 KiB bound, `mcpServers` and `servers`, every problem) and `manager.go` (`Connect`, the tools offered as `mcp__<server>__<tool>` with their cleaned and scanned descriptions and classes, `Status`, calls, the result bound, a server restarted after it exits, a listing that fails on a later page, the resource tools), against the client fixture's shell stand-in server | `mcpmanager.json` | 53 comparisons |
| `limbo.rs` (`tools`) | `limbo/integration.go`'s tools: `limbo_profiles` (the listing, its empty forms with and without skipped files) and `limbo_validate` (blank arguments, the timeout read leniently and clamped to 10..900, an infrastructure failure as an error with its metadata, Limbo's errors, the shell gate, a missing socket), against the scripted agent socket; the `limbo-*` commands come with the CLI | `limbotools.json` | 20 calls and the definitions |
| `memory.rs` | `memory/runtime.go` and `tools.go`: the two scopes, `Add`/`Replace`/`Remove` with the gate, the limits, duplicates, the files written, the prompt blocks (whole and the most relevant entries for a task), the summary, `memory_add`/`memory_replace`/`memory_remove`/`session_search`, and the files that may not be used (a link out of the workspace, a linked folder, a link to a secret file, a named pipe, one too large, a folder); the review queue (`review.go`) comes with the CLI | `memory.json` | 24 calls, 7 file cases |
| `tiers/` (config, guard, resource, cli, wake, router), `heimdall.rs` (`request_grant`), `gotime.rs` (formatting) | `tiers`: `Parse` (Go 1.27's struct reading: the type errors, keys in any case, duplicates), `Load`, `FromEscalation`, `RedactSecrets`, `LooksLikeCapture`, `Sanitize`, `Pressure`, the meminfo and nvidia-smi parsers, `BuildPrompt`, `Command`, the claude CLI's and the wake command's environments, `offMachine`, `estimateTokens`; the claude CLI client (its own process group, the timeout and a cancel, every kind of answer, the output bounds) against a stand-in in bash's builtins; the wake runner (URLs, the shell command under the permission model); the `Router` in 37 scenarios against scripted model servers and a scripted Heimdall (health probes and their cache, escalations, refusals, redactions, confirmation, /tier up and down, wake, the grant gate with requests filed, the CLI tier and its cap, status, the session record merged across turns); `heimdall.RequestGrant` | `tiers.json`, `tiersrun.json` | 548 + 328 |
| `tools/fsutil.rs` (`state_dir`), `memory/mod.rs` (`session_dir`) | `tools.StateDir` on a made-up tree (links out, in, dangling, at each level) | `statedir.json` | 216 |
| `memory/review.rs`, `cli/memorycmd.rs` | `memory/review.go` (the transcript, the array in a model's answer, proposals with the gate, `Propose` against a scripted model, the queue file through Load, Add, Pending, Get, Approve, Reject and the summary) and `cli/memorycmd.go`'s commands that need no model (memory-status, memory-show, session-search, memory-review-list, -approve, -reject, with their flags, usage errors and help) | `review.json`, `memorycmd.json` | 236 + 31 |
| `agent/claims.rs`, `agent/secrets.rs`, `goregexp.rs` (`\p{L}`, `\p{N}`) | `agent/claims.go` (every answer check and carry-on nudge, the texts sent back word for word; `isCommand` on a made-up PATH, `deniedReadable` and `taskInsteadOfFile` on a made-up workspace) and `secrets.go` (`toolTexts`, `secretValues`, `hideSecretsRead`, the stream redactor): on every string literal of `claims_test.go`, `secrets_test.go` and `acceptance_test.go` (and the same with Unicode spacing, in capitals, broken at full stops, cut in half, and answers at each check's limits) in 52 tasks from those tests; a task whose tool output is a JSON object run four times in Go (map order), every distinct answer kept | `claims.json` | 1.9 million answers checked |
| `agent/limits.rs`, `agent/prompt.rs`, `agent/compact.rs` | `limits.go` (tokens, the window from a model's name, `TakeSnapshot`, `Cost`, `checkBudget`), `prompt.go` and `lessons.go` (the system prompt's parts, the lessons file's rules, the guidance files on a made-up tree, the git snapshot against a stand-in git in bash, the curated-memory block), `clear.go`, `compact.go` (`reduce`: snip, fold, reactive), the pure parts of `notes.go` and `summarize.go` (coverage, `normaliseNotes`, the transcript, `tailWithin`, `cleanSummary`); session files compared by sha256 | `agentpure.json` | 368 |
| `agent/run.rs` | `agent.go` with the agent's parts of `plan.go`, `notes.go`, `summarize.go`, `window.go`, `tiers.go` and `subagent.go`: the drive loop against a scripted model from a loopback server, the Go agent and the port given the same answers (go/agent/run_parity_test.go): plain answers, tools, writes, the claim check, empty and thinking-only replies, continuations, every nudge kind the corpus reaches, the no-think rule, budgets, the turn limit, duplicate and broken calls, missing usage, streaming with a secret, backend errors, the repeat-limit retry, the reactive retry, plan mode with each decision, resume, notes by cadence and by window, notes the scan blocks, summaries and a failed one, snipping and compaction thresholds, the hard limit, cancellation, schema and reasoning, sub-agents and refused ones | `agentrun.json` | 41 tasks, 794 comparisons (every request body, the events, results, session files, notes, child sessions, the workspace) |
| `bin/sarina-rs.rs`, `cli/mod.rs` (`ported`), `cli/sessionmigrate.rs`, `cli/limbocmd.rs`, `cli/tiercmd.rs`, `goflag.rs` (repeatable flags, a usage line of the command's own, back-quoted names) | the commands sarina-rs runs itself: the memory commands above, `cli/sessionmigrate.go`, `limbo/integration.go`'s limbo-profiles and limbo-validate, `cli/tiercmd.go`'s tiers status with `runkit.EscalationFlags`; whole binaries side by side ("The CLI differential") | `rust/parity/cli-differential.py` | 195 cases, 975 comparisons (of the 289 the differential now has) |
| `cli/chatopts.rs`, `goflag.rs` (float flags with Go's `%v` and range rules, the flags given) | `cli/chat.go`'s `newOpts` (every flag, its order and usage) and `opts.build`: a fresh run, a stored session (given flags only; permissions only widen), another account's session (server, permissions, folders, escalation from this run), the access tier, budgets, the context switches, the schema file, the allowlist | `chatopts.json` | 37 |
| `cli/chat.rs` | `cli/chat.go` (`opts.agent`, `conversation`, `report`, the event printing, `runTurn` with Ctrl-C), `slash.go` (/help), `plancmd.go` (/plan, `askPlanDecision`), `skillcmd.go` (skill list, skill run, /skill), `tiercmd.go` (`tierRouter`, `stdinConfirm`, `linesConfirm`, /tier), `memorycmd.go` (memory-review, the review when a conversation ends), `owner_unix.go`; the default tools in the Go build's order with `sarina_doctor` run in the Go build | the CLI differential's chat cases (scripted model) | 94 cases |
| `session/readfile.rs` | `session/readfile.go` (new 2026-11-12): a session file or a session's notes as a bounded regular file; `Load`, `LoadByID` | Go's tests, `tests/session_reads.rs` | 1 each |
| `runkit.rs` | `runkit`: `Scrubber` (literals, the redactor's shapes, bearer tokens, secret-named JSON keys, key=value), `Entry`, `Transcript`, `Result` as the service and the GUI send them; `Build` from plain settings (allowlists, policy, compute tiers, the claude CLI escalation, sub-agents, prompt options), each built agent run once against the same scripted model as Go's | `runkit.json`, `runkitbuild.json` | 17 + 11 (every request body) |
| `agent/run.rs` (`Options.confirm`, `set_confirm`), `tools/registry.rs` (`Confirm`, `Context.confirm`) | `agent.Options.Confirm`, `SetConfirm`, `tools.RuntimeOperatorConfirm`: the per-call operator question (council_ask's channel), not the Approver; sub-agents get the parent's | Go's `TestSetConfirmAddsOnlyItsOwnRuntimeKey`, as a unit test | 1 |
| `bin/sarina-rs.rs`, `cli/entry.rs` | `cmd/sarina/main.go`: the names it answers to (`aliasCommand`), Heimdall's lock first (the built Go program run with a made-up lock under each name and command line: which are refused, the message, exit 3), and the hand-over to the Go build by execve: the arguments unchanged (bytes that are not UTF-8 too), the environment, the folder, the exit status, the caller's SIGPIPE (ignored or not); a missing Go build (127) | `entry.json` | 41, and 9 hand-over checks |
| `service.rs`, `gohttpd.rs`, `cli/servicecmd.rs`, `gourl.rs` (`parse_request_uri`), `goflag.rs` (`tribool`) | `service/service.go` (the pool with its LRU, create with every check of the ceiling, chat with resume and the local /help, delete, the browser guard, the bearer token, the 422, 403 and 404 bodies; each pooled session's agent on a thread of its own, which serialises its turns; a client that hangs up cancels its turn) and `command.go` (the flags with `triBool`, the banner, the listen errors, SIGINT and SIGTERM with Go's 5 s shutdown); Go 1.27's `net/http` server as the service uses it (request reading and its errors, keep-alive, 100-continue, the answer's header order, chunking past 2 KiB, draining an unread body, the mux's cleaned-path redirects) | the service differential | 33 scenarios, 279 steps |
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
- **Properties and fuzzing**: `rust/fuzz` targets `shellparse`, `bash_guards`, `parse_float`, `secrets` (cargo-fuzz, nightly; not run on
  this PC, which has no nightly toolchain); their bodies run on stable in `tests/properties.rs` over random bytes,
  shell-shaped lines and every fixture line with bytes changed. What must hold: no panic; a line that does not parse is
  refused by both guards; a privileged or destructive program, or one built at run time, is refused wherever it sits;
  `sudo` before anything is refused; ParseFloat agrees with Rust's parser on plain decimals and round-trips every finite
  float; ValidateArgs never adds a key and is idempotent; content ExfilRisk refuses always names a store for
  PrivateReference too; a secret file name stays one in any folder and in any case.

## Differences from Go, on purpose or accepted

- **Go 1.27's JSON is not v1's.** Go 1.27 builds with `GOEXPERIMENT=jsonv2` on, so `encoding/json` runs on the v2
  engine: a bad escape reads "invalid escape sequence `\x` in string" (v1 and Limbo's and Heimdall's ports: "invalid
  character 'x' in string escape code"), a number cut short "in numeric literal" without the place, the depth limit
  "exceeded max depth", the session reader's type error names `map[string]jsontext.Value`, and a number out of float64's
  range reads as 0 (not null) with the error while the rest is still read. `gojson.rs` follows Go 1.27; the siblings'
  ports may differ from their own Go builds in these texts (for their boards).
- **vecindex: two Go faults, fixed in Go too (2026-11-12), so Go and Rust agree.** `New(65536, ...)` passed Go's
  dimension check and never ended (its block loop appended blocks of size 0 until memory ran out; a damaged index file
  with that dimension did the same to `Load`): both refuse 65,536 as an unsupported dimension now. Go's `Load` allocated
  the slices for the file's vector count before reading (2^32 vectors passed its size check: about 52 GB); both check
  the count against the bytes the file still holds first, with the error the read would give (`EOF`, `unexpected EOF`).
  Both hit my PC through the first fixture writer (killed by the kernel's OOM killer, 2026-11-11 23:34); the fixture
  now records both cases, and Go's own tests (`TestDimension65536Refused`, `TestLoadCountBeyondFile`) run them bounded
  in time and allocation.
- **vecindex levels depend on the CPU.** Go's amd64 `math.Exp` has an FMA path and a plain one, which differ in the last
  bit now and then; the levels (not saved in the file) follow, so Go itself can search a moved index very slightly
  differently. The port takes Go's path on the same CPU (AVX and FMA), so the fixture holds on this PC and on CI.
- **Deep JSON.** Go reads 10,000 levels; a recursive Rust reader needed over 4 MB of stack for that (release), so the
  reader, the conversion to `any`, the writer and the drops of `Value` and `Any` are loops. `Any` therefore has its own
  `Drop` (parts are taken out with `into_object`/`into_array`/`into_string`); `Clone` and `==` are still recursive.

- **Go 1.27 reads a time in JSON loosely.** The v1 API on the v2 engine keeps `time.Parse(time.RFC3339, ...)`, not the
  strict RFC 3339 reading `Time.UnmarshalJSON` does since Go 1.20: a one-digit hour (`T1:00:00Z`), an offset up to
  `+24:60`, a comma before the fraction, and fraction digits past nine dropped are all accepted; the errors are
  `time.Parse`'s ("cannot parse \"z\" as \"Z07:00\"", "day out of range", "extra text: ..."). `gotime.rs` follows it.
  Heimdall's `gotime.rs` (where this one comes from) refuses `+24:00` and a one-digit hour and words its errors
  differently (for its board: it writes and reads the grants file Sarina reads).
- **analysiscache kept a value Go cannot write: fixed in Go and Rust (2026-11-12).** `Set` put the value in memory
  before encoding, so after a NaN or an infinity every later `Set` failed. Both now refuse such a value first
  ("analysiscache: value not stored: json: unsupported value: NaN") and the cache stays writable.
- **Normalize renames a server's own id.** Ids are made unique in order, and a generated `call_<i>` (or `call_<i>_<n>`)
  can take an id a later call brought itself, which is then renamed too (`call_1`, ``, `call_1_1` becomes `call_1`,
  `call_1_1`, `call_2`). Harmless (the ids only pair calls with results within the message); kept as Go does it.
- **Session ids that cannot name a file: fixed in Go and Rust (2026-11-12).** `Save` used to fail on an id with a
  slash only because Go's `CreateTemp` refused the pattern, and `MigrateDir` stopped at such a file on every run, after
  writing its backup. Now `CheckID` refuses an id with a slash or a NUL byte (or an empty one) before anything is written,
  and `MigrateDir` reports a file it cannot write in `Failed` ("<name>: why"), leaves it as it was, writes no backup for
  it and goes on with the rest (`sarina session-migrate` prints the failures and exits with an error). Kept as Go: a
  `.bak` that exists as anything (a folder too) means no new backup, and a session file that is a link is replaced by a
  regular file (the safer outcome).
- **Texts that are not UTF-8 in profiles become U+FFFD when read.** Go keeps a profile's name, description, prompt and
  tools as the file's bytes; the port keeps strings, so a byte that is not UTF-8 is U+FFFD at once, which is what Go's
  JSON encoder writes when those texts reach the model. Error texts quote the original bytes as Go does.
- **The HTTP client (`gohttp`) is not all of Go's.** It speaks `http://` and, since the model transport, `https://`
  (rustls with ring, TLS 1.3 and 1.2; the system's authorities read from the files Go's crypto/x509 reads on Linux:
  `SSL_CERT_FILE` or Go's six bundle paths, then `SSL_CERT_DIR` or /etc/ssl/certs and /etc/pki/tls/certs, each file read
  to at most 16 MiB). Not as Go: redirects are not followed (Go follows them; an answer is then taken only from the
  configured address, and an API key never goes to another); no proxy is taken from `HTTPS_PROXY`/`HTTP_PROXY` (Go's
  default transport uses them for addresses other than loopback); no `Accept-Encoding: gzip` is sent, so a body with a
  `Content-Encoding` reads as an error (Go asks for gzip and unpacks it); one request per connection (`Connection:
  close`); a URL Go's `url.Parse` refuses (`http://[::1`) gives another error text. TLS refusals carry Go's words for an
  unknown authority and for a name or address the certificate does not hold (both compared); an expired certificate
  reads without Go's times, and other TLS failures as `tls: ` and rustls's text. Everything else Sarina's code can see was compared with Go's client on raw
  answers: statuses, informational 1xx answers skipped, framings (Go's chunked reader ported whole: CRLF checks, bare LF,
  the 4096-byte size line, the overhead limit, the trailer), odd heads, cut-short bodies, deadlines, cancels. For
  `Consult` these differences all end as "no verdict" (fail open), as an unusable answer does in Go.
- **embed's answer is bounded, in Go and Rust (2026-11-12).** Go read Ollama's answer without a limit, so a program
  answering on Ollama's port could make Sarina read without end. Both now read at most `MaxAnswerBytes` (64 MiB) and
  refuse a larger answer ("embedder answer is larger than 67108864 bytes; not read"). Sized from the largest plausible
  answer: Sarina embeds at most 32 texts at a time, and even 256 texts of a 4096-number model at 24 bytes a number in
  JSON is about 24 MiB, so a real answer never comes near it. The rule for the clients still to come (llm, limbo, mcp,
  helpers): every read from another process gets a bound, in both languages.
- **The Python helper's pipes are bounded and its failures reported reliably, in Go and Rust (2026-11-12).** Go read an
  answer line without a limit and kept all of the helper's error output for its whole life; both now read at most
  `MaxAnswerBytes` (64 MiB) a line (a longer one stops the helper) and keep the last 64 KiB of its error output. Go
  could also name a dying helper's reason before its error output was copied (a race: "helper exited: EOF" instead of
  the reason); both now wait (at most 2 s) for that output, and once the helper is gone every later call answers the
  same error.
- **Limbo's over-long answer has its own error, in Go and Rust (2026-11-12).** Go read 1 MiB and cut the rest, so a
  long answer read as "Limbo returned something that is not JSON"; both now say "Limbo's answer is larger than 1048576
  bytes; not read". The limbo tools and commands (`integration.go`) come with step 4's tools.
- **The model clients' answers are bounded with their own errors, in Go now (2026-11-12); the Rust llm port does the
  same (`llm/openai.rs`).** Go cut a non-streamed answer at 256 MiB silently (it then read as damaged JSON) and read a streamed answer
  without any bound, line or total. Now an answer past `maxResponseBytes` (256 MiB) is refused, streamed or not ("the
  model's answer is larger than ... bytes; not read"), and so is a streamed line past `maxStreamLine` (16 MiB); the
  model-description probe (`discover.go`) treats an answer past 4 MiB as none. The MCP client was already bounded (a
  line of at most `MaxLineBytes`, 2 KiB of stderr kept).
- **The Anthropic client's errors never hold the API key, in Go and Rust (2026-11-12).** Go scrubbed the key from a
  transport error and an error status, but not from an error the server wrote in a 200 answer (`"type":"error"`, its
  message echoing the key) nor from the timeout's text (a base URL holding the key). Both now scrub every error text;
  Go's `TestAnthropicErrorsInAnswersScrubTheKey` failed before the fix, and the fixture checks every case.
- **The file tools: four faults fixed in Go and Rust (2026-11-12).** A refused `write_file` marked the file as hers before
  the "you have not read it" refusal, so the same write sent again replaced content never read (and a failed `edit_file`
  did the same); now a file counts as hers only once it is written (`TestRefusedWriteOrFailedEditDoesNotUnlockAnUnreadFile`
  failed before the fix). A line-range `read_file` held every line asked for (a range over a huge file, or one line of
  gigabytes) before the answer was cut: both gather at most 32 MiB (`maxRangeRead`) and skip earlier lines without keeping
  them, and every whole read (`read_file`, the file `write_file` replaces, `edit_file`, the read-back) stops at 32 MiB
  (`TestReadFileRangeIsBounded`). `write_file` onto a named pipe and `grep_search` given one waited for a writer forever;
  both refuse anything but a regular file now (`TestFifoIsNotOpenedByTheFileTools`).
- **bash's timeout message names the time used, in Go and Rust (2026-11-12).** Without a configured timeout a command was
  killed after the default 30 s and reported as "timed out after 0s"; the default is applied before the call now
  (`bashTimeout`, `TestBashTimeoutIsTheOneNamed`).
- **A child's output pipes are read with poll(2), not by threads.** Go copies each pipe in a goroutine and stops waiting
  2 s after the shell exits (WaitDelay); the port reads both in one loop that also sees the shell's end, the timer and a
  cancel within 50 ms, and stops reading 2 s after the end in the same way (a job left in the background cannot hold
  the call: a unit test). The output buffer keeps the last 1 MiB once it passes 2 MiB, as Go's; where it cuts depends on
  how the pipe delivers the bytes, so output past 2 MiB can start at a slightly different place than Go's.
- **git_read: three faults fixed in Go and Rust (2026-11-12).** git's output was read without a bound (a huge diff was
  held whole) and so was the repository's config listing: both keep 1 MiB (`maxGitOutput`) and read and drop the rest,
  and a config listing past it counts as risky (a filter driver after a megabyte of harmless settings would otherwise go
  unseen). `diff` without a ref asked git for HEAD before the repository's settings were checked, with the whole
  unfiltered environment and no time limit; now the settings come first and rev-parse gets the filtered environment
  and the 20 s limit (`TestGitOutputIsBounded`, `TestGitReadChecksSettingsBeforeRunningGit`).
- **The to-do list and the plan stay in the workspace, in Go and Rust (2026-11-12).** `.port_sessions/` was joined to the
  workspace without being resolved, so a `.port_sessions` link planted in a workspace led `task_runtime.json` and
  `plan_runtime.json` out of it (read and written there). Both resolve the store as a file tool's write is now; a store
  outside reads as empty and is not written (`TestTaskStoreStaysInTheWorkspace` read and wrote the outside list before).
  The same pattern exists for the agent's memory (`memory/runtime.go`, `.port_sessions/memory/AGENT_MEMORY.md`) and the
  default session folder: to be looked at when those are ported.
- **The checks after a code change read their programs' output with a bound, in Go and Rust (2026-11-12).** A build or
  test run's output (and `go vet`'s, and the sandbox copy's) was collected whole; both keep the first and the last MiB
  (`maxCheckOutput`) with how many bytes were left out between them, since the localiser reads the first failure and the
  last assertion (`TestCheckOutputIsBounded`). The source files the localiser searches, the script the shell lint reads
  and `Cargo.toml` are read through the 32 MiB bound (a larger file is skipped). `sandboxStep`'s bubblewrap command line
  is a pure function now (`sandboxArgs`), so the fixture proves it without copying or running anything; behaviour
  unchanged.
- **hasPyTests reads the folder, not a glob.** Go globs `<dir>/test_*.py`, so a folder whose name holds `*`, `?` or `[`
  is read as a pattern and its tests may be missed; the port lists the folder and matches the names. Same answer for
  every ordinary folder.
- **skills.json is read through the skill files' 4 MiB cap, in Go and Rust (2026-11-12)** (it was read whole). Not
  covered by the fixture: the 0.45 similarity below which a passage is not offered (no made-up passage scores between
  0.44 and 0.45).
- **The agent's memory stays in the workspace, in Go and Rust (2026-11-12).** `<workspace>/.port_sessions/memory/AGENT_MEMORY.md`
  was read without resolving links and put in front of the model every session, with no scan on reading: a workspace
  that planted it as a link to a private key (or the folder as a link) put that file's text into Sarina's prompt, and a
  write went through it. A named pipe there hung the session at its start, and a file that could not be read counted as
  empty, so the next `memory_add` replaced it. Now the agent file and the review queue are resolved inside the workspace
  on every use (never a secret or private file), only a regular file of at most 1 MiB is read, and a write over one that
  cannot be read is refused ("cannot use the stored memory, so nothing was changed"). `TestAgentMemoryStaysInTheWorkspace`
  saw the key's text in the prompt before the fix; `TestAgentMemoryFileMustBeRegularAndReadable` hung on the pipe.
- **The claude CLI's and nvidia-smi's output is bounded, in Go and Rust (2026-11-12).** The claude_cli tier kept the
  CLI's whole standard output and error output, and the GPU sampler read nvidia-smi's whole answer. Now at most 16 MiB of
  the CLI's answer is read (a larger one fails the hand-off: "claude CLI's output is larger than 16777216 bytes; not
  read"), only the 2,000-byte preview of its error output is kept, and nvidia-smi's answer is read to 64 KiB (more is no
  reading) (`TestCLIOutputIsBounded`, `TestNvidiaOutputIsBounded`).
- **Session files, notes and scratchpads stay in the workspace, in Go and Rust (2026-11-12).** The default session folder
  (`<workspace>/.port_sessions/agent`) and scratchpad root were joined to the workspace without being resolved: a
  `.port_sessions` link planted in a workspace made Sarina save the whole conversation, its notes and her scratchpads
  wherever it pointed, and made session_search read the .json files there. `tools.StateDir` (Rust `state_dir`) now
  resolves a state folder under the workspace's own .port_sessions as a file tool's write is resolved and refuses one that
  leads out; folders the operator chose elsewhere are used as given. The operator's own commands (`resume`,
  `session-search`, `memory-review`, `session-migrate`, `doctor`, `gui`) still take the default folder as it is: the
  operator runs them on a folder of their choosing (`TestSessionFilesStayInTheWorkspace`, `TestSessionSearchStaysInTheWorkspace`).
- **The tier router's HTTP probes are gohttp's.** A health probe or wake URL that redirects is not followed (Go follows
  it); a probe for a context already cancelled is not dialled (as Go). A wake URL Go's `url.Parse` refuses reads "bad
  wake URL: parse "...": invalid URL" (Go names the parse error). nvidia-smi is waited for at most 2 s after it ends
  (Go waits for its output without a limit).
- **The claude CLI hand-off's answer is not redacted.** The previews in the session record are, as in Go; the answer text
  is the turn's content as the CLI wrote it (kept as Go; the session redactor applies to tool output, not to answers).
- **The review queue keeps two Go behaviours.** A queue file that does not read as JSON counts as empty, so the next
  proposal overwrites it (only proposals are lost; memory itself is never written over an unreadable file); and of two
  entries with the same id only the first can be approved or rejected (ids are 12 random hex digits).
- **The git snapshot and the guidance files are read with a bound, in Go and Rust (2026-11-12).** Each git command of the
  snapshot a session starts with was read whole (a status of millions of changed files, of which 2,000 characters are
  kept), and a workspace's CLAUDE.md (and the other guidance files) was read whole into memory: a sparse file of
  gigabytes costs nothing on disk. Both read the first MiB now (`maxSnapshotOutput`, `maxGuidanceBytes`;
  `TestGitSnapshotOutputIsBounded`, `TestGuidanceFileIsReadWithABound`, both failing on the old code). A guidance file
  whose first MiB is all spaces now reads as empty (before: its text after them).
- **The answer checks' `\p{L}` and `\p{N}` are the regex crate's tables** (Unicode 16) where Go has its own; a letter
  added in a later Unicode version could count as a word in one and not the other (`inventedHistory`'s words of four
  letters or more). A lessons rule cut at 300 bytes inside a character keeps the bad bytes in Go and has U+FFFD here: the
  same once written to JSON (the model's request, the session file). `secretValues` sorts values of the same length
  stably (Go's sort is stable only up to twelve values), and `toolTexts` walks an object in key order (Go: random); the
  fixture accepts any answer Go gave.
- **The agent loop: what differs from Go's.** A streamed `usage` event holds the usage as Go's JSON writes the struct (its
  fields in the struct's order: the text is kept as written, `Any::Num`). The tier tracker's events reach the run's list
  after the model call they happened in, and before the next streamed piece (Go appends them as they happen: the same
  order unless an escalation and a streamed reply interleave). The `agent` tool's handler hands the checked call to the
  loop, which runs the child (a handler cannot reach the agent that runs it); the registry's checks come first, as in
  Go. Not in the Rust options yet: Go's `Confirm` (`tools.RuntimeOperatorConfirm`, for council_ask: no tool that asks
  the operator per call is ported). A run returns its result and its error separately (Go: both from one call). The
  reactive retry's halved tail (1 or 2 messages kept) is not told apart by the fixture (the one mutation it missed).
- **grep_search's patterns are read by `goregexp`.** Go's regexp syntax is translated to the regex crate's with Go's
  meaning, and constructs it does not translate (named groups, POSIX classes, `\pL`, `\Q...\E`) are refused; Go
  accepts them. A pattern Go refuses is refused here too, but the error after "invalid pattern: " is the regex crate's,
  not Go's ("error parsing regexp: missing closing ): `(`"); the fixture checks only that it is refused. A repeat count
  over 1,000 is accepted here (Go refuses it). Only what grep finds could differ, never what it may read.
- **The checks of code files after a change (`verify.go`) are not ported yet**: a change to a `.go`, `.py`, `.sh`, `.js`
  or `.rs` file says "the Rust port does not check code files yet, so the change was not verified" instead of a result.

- **Map order.** Go walks the pipelines of a command line (`CheckDestructive`) and an object's properties
  (`ValidateArgs`) in map order, which is random: when two pipelines are refused for different reasons, or two properties
  are wrong, Go names either one. The port names the first (source order; sorted property names). The fixture runs such
  lines 400 times and accepts any reason Go gave. Whether something is refused never depends on the order.
- **An MCP server's own requests are answered at most eight at a time, in Go and Rust (2026-11-12).** Go wrote each
  answer from a goroutine of its own, so a server that sent requests without reading its input made the client start
  goroutines without bound (5,000 requests: 5,001 goroutines, measured); both now keep at most `maxReplies` (8) answers in
  flight and drop the rest (`TestFloodOfServerRequestsIsBounded`; the fixture's flood scenario). Both also wait (at most
  2 s) for a dying server's stderr before naming it, as `go/helpers` does (a race there was seen; here it could not be
  made to fail on my PC, `TestDyingServerReasonIsNotLost` guards it).
- **The MCP client passes the child an environment as names and values.** Go hands `execve` the list as it is; the port
  sets each `NAME=value` entry, so an entry without `=` or with an empty name is left out, and of two entries with the
  same name the last wins (C's getenv finds the first). The environment Sarina builds has neither.
- **Discovery's model_info in map order.** Go walks `/api/show`'s `model_info` in random order and takes the first key
  ending in `.context_length` with a plausible number; the port takes the first in sorted order. Only a server naming two
  different plausible windows there could tell (none does); the fixture avoids that case.
- **`sarina-rs` writes a ported command's output as Go does, nearly.** Each line goes out as it is printed; a usage
  error's text is written when the command returns (Go writes it during the parse): the same bytes on each stream, only
  the interleaving of stdout and stderr on one terminal can differ. A write to a closed stdout or stderr ends sarina-rs
  by SIGPIPE, as Go's runtime ends the Go build, even when the caller started it with SIGPIPE ignored (Go does that
  too, for fds 1 and 2; the differential checks both). An environment value that is not UTF-8 reads as U+FFFD
  replacements in a ported command (Go keeps the bytes); none of the variables these commands read holds such bytes.
- **`sarina_doctor` in a chat run by sarina-rs** runs the check-up in the Go build (`doctor --json --cwd ROOT
  --session-dir DIR`, the process's environment, its report bounded at 4 MiB), where Go runs it in its own process; the
  report and `doctor_failed` are the same (the differential's chat doctor case). A Go build that cannot be started makes
  it a tool error, where Go's never fails. It goes when doctor is ported.
- **Ctrl-C.** Go's `cli.Main` takes SIGINT for every command, so a Ctrl-C does nothing to a command that does not watch for
  it (limbo-validate waiting on a guest for up to 20 minutes, a session-search over many files); the moved commands in
  sarina-rs keep the default action and end. chat, resume and skill run take it as Go's do (a thread waits for the signal:
  it cancels a running turn, else it reaches whoever waits for input; the differential's Ctrl-C cases).
- **limbo-validate with a huge `--timeout`.** Go's deadline is `time.Duration(timeout+300) * time.Second`, which wraps
  for a timeout past about 9.2e9 s (a deadline already past, so a run still running is "did not finish in time"); the
  port wraps the same way. Before 2026-11-12 the port added in plain i64 (a panic in a debug build).
- **Found on the way** (not Sarina's): Limbo's port of the flag package (`rust/limbo/src/cli/flags.rs`, where `goflag.rs`
  comes from) kept a bool flag's old value after an invalid one (`-w=x`); Go stores false before it reports the error.
  Harmless there (the command stops on the error); fixed here.
- **The service's HTTP server** (`gohttpd.rs`, Go's `net/http` server). The head's 1 MiB + 4 KiB limit counts the head's
  own bytes where Go counts what its 4 KiB reads took from the socket, so the two may differ within 4 KiB of the limit
  (both answer 431 well past it); a trailer after a chunked body must already be buffered with the last chunk (Go waits
  for it within its buffer, the same in practice); at most 256 connections are served at once (a thread each), a
  further one is closed at once (Go has no limit); `--host` with a name other than localhost (only allowed with a token)
  is resolved by the system's resolver and a failure reads "lookup NAME: no such host". Each pooled session's agent runs
  on a thread of its own (8 MiB of stack reserved, used as needed); nothing of it shows on the wire.
- **Paths**: `realRoot` follows links with a port of Go's `EvalSymlinks` (not `std::fs::canonicalize`, which differs for
  relative paths and missing parts).

## The CLI differential (`rust/parity/cli-differential.py`)

Both builds on the same command lines: each case is run by the Go build and then by sarina-rs, each time in a throwaway
folder made afresh with the same made-up home (`~/.sarina`: user memory, tiers.json) and workspace (`.port_sessions`:
agent memory, the review queue, session files), the same environment (only what the case sets, `PATH` a folder of its
own with stand-ins where a case needs them, Heimdall's lock path a file of the case's), stdin and working folder. Two
scripted servers record every request and answer from the case's list: Limbo's agent socket (a unix socket named
`agent.sock`) and one loopback HTTP server that stands in for model endpoints (the tiers' `/models` probes; every
`base_url` and `OPENAI_BASE_URL` points at it) and Heimdall's review API. Compared: stdout, stderr, exit status, the
requests (method, target, Host, Content-Type, the key headers, the body; not User-Agent, Accept-Encoding and
Connection, see "Differences" for the HTTP client) and every file left (type, mode, contents). Normalised: only the
machine's memory percentage in `tiers status` (it changes between the two runs).

On 2026-11-12: 195 cases, 975 comparisons, no difference; with the chat cases (below) 289 cases, 1,445 comparisons, no
difference. The cases: the memory commands on usual, missing, damaged,
over-full, linked, non-UTF-8 and invisible-character memory, every review entry approved and rejected (also with Heimdall
denying, allowing and failing, and through the old `SENTRY_REVIEW_URL`); session-search with flags among the words,
bad and huge `--max`, links, damaged files, 30 sessions, a long message; session-migrate on retired calls, dry runs, a bad
bool, damaged and linked files, a read-only file and folder, an id that cannot name a file; limbo-profiles and
limbo-validate on every answer shape (names only, details, skipped profiles over the cap, control characters, errors with
and without text, empty, not JSON, hang-ups, an answer over the bound, a run still running, failed, timed out, untrusted
output, a bad run id, wrong socket names, a huge and a negative `--timeout`); tiers status with no, damaged and invalid
configs, one to three tiers healthy, refusing and hanging up, claude tiers with and without a key or a model, disabled,
wake, resource limits, a stand-in nvidia-smi, the escalation flags, `SARINA_TIERS_FILE`; Heimdall's lock for ported
commands; standard output a closed pipe (SIGPIPE, also when the caller ignores it). CI runs it after the tests.

```bash
(cd go && go build -o /tmp/sarina-go ./cmd/sarina) && (cd rust && cargo build -j4 --offline --bin sarina-rs)
python3 -I rust/parity/cli-differential.py --go /tmp/sarina-go --rs rust/target/debug/sarina-rs [--only NAME] [--show]
```

The socket path must stay under 108 bytes: with a long temporary folder give `--out` a short one (or set `TMPDIR`).

### The chat cases (2026-11-12)

chat, resume, skill and memory-review are ported but not moved: sarina-rs runs them only when `SARINA_RS_TRY` names them
(the differential sets `chat,resume,skill,memory-review`) and `cli::chat::needs_go` finds none of the optional tool sets
the port lacks switched on (`SARINA_CRYPTO` or `SARINA_MATH` on/auto, `SARINA_QUANTUM`, `SARINA_REVERSE`, `SARINA_SYSTEM`,
`SARINA_FORGE` on, `SARINA_VAULT` set, a council file): with one of them the Go build would offer the model tools the
port does not have, so the Go build answers. For these cases the loopback server is the model: the chat completions take
the scripted answers (plain JSON, or a stream of `data:` lines), the context-window discovery's probes (`/api/ps`,
`/api/show`, `/props`, `/v1/models`) get 404. Compared as everywhere: output, exit status, every request (whole bodies:
the system prompt, the tool definitions, the history), every file (the session file, notes, scratchpad, memory, the
review queue); normalised: the session id and a memory proposal's id (random) and times. Covered: one turn, words and
flags, empty and streamed answers, a streamed draft sent back by a claim check, backend errors (500, 404, hang-ups),
tools (read, a refused and an allowed write, a secret file, an unknown tool, bad arguments, two calls, memory_add,
session_search, sarina_doctor, the max-turns stop, the budget stop), the allowlist and no tools, the prompt flags, a
schema, guidance files, plan mode (entered, a plan pending: exit 3), every slash command, skills (/skill, skill list and
run, hidden and unknown ones), interactive conversations (two turns, end of input, /exit, --interactive, the plan
question answered and revised, the memory review at the end proposing an entry), resume (own and broken files, the plan
flags on a session waiting on a plan, flags over a stored session, retired tool names, an old server), memory-review,
compute tiers (a fall-over to a second tier, /tier, a damaged file), the operator policy (and a damaged one), and Ctrl-C
at the prompt, after a turn (the review still runs), during a one-shot turn and during an interactive turn.

## The service differential (`rust/parity/service-differential.py`)

Both builds' `sarina service` on the same scripted steps: each scenario starts the Go build's service and then
sarina-rs's (`SARINA_RS_TRY=service`, `SARINA_RS_NO_GO=1`), each in a throwaway folder made afresh (the made-up home and
workspace of the CLI differential, whose helpers it imports), with the same flags and environment, on a port the kernel
picks (`--port 0` before the scenario's own flags: never 8899, where the real service listens). Behind it: one loopback
server for the model's chat completions (the context-window probes answered 404) and Heimdall's review and grant-request
API (`HEIMDALL_REVIEW_URL` always points there: a grant request would otherwise go to 127.0.0.1:8930), and Limbo's agent
socket. The steps: requests sent raw on fresh or kept-alive connections, sessions created, chats, resumes and deletes
(the pool's ids and the agent's session ids carried from answer to request), a client hanging up mid-turn, two turns at
once (on one session and on two), bodies of every form (Content-Length, chunked with extensions and trailers,
100-continue, HTTP/1.0, Connection: close, past 8 MiB, one that never arrives), SIGINT and SIGTERM. Compared byte for
byte: every answer (the Date header, the port, times and random ids normalised: the ids numbered in the order they
first appear), the service's stderr and exit status, every request the model, Heimdall and Limbo got, and every file
left (sessions, notes, scratchpads, memory).

The scenarios: the healthz and the pool (HEAD, a query, unknown paths and methods, delete twice); every create check
(missing, empty, blank, missing and file cwd; wrong types of each kind, numbers past int and float64, a top-level array,
null, duplicate keys, keys in any case, trailing data, `~`; the ceiling for shell, write, base_url and api_key; session
and scratchpad folders inside and outside; allowlists empty, named, unknown, duplicated; budgets); chat's checks and
the local /help (in any case, with a resume id), resume ids that are hostile, unknown, too long, of another folder; the
token (missing, wrong, other schemes, case, extra spaces, bytes that are not UTF-8) and the permits from flags and
environment; cwd roots (repeated flags, the environment's list, a link out); the browser guard (rebound names, loopback
forms, IPv4-mapped and IPv6 hosts, no Host in 1.1 and 1.0, Sec-Fetch-Site, Origin of every kind, absolute-form targets,
two Hosts, a malformed Host); 75 odd requests (malformed request lines, methods, versions, header lines, continuation
lines, NUL and non-ASCII values, a 2 MiB header, cut-off heads, cleaned paths and their 307s, escapes, `*` and
`OPTIONS *`, CONNECT, the HTTP/2 preface, bad, duplicate and signed lengths, transfer encodings, Expect, keep-alive and
pipelining, CR/LF after a POST, bodies past 8 MiB and past the 256 KiB drain); tools with Heimdall and Limbo (reads,
writes and bash refused then allowed, memory_add allowed and denied, limbo_profiles); backend failures (500, hang-ups),
the turn limit, a tool budget, no tools; the pool's eviction with resume from disk; session folders of their own; a
session folder that cannot be made (the 500 and its error type); the escalation flags with the grant gate; the command
line (help with secrets in the environment, bad flags, extra words, non-loopback without a token, ports out of range or
in use, localhost and ::1, the banner with every flag, the `sarina-service` name, Heimdall's lock, SIGINT, SIGTERM
during a turn). On 2026-11-12: 33 scenarios, 279 steps, no difference; a mutation run of the port (10 changes: the
hang-up's cancel, the chunking threshold, the guard's site rule, Connection: close, the drain limit, the LRU, the
Bearer prefix, the permits' default, the resume id, an empty chunk size) caught 9 of 10 (the tenth, dropping the empty chunk-size check, is equivalent: the hex parse refuses an empty size too). CI runs it after the
CLI differential (about five minutes: one case waits out the 30 s body deadline in each build).

```bash
python3 -I rust/parity/service-differential.py --go /tmp/sarina-go --rs rust/target/debug/sarina-rs [--only NAME] [--show]
```

## Running the tests

Every build, test and fixture run goes under a memory cap (a fixture writer once took 26 GB and the kernel's OOM killer
took the editor with it):

```bash
CAP="systemd-run --user --scope -q -p MemoryMax=8G -p MemorySwapMax=0 nice -n 19"
cd rust
$CAP cargo test -j4 --offline                 # every fixture, the properties, the fuzz bodies
$CAP cargo clippy -j4 --offline --all-targets -- -D warnings
cd ../go && $CAP go test -p 4 ./... -run Parity               # the fixtures are current with the Go code
$CAP env SARINA_PARITY_UPDATE=1 go test -p 4 ./... -run Parity  # after a deliberate change: rewrite them, review the diff
```

Never set `RUST_MIN_STACK` around cargo: rustc reads it too (a small value made it crash on 2026-11-11).

```bash
```

## Resume here

Done (2026-11-11 night and 2026-11-12): phase 0; the workspace and the fixture writer; step 1's leaves (`shellparse`, the
bash guards, `ValidateArgs`, the file tools' guards and path resolution, `testguard`, `pyfmt`, `flagx`, `version`); then
`gojson` (Go 1.27's JSON), the session file with config's loaders and llm's types (and a read-only run over a private copy
of 9,839 real session files: Go and Rust wrote the same bytes for every one; the copy was deleted), the permission gate
and the registry, the tool definitions, vecindex with Go's math/rand and math.Exp; and in the second half of 2026-11-12
`trust` (ed25519 through `ring`), Heimdall's grants gate with `AccessTierFromEnv`, `policy`, `guard`'s detectors,
`analysiscache`, `llm`'s repair and normaliser, `session.Save` and `MigrateDir`, `sessionsearch` (with Go's amd64
`math.Log`), `agents`, the skill file format, plus `gostruct` (Go 1.27's Unmarshal into a struct), `gotime` (Go's loose
RFC 3339) and `goos` (MkdirAll, CreateTemp, WriteFile). About 42,000 fixture cases in all, a mutation run for each piece (the
cases widened where one got through), fuzz targets for every reader (17 now). CI green through the last push.
Go code changed, each a bug the port found, fixed in Go and Rust both: `go/vecindex` (`28b9c3f`), session ids and
MigrateDir, analysiscache's non-finite values (`261834a`), embed's answer bound (`a4a6f72`), the Python helper's pipes
(`f013ceb`), Limbo's over-long answer (`49b598e`), the model clients' bounds (`9aba2f6`); otherwise only new
`*parity_test.go` files and the test-only `go/internal/parity`.

Later on 2026-11-12: two bugs the port found were fixed in Go and Rust (`261834a`: session ids that cannot name a file
are refused by Save and skipped and reported by MigrateDir; analysiscache refuses values JSON cannot hold), and step 3
began: `gohttp`, Heimdall's review client (`Consult`), `embed` (its answer now bounded in both), `helpers` (its pipes
bounded in both), `limbo`'s client (its long answers named in both) and the OpenAI-compatible wire format (Go's
model clients bounded first), proved against Go's client on raw answers from a loopback server (`go/internal/parity/rawserver.go`, `tests/common` `raw_server`).

Second half of 2026-11-12: step 3 finished but MCP's loader and manager. `llm`'s transport (`Complete` with the retry,
fallbacks, the timer and cancellation, over `gohttp` with TLS through rustls and the system's authorities, proved on
scripted loopback servers with a test authority each test makes), the Anthropic client, the context-window discovery
(with Go's `url.Parse` for `serverRoot`), the MCP stdio client against one shell stand-in server; `goctx` (Go's context:
cancel, deadline, sockets shut on cancel). Fixtures `transport` 45, `anthropic` 80, `discover` 76, `mcp` 19 cases
(42,486 in all); mutation runs 6/6, 7/7, 8/8, 12/12 after widening; 18 fuzz targets. Bugs fixed in Go and Rust both: the
Anthropic client's errors never hold the API key now (an error in a 200 answer and the timeout text kept it,
`2170366`); an MCP server's own requests are answered at most eight at a time (a flood started a goroutine each), and a
dying server's stderr is waited for (`62f59ef`). CI found two timing-dependent fixture cases on a busy runner; the
fixtures now settle before counting requests and print the first line that moved (`bae7378`, `6a2ebc1`).

Late on 2026-11-12, step 4 began with the tools: the file tools (`read_file`, `list_dir`, `write_file`, `edit_file`,
`glob_search`, `grep_search`, with Go's `WalkDir`, `ReadDir` and `path.Match`), `bash` with background processes,
`TaskOutput` and `TaskStop` (the child read with poll(2) and its pidfd, bounded, its group killed), `git_read` (against
a stand-in git in bash's builtins) and the plan-mode tools. The registry has `execute_stream` now, and `Context` the
workspace, output limit, step, file state, cancel context, timeout, environment layers, process manager and plan-mode
probe. Fixtures `files` 178 calls + 770 glob matches, `bash` 46 calls + 3 environments, `git` 40 calls, `plantools` 21;
mutation runs 13/14, 11/11, 6/6 (the rest equivalent); a fuzz target for glob patterns (19 now). Faults found and fixed
in Go and Rust both: a refused `write_file` (or a failed `edit_file`) let the same write replace a file never read; a
line-range read held every line asked for; `write_file` onto a named pipe and `grep_search` given one waited forever
(`b6fb83e`); bash's timeout message said "0s" for the default 30 s (`2859309`); `git_read` read git's output and the
config listing without a bound and ran rev-parse before the risky-settings check, unfiltered and without a time limit
(`280d48b`). The bash and git fixtures run bash's builtins only, with nothing but bash (or the stand-in) on PATH. Then the
to-do and plan tools (`todo.rs`, 77 calls), after a Go fix: a `.port_sessions` link planted in a workspace led the to-do
list and the plan out of it. Then the checks after a code change and `check_code` (`verify.rs`, against one stand-in
for every program; the bubblewrap command line recorded as data), after a Go fix: their programs' output was collected
whole (now the first and last MiB). Then skills' discovery, the skill tool and the long-skill
library (`skills/`, its saved indexes byte for byte Go's), with skills.json read through the 4 MiB cap in both. Then MCP's definitions loader and manager
(`mcp/config.rs`, `mcp/manager.rs`, against the client fixture's stand-in server). Then the limbo tools. Then memory
(`memory.rs`), after a Go fix: a link planted at the agent memory file put the file it named into every session's
prompt (`315b10b`). Fixtures this part: `todo` 77, `checks` 124, `skills` 122, `mcpmanager` 53, `limbotools` 21,
`memory` 31 comparisons; CI green through each push.

Second session of 2026-11-12 (afternoon): compute tiers (`tiers/`: the config with Go 1.27's struct reading, the
redactor and Sanitize, the sampler, the claude CLI hand-off in its own process group, the wake runner, the Router with
Heimdall's grant gate and `RequestGrant`), proved on 37 Router scenarios against scripted model servers and a scripted
Heimdall and on a claude stand-in in bash's builtins (`tiers.json` 548, `tiersrun.json` 328 comparisons; mutation runs 13
of 14 each, the rest equivalent); `StateDir` (`statedir.json` 216); memory's review queue and the memory commands in a new
`cli/` module (`review.json` 236, `memorycmd.json` 31; mutation 9 of 9). Go fixes, each in Go and Rust with a Go test that
failed before: the claude CLI's and nvidia-smi's output bounded (`f8f0f56`); session files, notes and scratchpads kept in
the workspace against a planted `.port_sessions` link, and session_search kept off the folder it pointed at (`15627ba`).
The switch plan is written ("The plan for `sarina-rs`" above). CI green through `483a1cf` (`7b40b29` failed only `cargo fmt --check`).

Third session of 2026-11-12 (evening): the agent loop, all four steps. (a) `claims.go`'s checks and nudges and
`secrets.go` (`agent/claims.rs`, `agent/secrets.rs`; `claims.json`, 1.9 million answers in 52 tasks, mutation 17 of 17
after the corpus was widened). (b) The pure helpers (`agent/limits.rs`, `prompt.rs`, `compact.rs`; `agentpure.json`, 368
comparisons, mutation 26 of 26). (c) and (d) The loop itself (`agent/run.rs`: the drive loop, tools, nudges, the claim
check, budgets, cancellation, notes and summaries with the model, plan decisions, resume, sub-agents; the tiers Router and
the context-window discoverer plug in), proved by running the Go agent and the port against the same scripted answers
from a loopback server (`agentrun.json`, 41 tasks, 794 comparisons, mutation 23 of 24). Go fixes, each in Go and Rust
with a Go test that failed before: the git snapshot's and the guidance files' reads bounded (`8d97da5`). A timing flake in
the MCP fixture fixed (`80ea200`: the over-long line's call gets 3 s like the others). CI green.

Fourth session of 2026-11-12 (afternoon): the CLI differential (`rust/parity/cli-differential.py`, "The CLI
differential" above; 195 cases, 975 comparisons, no difference; in CI) and the commands without a model moved to
sarina-rs (`cli::ported`: memory-status, memory-show, session-search, memory-review-list/-approve/-reject,
session-migrate, limbo-profiles, limbo-validate, tiers status), with `SARINA_RS_NO_GO=1` making an unported command fail
in the differential. Every case agreed on the first run once the commands were in the table; nothing found in Go. Fixed
in Rust only: limbo-validate's deadline for a huge `--timeout` overflowed (a panic in a debug build), now wrapping as
Go's does. Then Go's per-call operator confirmation (`agent::Options.confirm`, `Agent::set_confirm`,
`tools::registry::Context.confirm`), the channel council_ask will use. Commits `eba8bc8`, `320b139`.

Fifth session of 2026-11-12 (evening): packaging and chat.

- **Packaging**: Sarina's `packaging/arch/PKGBUILD` (main) and both copies of Labyrinth's sarina-git recipe (branch
  `sarina-rs-switch`, `de022de`, not on main: Labyrinth's publish builds main, so merging is the switch) build both:
  `/usr/bin/sarina` is sarina-rs with its two links, the Go build is `/usr/lib/sarina/sarina`, `options=('!lto')` (ring
  does not link from GCC's LTO objects), check() also runs `tests/sarina_rs.rs`. The Go build's file is called `sarina`,
  not `sarina-go` as first planned: after the hand-over the process's name and executable are the Go build's, and
  Heimdall knows Sarina's processes by the name `sarina` (item 3 of the plan above). Built without root
  (1.0.0-86 in `~/.cache/labyrinth-switch`): the installed 1.0.0-82's file list plus `usr/lib/sarina/sarina`; sarina-voice
  byte for byte the installed one. Labyrinth's Heimdall template lists `/usr/lib/sarina/sarina` for file integrity (an
  existing `/etc/heimdall/config.json` needs it by hand). CONTRACT entry on Labyrinth's board (`69b307e`).
- **Go fix, in Go and Rust, each with a Go test that failed before**: a named pipe under a session file's name hung
  resume, session-migrate and the session_search tool, and the notes the same; any size was read. Now a bounded regular
  file (`session.ReadFile`, 64 MiB; notes 1 MiB; `45d306d`).
- **Go fix, in Go and Rust, with a Go test that failed before**: the scrubber the service and the GUI put on everything
  they send out never matched a secret-named key in JSON (`{"password":"..."}`, the form tool-call arguments take): the
  key's closing quote stands before the colon. Now replaced inside its quotes, so the arguments stay JSON
  (`TestScrubberJSONKeys`).
- **runkit in Rust** (`runkit.rs`): the scrubber, the transcript and result as both servers send them, and Build, proved
  on `runkit.json` and `runkitbuild.json` (every request each built agent sends a scripted model; a mutation that drops
  the policy's deny-list fails it).
- **chat, resume, skill, memory-review in Rust** (`cli/chatopts.rs`, `cli/chat.rs`; `chatopts.json` 37 cases), proved by
  the CLI differential's chat cases ("The chat cases" above). Not moved: they run in Rust only under `SARINA_RS_TRY`.
- Two CI timing flakes in fixtures fixed (MCP's long lines `a9b0b6d`, the helper stand-ins `9f5adb4`).

Sixth session of 2026-11-12 (night): the service.

- **The service differential** (`rust/parity/service-differential.py`, "The service differential" above): both builds'
  services on the same raw requests against a scripted model, Heimdall and Limbo; 33 scenarios, 279 steps, no
  difference; in CI. Run Go against Go first to find what is not deterministic (the ids, numbered by first appearance).
- **service in Rust** (`service.rs`, `gohttpd.rs` for Go's net/http server, `cli/servicecmd.rs`), proving: it runs in
  Rust only under `SARINA_RS_TRY=service` (the differential's setting), as chat does. The packaged unit still reaches the
  Go build until `service` moves to `cli::ported`. `runkit` now shares one context-window discoverer per process (Go's
  `DefaultDiscoverer`): the service builds an agent per session and Go's cache is shared.
- **Go fixes, in Go and Rust both, each with a Go test that failed before**: `sarina chat -h`, `sarina-service --typo`
  (into the journal), `sarina gui -h` and `chymaera-console -h` printed the API key and tokens, which were the flags'
  default values (`b6a597f`); the service waited for ever for a request body that never came (also after answering,
  draining it) and kept idle connections for ever: now a 30 s body deadline per request and a 120 s idle timeout
  (`b6e62aa`), and `OPTIONS *` goes through the service's own handler instead of net/http's, which read its body without
  any deadline (`f85e3cf`; the deadline only for a request with a body, `a9e7278`). A body that fails for another reason than its size is "400 request body could not be read" with the
  connection closed (it was "413 request body too large").
- **The same two bounds in the GUI's server** (`go/gui`, Go only until it is ported; `e38dca6`): the body deadline only
  for a request with a body (the event stream's GET must live on: a deadline reaching net/http's background read cancels
  the request), the idle timeout, `OPTIONS *` through its own handler; tests that failed before.

Next, in this order:

1. **Move chat, resume, skill, memory-review and service** (from `cli::proving` to `cli::ported`) once the owner has run
   the switch package (Labyrinth's branch) and used the moved commands for a while; `needs_go` keeps handing the optional
   tool sets to Go until they are ported (step 5). For the service the package needs nothing more:
   `sarina-service.service` runs `/usr/bin/sarina-service`, which is sarina-rs. Owner steps for the switch: the board
   entry on Labyrinth's TEAM_BOARD (2026-11-12).
2. **gui** on `runkit::build` and `gohttpd` (SSE streaming, the approval flow, the page's token, the launch link):
   first its differential, as the service's (a scripted model; the page's requests raw); the bounds above come with it.
3. Doctor (last of the core): its check-up, so `sarina_doctor` stops running in the Go build.

Notes for whoever continues: tool calls that write files can turn a literal `\u....` escape into the character itself
(it happened to a test's expected string here); build such strings from parts or check them with grep. The timed client
fixtures take about a minute of the Go parity run.

The mutation script used in this session copied the file back with `cp`/`mv`, which leaves the restored file older
than the last (mutant) build, so cargo did not rebuild the first test run after a series: touch the file after restoring
(CI builds fresh, so nothing pushed was affected).

Rules for whoever continues: every build, test and fixture run under the memory cap above (`-j4`, `-p 4`, nice 19);
keep fixtures small and bounded (no input that makes Go allocate by a count it reads); pull before each commit (another
helper works in `packaging/`); stage by explicit path.
