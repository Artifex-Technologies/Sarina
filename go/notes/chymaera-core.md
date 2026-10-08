# Chymaera core: console client, firmware advisory, capture analysis (status)

Ported from the owner's ORIGINAL modules `sarina/chymaera/{console,firmware,analysis}.py` and their tests, plus
`docs/ECOSYSTEM.md`, spec 00 and 04-cli.md section 4.2. No derived file under `sarina/` or `tests/` was read
(so the exact stdout/stderr wording of the Python CLI wrappers in `main.py` was not available; see deviations).
Standard library only. `go vet` and `go test` pass for the packages below.

| Package | What it holds |
| --- | --- |
| `chymaera/console` | NDJSON-over-loopback-TCP client (default 127.0.0.1:44700, 5 s per I/O, optional token). Same errors as Python: `*Error` (`cmd: message`), `*UnavailableError` (refused, hang-up, malformed JSON, non-object, timeout). Ids start at 1; nil params omitted; one request per connection unless `Connect()` was called. Commands: ping, help, status, log.tail, log.add, session.start/end, capture.list/import. `RenderStatus` as Python. |
| `chymaera/firmware` | `ParseDeviceInfo`, `CheckDeviceInfo` (stock fork, enclave, radio, `unknown` version = the only version warning, `<upstream>-CHY-<ours>[+ahead]` notes, legacy `-N-gHASH` recognised, commit prefix match), `ClassifyPath` (area label only), `MergeImpact` (shells out to `git merge-base`, `diff --name-only`, `ls-tree -r --name-only`; tier by upstream existence: high = exists and changed upstream, medium = exists, low = absent), policy alarm over 12 non-additive files, report text identical to Python. |
| `chymaera/analysis` | statistics path only: counts by type, duplicate NFC UIDs, frequency groups, rare (seen once) Sub-GHz frequencies, from each record's `meta` JSON string (numbers keep their written digits). |
| `chymaera` (commands.go) | registers `chymaera-console`, `chymaera-analyze`, `chymaera-device-info`, `chymaera-merge-report` via `cli.Register`. Import for side effects (`cmd/sarina/main.go` has the one added line). |
| `chymaera/internal/pyfmt` | Python `str`/`repr`/truthiness helpers so report text (`True`, `'x'`) matches the original. |

Exit codes: console/analyze 1 on console failure, 0 otherwise; device-info 0 iff no warnings; merge-report 1 on git
failure. Flag parse error exits 2 (`-h` exits 0).

## Deliberate deviations / open points

- **Semantic grouping is not ported** (needs onnxruntime). No `sarina_helpers.embed_group` exists in the repo, so none is
  invoked or invented. `chymaera-analyze --model-dir D` prints `semantic grouping unavailable: <reason>` to stderr and the
  report carries `Semantic grouping unavailable: <reason>`; statistics still run, exit 0. `--similarity-threshold` is
  accepted and ignored. The no-model reason text differs from Python (no `pip install` hint).
- Error messages from the commands go to **stdout** (Python's `print`); the spec says "print the error message" without a
  stream. Easy to switch if the Python wrapper used stderr.
- `chymaera-device-info` with an unreadable `--file` prints the OS error and exits 1 (Python behaviour not specified).
- Go's `flag` package accepts `-x` as well as `--x`, and stops at the first positional argument.
- Python's `analysis.py` also exposed `frequency_groups`; kept as `FrequencyGroups` (ordered slice, first-seen order).
- Tests run a real loopback server (console), real temp git repos (firmware; skipped if `git` is missing), and drive
  the commands through `cli.Main`. Not run here: `go test ./...` as a whole, because another agent's `chymaera/fleet`
  package was mid-edit (`runCLI` undefined in its tests) when this was written.
