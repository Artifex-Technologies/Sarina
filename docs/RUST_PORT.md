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
| 2 | the file formats: session files, config loaders, llm message types, vecindex, trust, policy, analysiscache | **in progress**: gojson, the session file, config's loaders, llm's types, the permission gate and registry, the tool definitions, vecindex (with Go's math/rand and math.Exp) done 2026-11-12 |
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
| `vecindex.rs`, `gorand.rs`, `gomath.rs` | `vecindex` (rotation, levels, Add, Remove, Search with container/heap's tie order, the SVEC1 file), `math/rand` with a seeded source (table generated from the Go source), amd64 `math.Exp` on both its paths | `vecindex.json` | 93 (1,029 comparisons) |
| `trust.rs`, `gostruct.rs` | `trust` (Go's base64, `ed25519.Verify` through `ring` with small-order and non-canonical keys and S+L, the signed list read once, `Refuse` on a made-up tree, `Contained`), Go 1.27's `json.Unmarshal` into a struct (case-insensitive keys with Go's two special folds, merging duplicates, null, the first type error) | `trust.json` | 134 (47 trust folders x 21 paths) |
| `heimdall.rs`, `gotime.rs`, `config.rs` | `heimdall/grants.go` (`LoadGrants`: link, owner, others' write, the folder, unreadable, a folder, a path under a file; `TierCap`, `EscalationAllowed`), `config.AccessTierFromEnv` (19 values of SARINA_ACCESS_TIER against 66 grants states), Go's `time.Parse(time.RFC3339)` as JSON reads a time, with its error texts | `accesstier.json` | 131 (1,584 answers) |
| `policy.rs` | `policy` (`Load`: operator files, SARINA_POLICY_FILE's list with duplicates, missing files, folders, unreadable and oversized files, the tighten-only workspace file, the trust check; `Denied`, `DenyRules`, `ApplyBudget` on three starting budgets); each case run 60 times in Go, every distinct answer kept (map order) | `policy.json` | 93 (25 tools, 3 budgets each) |
| `guard.rs`, `gotext/cf.rs` | `guard`'s local checks (`Gate.Scan` before the Heimdall call): invisible and format characters (Go's `unicode.Cf` table, written by the Go test), the leading BOM, the 15 instruction patterns with the match quoted, the 12 credential shapes; on the Go tests' inputs of guard, memory, skills and the vault, seeds changed in case, spacing and length, every gap at its limit, bytes that are not UTF-8 | `guard.json` | 3,347 (two gates each) |
| `analysiscache.rs`, `goos.rs` | `analysiscache` (`New` over files of every shape, `Get`, `Set`'s file byte for byte, its mode, its errors: `MkdirAll`, `CreateTemp`) | `analysiscache.json` | 39 |
| `llm/repair.rs`, `llm/normalize.rs` | `llm/repair.go` (`RepairJSON`: fences, trailing commas, single quotes, raw control characters, closing what was cut off; `ParseArguments`) and `normalize.go` (`<think>` split, `<tool_call>` blocks, the python tag, bare JSON calls, cut-off arguments on a length stop, unique ids), on the Go tests' inputs, tool calls in every shape, every cut of each, generated damage | `repair.json` | 4,021 |
| `session/save.rs`, `goos.rs` | `session.Save` and `writeAtomic` (the folder made, the temporary file, the mode from SARINA_SESSION_FILE_MODE, the rename; 12 sessions with odd ids x 6 modes x 4 folders) and `MigrateDir` (dry and real runs twice, backups, skipped files, a link, a `.bak` folder, an id with a slash), compared as whole folder listings | `save.json` | 301 |
| `sessionsearch.rs`, `gomath.rs` (`log`), `gotext/letters.rs` | `sessionsearch` (`Search`: files newest first, the lenient reading of messages, the phrase BM25 and the word count, snippets), Go's amd64 `math.Log` (math/log_amd64.s, 1,700 values), Go's letter and digit tables (written by the Go test) | `sessionsearch.json` | 175 |
| `agents.rs` | `agents` (`Parse`: the front matter, its keys, quotes, lists, switches and limits, each document 30 times for Go's map order; `Load`: operator folders, SARINA_AGENTS_DIR, overrides of the built-ins, duplicates, links, folders, oversized and unreadable files, the trust check, the text gate; `Get` with Go's case folding) | `agents.json` | 174 |
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
- **analysiscache keeps a value Go cannot write.** `Set` puts the value in memory before encoding the file, so after a
  NaN or an infinity every later `Set` of that cache fails with "json: unsupported value: NaN" and nothing more is
  written in that process. The port does the same (the fixture records it); worth fixing in both when the cache is next
  touched.
- **Normalize renames a server's own id.** Ids are made unique in order, and a generated `call_<i>` (or `call_<i>_<n>`)
  can take an id a later call brought itself, which is then renamed too (`call_1`, ``, `call_1_1` becomes `call_1`,
  `call_1_1`, `call_2`). Harmless (the ids only pair calls with results within the message); kept as Go does it.
- **Saving a session id with a slash fails, by accident, and MigrateDir stops there.** `Save` joins the id into the path
  (`../x` would leave the folder) but Go's `CreateTemp` refuses the pattern `<id>.*.tmp` because it has a slash, so
  nothing is written: the property test `save_stays_in_its_folder` holds the port to that. `MigrateDir` meets such a file
  after writing its backup and stops with the error, every run, so the files after it are never brought up to date.
  Also as Go: a `.bak` that exists as anything (a folder too) means no new backup, and a session file that is a link is
  replaced by a regular file. All kept; worth an explicit id check in both later.
- **Texts that are not UTF-8 in profiles become U+FFFD when read.** Go keeps a profile's name, description, prompt and
  tools as the file's bytes; the port keeps strings, so a byte that is not UTF-8 is U+FFFD at once, which is what Go's
  JSON encoder writes when those texts reach the model. Error texts quote the original bytes as Go does.
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
and the registry, the tool definitions, and vecindex with Go's math/rand and math.Exp. 33,207 fixture cases in all, a
mutation run for each piece, fuzz targets for every reader (`json`, `session`, `vecindex` new). CI green through
`180d0de`. Go files changed: only new `*parity_test.go` files and the test-only `go/internal/parity`.

Nothing switches yet: the Rust code is a library with tests, no binary. Next, in this order:

1. The rest of step 2: `trust` (ed25519 against Heimdall's signed list: `ring`), `policy`, `analysiscache`, `guard`'s
   detectors, the `agents` and `skills` file formats, `sessionsearch`, `llm`'s pure parts (`repair.go`, `normalize.go`),
   `session.Save` (the atomic write) and `MigrateDir`, `config.AccessTierFromEnv` with Heimdall's grants cap
   (`go/heimdall/grants.go`: a fixture of every tier against the cap missing, damaged, lower, higher).
2. Step 3, the clients, starting with `heimdall` (review) and `embed`.
3. Which tools a session offers (the providers' switches and their order) comes with the tools in step 4; `defs.json`
   already holds every definition.

Rules for whoever continues: every build, test and fixture run under the memory cap above (`-j4`, `-p 4`, nice 19);
keep fixtures small and bounded (no input that makes Go allocate by a count it reads); pull before each commit (another
helper works in `packaging/`); stage by explicit path.
