# Chymaera Ecosystem — integration contract

Sarina is one of five repos. It is the only one that touches all the others, so
this file records how they fit together **from Sarina's side**.

Status keys: **live** (implemented and verified), **agreed** (both sides accepted
the shape, not yet built), **proposed** (awaiting the owning repo's answer),
**open** (nobody has proposed anything).

| Repo | Role | Owns |
| --- | --- | --- |
| [`Chymaera_Linux`](https://github.com/Artifex-Technologies/Chymaera_OS) | Arch-based security distro; the host | ISO profile, package set, the packaging contract |
| [`Chymaera_Zero_Firmware`](https://github.com/Artifex-Technologies/Argus_Firmware) | Flipper Zero firmware fork | RPC wire format (`assets/protobuf/`), the BLE budget |
| [`Chymaera_Zero_App`](https://github.com/Artifex-Technologies/Argus_App) | qFlipper fork; desktop operator console | **The Sarina control API**, datastore, exporters |
| [`Chymaera_Sentry`](https://github.com/Artifex-Technologies/Heimdall) | Go security agent watching Sarina | Its own detection engine; reads Sarina's session files, read-only |
| `Limbo` | Sandbox that runs guest VMs (Linux/Windows) as seamless windows; quarantine and validation runs | The agent socket Sarina uses (section 8) |
| `Sarina` (this repo) | Local-model agent shipped in the distro | The client/advisory surfaces below |

> **Each contract is owned by exactly one repo, and it is usually not this one.**
> Where a sibling has published a spec, that spec wins and Sarina conforms. This
> file records what Sarina implements against, not what Sarina wishes were true.

---

## 1. Sarina ↔ Chymaera_Linux — **live**

Sarina ships *in* the distro, as a package built from a submodule.

### The packaging contract (owned by Chymaera_Linux)

`packages/pkgbuilds/sarina-git/PKGBUILD` builds this repo directly and **breaks
if any of these move**. Verified present as of this commit:

| Path here | Used for |
| --- | --- |
| `pyproject.toml` | `python -m build --wheel --no-isolation` |
| `packaging/arch/sarina-service.service` | → `/usr/lib/systemd/system/` |
| `packaging/arch/sarina.sysusers` | → `/usr/lib/sysusers.d/sarina.conf` |
| `packaging/arch/service.env.example` | → `/etc/sarina/` |
| `README.md` | → `/usr/share/doc/sarina-git/` |

**Entry points the ISO expects on `PATH`** — all three verified resolvable:

| Console script | Target | Status |
| --- | --- | --- |
| `sarina` | `sarina.main:main` | present |
| `sarina-gui` | `sarina.gui.__main__:main` | present |
| `sarina-service` | `sarina.service.__main__:main` | present |

**Runtime dependencies must stay in sync and must exist in Arch's repos.**
This repo declares `fastapi`, `uvicorn`, `pydantic`; the PKGBUILD declares
`python`, `python-fastapi`, `uvicorn`, `python-pydantic`. Note Arch's uvicorn
package has **no `python-` prefix** — that mismatch has already broken the ISO
build once. A PyPI-only dependency needs its own PKGBUILD in the distro first,
so **adding a dependency here is a cross-repo change**: say so in the commit
message with `CONTRACT:` so the distro side can grep for it.

### What the distro needs from Sarina — **open, not built**

The distro ships a **small hand-picked tool core**, not `blackarch-*` category
groups. The groups were tried and abandoned (thousands of packages, unresolvable
`lib32-*` deps, provider menus, an ISO too large for CI). Everything past that
core is explicitly Sarina's job:

1. **Install-time tool selection** — ask what the machine is for and install
   accordingly, instead of a guess baked into the ISO.
2. ~~**Categorisation**~~ — **built, see below.**
3. **Data analysis** — making sense of scan and capture output in place. This is
   the reason the agent is in the OS at all.
4. **`blackarch_compat` against a real Docker daemon** — it is unit-tested only
   and has *never* run against real Docker. It is the safety check behind (1);
   until it has actually run, treat any category it reports as unvalidated.

5. **Crash diagnosis** and 6. **Proton VPN lifecycle** — added by the distro,
   **open, not built** (see its `CLAUDE.md`).
7. **Agent manager** — **built**, see "Agent fleet" below.

> **Correction to an earlier version of this file:** it described
> `blackarch_compat/` as validating the distro's category selection. The distro
> no longer installs category groups at all, so that harness now serves (1)
> above — vetting categories *before* the agent proposes them — not the shipped
> package list.

Do not add tools to the distro's `packages.x86_64` to work around the gap; that
re-creates the problem the categories caused.

### Menu categorisation — **live** (`sarina desktop-audit`)

`sarina/desktop/` answers ask (2). Read-only by default: it reports, and writes
only with `--apply`. **General-purpose, not security-specific** — the problem is
any package shipping a bare binary; BlackArch is where it is worst.

Two jobs, matching the distro's own split:

- **Repair** — an entry exists but carries no registered Main Category, so it
  has no menu location. `desktop-file-validate` states this directly for a real
  `burpsuite.desktop`: *"does not contain a registered main category;
  application might only show up in a catch-all section"*. The 52
  `blackarch-*` groups and their `X-BlackArch-*` desktop tokens are mapped to
  spec-valid categories.
- **Create** — the package ships no `.desktop` file at all.

**Three signals, cheapest first**, so a run with no model is still useful:
pacman *group* → *description* keyword rules → optional semantic match against
prototype vectors (`--model-dir`). Measured against Arch's real repos, the
**description is usually the only signal that exists**: all 11 security tools
sampled from the distro's core (`nmap`, `sqlmap`, `hydra`, `metasploit`, …) come
from `extra`, ship **zero** `.desktop` files, and have `Groups: None`. The group
path only fires for packages actually installed from `[blackarch]`.

**Two filters do most of the work**, and removing them fills the launcher with
noise:

| Filter | Why |
| --- | --- |
| `NoDisplay=true` / `Hidden=true` entries are never touched | 93 of 108 entries on a minimal Arch install are hidden service files |
| only explicitly-installed, application-shaped packages | 35 of 725 installed packages are explicit; `qt6-base` otherwise scores well enough on description alone to be proposed |

Reversible by construction: writes go to the **user** application directory (no
root, survives upgrades), every file carries `X-Sarina-Generated`, and
`--remove-generated` deletes exactly those — verified to leave hand-written
files alone. A repair rewrites one line of a *copy* of the original, preserving
`MimeType`, desktop actions, and translated names, because a user-level file
replaces the system one outright rather than merging.

**Open question for the distro:** the emitted `Security` token is what a
dedicated Plasma Security menu would match on, but freedesktop says `Security`
should be paired with `Settings` or `System`, so `Network;Security;` draws an
informational hint (validation still exits 0). Pairing every tool with `System`
would silence it at the cost of listing every security tool in the System menu
too. **Left as-is pending the distro's menu design** — if you want a dedicated
Security menu, you need an XDG `.menu` file plus a `.directory`, which is Plasma
configuration and belongs on your side. Also note the ISO's MacTahoe icon theme:
generated entries currently set no `Icon`, so they take a generic fallback.

---

### Agent fleet: Omarchy's agents panel — **live** (`sarina fleet-*`)

Answers `Chymaera_Linux` contract item 7. The distro adopts
[Omarchy](https://github.com/omacom/omarchy)'s desktop, whose bar has one panel
showing every AI agent on the machine. The panel is a pure display: each agent
is one JSON record in `~/.local/state/omarchy/agents/usage/`, and **any file
that lands there becomes a tab**. Three steps, built in increasing order of
power and risk:

1. **`sarina fleet-usage`** — prints (or, with `--write`, writes)
   `sarina.json` from Sarina's saved sessions, in the exact shape Omarchy's own
   Codex collector prints (verified against `bin/omarchy-agent-usage-codex` at
   Omarchy v4.0.4). Omarchy's panel regenerates records by running every
   `omarchy-agent-usage-*` script in `$OMARCHY_PATH/bin` and writes each script's
   **stdout** to `<agent>.json` itself, so the distro ships a one-line wrapper,
   `omarchy-agent-usage-sarina`, that runs `sarina fleet-usage` (no `--write`;
   that flag is for standalone use, e.g. a timer). Session totals are cumulative, so a session's
   tokens are attributed to the local day its file was last written.
2. **`sarina fleet-status [--suggest]`** — **read-only.** Reads every record in
   that directory (Claude Code, Codex, ...) into one view of usage and limits,
   and optionally names the agent with the most headroom. **The records are
   untrusted data written by other programs:** control and bidi characters are
   stripped, text is truncated, ids must match `[a-z0-9][a-z0-9_-]{0,31}`,
   numbers are clamped, files over 1 MiB are skipped. Unknown headroom is never
   ranked as unlimited.
3. **`sarina fleet-launch <agent> [prompt]`** — **proposal-first.** Omarchy
   starts agents in their auto-approve modes (`claude --permission-mode auto`,
   `gemini --yolo`, ...). This launcher has **no way to add such a flag**; each
   agent asks before acting. It prints the proposal, needs a human's `y` (or
   `--yes`), refuses if the prompt changed after the proposal, and appends every
   attempt to `~/.local/state/sarina/agent-launch.jsonl` for Chymaera Sentry to
   read — prompt as a SHA-256, a length and a 60-character preview, never in
   full. **It is a CLI command only and is deliberately not a model tool**
   (`tests/test_chymaera_agent_launch.py` checks the tool layer never imports
   it), so a prompt-injected model cannot trigger it. `crush` and `openclaw` are
   unsupported and say why.

Not yet verified against a running Omarchy shell: whether the panel draws
Sarina's tab correctly. The record matches the fields the panel's QML reads,
but that was read from source, not observed.

## 2. Sarina ↔ Chymaera_Zero_Firmware — **live (read-only)**

The firmware project set the boundary: *"anything that writes to hardware should
stay behind explicit human approval for now."* Sarina honours it.

**Implemented** in `sarina/chymaera/firmware.py`:

| Command | What it does |
| --- | --- |
| `sarina chymaera-device-info [--file F] [--expected-commit SHA]` | Parses captured `device_info`; warns on stock firmware, invalid enclave, dead radio, a version git could not produce, or a commit that does not match the tree. Notes a development build without failing. Exit 1 on any warning. |
| `sarina chymaera-merge-report --cwd <firmware> [--upstream-ref upstream/dev]` | Reports what merging upstream would touch, **without merging**, using the existence test below. |

### Merge risk — decided by upstream existence, not path

The firmware repo corrected an earlier version of this classifier: risk is not
which directory a path sits in, it is **whether the path exists upstream**. A
file absent from `upstream/dev` cannot produce a merge conflict no matter where
it lives — module-style code (`menu_wii.c`) has to sit inside an upstream
directory for its `#include` to resolve, but is still a new, non-conflicting
file. `merge_impact()` now tests existence directly
(`git ls-tree -r --name-only <upstream_ref>`), not a path prefix:

| Tier | Test | Meaning |
| --- | --- | --- |
| `high` | exists upstream **and** upstream also changed it | will conflict |
| `medium` | exists upstream, upstream did not move it | conflicts only if upstream moves it later |
| `low` | does not exist upstream | additive, cannot conflict |

A path-prefix `area` label (`wire-format`, `core`, `build`, `external-app`,
`docs`) is still attached to each result for context, but it no longer drives
the tier. If the count of `high`+`medium` files exceeds
`REBASE_POLICY_BUDGET` (12 — the firmware project's own number, "roughly a
dozen"), the report prints a policy alarm: that many non-additive first-party
files means the "add directories, don't edit upstream files" policy is being
violated, which matters more than any single file's risk.

The firmware repo confirmed `applications_user/` as the common case (existing
classifier assumption unaffected there) and named the current complete
medium/high set for their tree: `scripts/version.py`,
`applications/services/loader/loader_menu.c`, `fbt_options.py`.

### The BLE budget — a hard constraint on anything Sarina ingests

Streaming targets **BLE to a laptop running Chymaera Linux**, not USB. The
firmware repo owns these numbers:

| Constant | Value |
| --- | --- |
| Bytes per GATT notification | 243 |
| Max per TX call | 486 |
| Connection interval | 7.5 – 45 ms |

That is a ceiling of roughly **5–30 KB/s** — about an order of magnitude *worse*
than USB CDC — and it is to be **measured, not designed against**.

**What this forbids Sarina from building:** any ingest path that assumes raw IQ,
full 802.11 capture, or a live spectrum waterfall. What fits: decoded events,
RSSI sample streams, BLE advertisement records, scan tables, periodic telemetry.
Reduction happens on-device; where fidelity matters, the device captures to SD
and the file is pulled over RPC storage afterwards.

### Transport tiering — Sarina must check the tier before invoking

The firmware tiers every feature by transport rather than pretending the two are
equivalent:

| Tier | Transports | Covers |
| --- | --- | --- |
| **USB-only** | USB CDC | bulk capture retrieval, SD/file sync, firmware update, high-rate or raw sampling |
| **Either** | USB CDC or BLE | control/commands, device info, decoded events, RSSI streams, scan tables, telemetry |

Rule of thumb from that repo: *if it would take more than a few seconds at
~10 KB/s, it is USB-only.* A USB-only feature **refuses a BLE session outright**
with a presentable error rather than degrading silently. Sarina should surface
that refusal as-is and, where it can, learn a feature's tier *before* invoking
it rather than discovering it by failure.

Connection shape, for any headless setup: the Flipper is a BLE **peripheral**
running a serial service; a Linux client connects over BlueZ and speaks the same
protobuf RPC used over USB. Pairing uses `GapPairingPinCodeShow` with bonding —
**a PIN appears on the device screen and must be confirmed by a human**, which
rules out fully unattended pairing.

### Firmware version reporting

As of 2026-09-12, `scripts/version.py` composes `firmware_version` as
`<upstream>-CHY-<ours>[+<ahead>]` — e.g. `1.4.3-CHY-0.1.0`,
`1.4.3-CHY-0.1.0+8` on a development build (`+N` commits since the last
`chy-*` tag), or `1.4.3-CHY-dev` when no `chy-*` tag exists yet. The commit
hash is no longer in this string — use the separate, exact `firmware_commit`
field for that. `chymaera-device-info` parses this format and notes the
tracked upstream release and dev-build status **without failing the check**; a
device still running the pre-2026-09-12 format (`git describe --tags --always`,
e.g. `chy-0.1.0-2-g540d03c7`) is recognised and noted as legacy rather than
misparsed. `unknown` is still a warning — it means git failed outright.

`1.4.3` (`GitVersion.UPSTREAM_VERSION`) is a **hand-maintained constant** in
the firmware repo, not derived from git — release tags aren't ancestors of
`dev`. It can go stale after an upstream merge; the firmware repo flagged this
as a natural thing for `chymaera-merge-report` to notice, not yet implemented.

### Hard boundaries Sarina will not cross

- No flashing, no `fbt flash*`, no `update.fuf` option-byte edits.
- No edits to `assets/protobuf/` — that is the firmware↔app wire contract.
- No unattended firmware-core patches. Sarina may *recommend* an external app
  over a core patch; it does not apply core patches itself.

---

## 3. Sarina ↔ Chymaera_Zero_App — **live (client implemented)**

**The App owns this contract.** Its spec is
[`docs/SARINA_API.md`](https://github.com/Artifex-Technologies/Argus_App/blob/main/docs/SARINA_API.md),
implemented in `backend/chymaera/sarina/sarinaserver.cpp`. Sarina implements the
client half in `sarina/chymaera/console.py` and does not extend the protocol.

| Aspect | Value |
| --- | --- |
| Transport | Newline-delimited JSON over TCP, loopback only; non-loopback peers dropped |
| Port | **44700** |
| Default state | **Off.** Started from the Chymaera Console UI, `Chymaera.startServer(port, token)`, or headlessly (below) |
| Auth | Optional `"token"` field on every request; mismatch → `{"ok":false,"error":"unauthorized"}` |
| Request | `{"cmd": "status", "id": 7, "token": "optional"}` |
| Response | `{"ok": true, "id": 7, ...}`; failures carry `"error"` |

Commands Sarina implements, exactly those the App documents: `ping`, `help`,
`status`, `log.tail`, `log.add`, `session.start`, `session.end`, `capture.list`,
`capture.import`.

```bash
sarina chymaera-console [--host H] [--port 44700] [--token T] [--captures N]
```

The token may come from `CHYMAERA_CONSOLE_TOKEN` instead of the command line.

**Starting the server without a GUI.** The App ships a headless CLI, which is
how this works on a server, over SSH, or under WSL where Qt Quick may not
render — and therefore the path that matters most on Chymaera Linux:

```bash
qFlipper-cli chymaera serve 44700 <token>   # starts the API, streams the event log
qFlipper-cli chymaera selftest              # end-to-end check, includes an API ping
```

`selftest` is the fastest way to tell whether a Sarina-side failure is Sarina's
fault or the app's.

### Capture analysis — Sarina reading, not extending

`sarina chymaera-analyze` (`sarina/chymaera/analysis.py`) pulls records via
`capture.list()` and works only with the `meta` JSON field the App already
attaches to each one — it does not parse `.nfc`/`.sub` files itself, and never
will; that parsing is the App's job (§4's boundary). Statistical analysis
(duplicate NFC UIDs, rare Sub-GHz frequencies) needs only the fields the App
already documents in "Capture record shape." An optional `--model-dir` adds
semantic grouping via `sarina.ml` (a local embedding model, see the README) —
this is Sarina-side only and adds nothing to the App's contract.

**Rules Sarina follows here:**

- Sarina reads the datastore **only through this API**, never by opening the
  SQLite file at `status.datastorePath` directly — schema changes stay behind
  the contract and nothing contends for the file lock.
- New commands get proposed on the App's board **before** any client code.
- Destructive device operations are deliberately not exposed, and Sarina does
  not work around that.

> **Superseded:** an earlier version of this file proposed a different shape for
> this API (HTTP, port 8900, bearer header). The App had already built and
> documented the protocol above. Their spec is authoritative; the proposal is
> withdrawn and this section describes what is actually implemented.

**One point still worth the App instance's judgement:** if commands that
transmit, flash, or mutate device state are ever added, they should require an
approval the operator granted in the app's own UI. An assistant should not be
able to make a Flipper transmit because a model decided it was a good idea.
Read paths need no such gate. Recorded as a request, not a demand — that repo
owns the call.

---

## 4. Sarina ↔ Chymaera_Sentry — two contracts, opposite directions

### 4a. Sentry reads Sarina's session files — **proposed**

Sentry is a separate Go process with no import dependency on Sarina. It
reads, on a poll loop, the JSON files Sarina's `sarina/session_store.py`
already writes to a workspace's `.port_sessions/` directory: `session_id`,
a flat `messages` list, and aggregate `input_tokens`/`output_tokens`.

**This is Sentry's proposal, not yet Sarina's commitment.** The session-file
shape above is Sarina's internal persistence format, not a published
contract, and Sarina has not agreed to freeze it for an external reader.
Until this section's status changes to **agreed**, a future change to
`session_store.py`'s output shape is not a breaking change on Sarina's
side — Sentry absorbing it is Sentry's problem. Sarina should simply keep
doing whatever it already does here; this section exists so a future
Sarina change is made with the awareness that something outside the repo
now reads this format, not to freeze it prematurely.

Sentry never writes into `.port_sessions/` and never opens a Sarina
process's sockets or files other than this one directory.

### 4b. Sarina consults Sentry's governance API — **live, opt-in**

The reverse direction, added 2026-09-26: `sarina/sentry_governance.py`
(stdlib `urllib.request` only) calls Sentry's `POST /v1/review` over
loopback HTTP, opt-in via the `SENTRY_REVIEW_URL` environment variable.
`sarina/memory_security.py`'s `scan_memory_text` — the single function
both memory-write call sites already funnel through — consults it *after*
its own in-process checks pass, and folds an actual `"deny"` response into
the same `MemoryScanResult(blocked=True, reason=...)` shape those checks
already produce. No call site outside `memory_security.py` changed.

**Fails open on Sentry's absence, fails closed on an actual verdict.**
Unset `SENTRY_REVIEW_URL`, or an unreachable/erroring Sentry, and
`consult_sentry` returns `None` — `scan_memory_text` falls back to its own
in-process result exactly as before this integration existed. Sentry is
optional infrastructure, not a hard dependency; but a real `"deny"`
response blocks the write, same as an in-process match would.

**Why this isn't pointless redundancy** even though Sentry currently runs
the identical three pattern lists Sarina's own in-process check does: the
point is that Sentry's copy runs in a separate OS process this repo cannot
reach into. A compromised or sufficiently manipulated agent session could
in principle disable or bypass its own in-process `memory_security.py`
checks from within its own process; it cannot touch Sentry's.

Marked **live** rather than proposed because both halves are built,
tested, and verified working together over a real loopback connection —
not a claim that the shape will never move. A breaking change to the
`/v1/review` request/response JSON needs updating this section,
`sarina/sentry_governance.py`'s docstring, and Sentry's own
`internal/governance`/`internal/api/review.go`, together, in one commit.

Full detail in both repos' 2026-09-26 Coordination Log entries — see
[`Chymaera_Sentry/CLAUDE.md`](https://github.com/Artifex-Technologies/Heimdall/blob/main/CLAUDE.md)
§5 for Sentry's side.

### 4c. Sentry asks Sarina for an advisory — **live, opt-in**

Added 2026-10-04 at the owner's request. When Sentry quarantines a Limbo guest
(section 8) it may ask Sarina for an opinion for the human who must decide. It
uses Sarina's service API (`sarina-service`, `POST /v1/sessions` then
`/chat`) with `tool_allowlist: []`, meaning the session has **no tools at all**;
bearer token if configured, loopback only. The evidence is sent framed as
untrusted data (it comes from a guest that may be hostile).

**Why an allowlist and not just `allow_shell=false`:** `AgentPermissions` only
*deny at call time*; the model is still offered every tool. Sentry's first
version relied on those flags, and the chain test showed the advisory model was
offered ~70 tools (`web_fetch`, `memory_add`, `config_set`, `mcp_call_tool` ...),
most of which those flags do not gate. `tool_allowlist` (additive field on
`POST /v1/sessions`) removes tools from the session entirely; the response's
`tools` field reports how many it really has, and Sentry refuses to send evidence
unless that is 0, since an older service silently ignores unknown fields.

**Sarina's answer is an advisory and nothing else.** Sentry stores it on the
quarantine record, where Limbo bounds and cleans it and the human sees it
labelled as untrusted. It cannot change a record's status or release a guest;
only a human verdict can. Assume the advisory can be wrong or manipulated; the
design makes that survivable rather than relying on Sarina resisting it.

This is Sentry initiating contact with Sarina, which Sentry's contract had not
allowed before; recorded in both repos. It needed one additive Sarina change,
the `tool_allowlist` field and `tools` count described above.

---

## 5. Firmware ↔ App

Not Sarina's contract, recorded so the picture is complete: the interface is
`assets/protobuf/` in the firmware repo, and new capability goes through
`DataExchangeRequest` in `application.proto` rather than new top-level RPC
messages. RPC is transport-agnostic — the same protobuf session runs over USB
CDC or the BLE serial service. Sarina treats that boundary as read-only.

---

## 6. Shared conventions

### Port registry

| Port | Service | Repo | Status |
| --- | --- | --- | --- |
| 8765 | Sarina web GUI | Sarina | live |
| 8899 | Sarina integration service | Sarina | live |
| 44700 | Sarina control API | Chymaera_Zero_App | live |
| 8930 | Sentry status API (loopback, read-only) | Chymaera_Sentry | live |

Claim a port by adding it here in the same commit that binds it.

### Rules that apply in every repo

- **Loopback only.** No ecosystem service binds a routable interface by default.
- **Never commit secrets.** No tokens, keys, capture data, or client engagement
  data. Real capture output belongs in gitignored paths.
- **Hardware writes need a human.** Flashing, transmitting, and option-byte
  changes stay behind explicit operator approval, in every repo.
- **Stock compatibility is a hard requirement.** The device must stay usable
  from stock qFlipper; the wire format is not ours to fork.

### Device access on Linux

USB: VID `0483` / PID `5740`, udev rules from the App repo
(`installer-assets/udev/42-flipperzero.rules`), user in the right group. Under
WSL, USB passthrough needs `usbipd-win`. BLE: see the pairing note in §2.

---

## 7. Change protocol

1. Changing anything another repo depends on means updating **this file** and the
   affected repo's `CLAUDE.md`, in the same commit.
2. Use `CONTRACT:` in the commit message for anything on a contract list, so the
   other side can grep for it.
3. A contract marked **proposed** is not a licence to build against it.
4. **Check the owning repo before assuming a contract still holds.** Twice now a
   spec here went stale within a day — the App shipped its API before Sarina's
   proposal arrived, and the distro dropped BlackArch category groups. Read the
   sibling's current board, do not trust this file's summary of it.

---

## 8. Sarina ↔ Limbo — **live (client implemented)**

Limbo owns this contract (`Limbo/docs/QUARANTINE.md`); Sarina conforms.

Sarina uses Limbo to **validate an update or install in a disposable guest
before the real system is touched**, and to troubleshoot. `sarina/chymaera/limbo.py`
(stdlib only) talks to Limbo's **agent socket** (`SARINA_LIMBO_SOCKET`, normally
`/run/limbo/agent.sock`, group `limbo-agent`). Surfaces:

- CLI: `sarina limbo-profiles`, `sarina limbo-validate PROFILE COMMAND...`
- `limbo_profiles` also shows what each profile allows (network mode, whether it
  grants `sudo`, the command time cap), read-only: a model given names alone ran
  `pacman` without sudo on a profile that granted it and then blamed the profile.
- Model-callable tools: `limbo_profiles` (read-only) and `limbo_validate`
  (gated by the same `allow_shell_commands` permission as bash, since it runs
  commands, albeit in a VM). Inert unless `SARINA_LIMBO_SOCKET` is set.

What the boundary is, and who enforces it:

| | |
| --- | --- |
| Sarina names a **profile**, never a manifest | Profiles are written by the human into root-only `/etc/limbo/validation/`; a non-disposable profile is refused. Sarina cannot choose the image, network, shares or devices. (Enforced by Limbo.) |
| Each run is a fresh guest, torn down always | Time, output size and concurrency are capped by Limbo regardless of what Sarina asks. |
| No access to quarantine | The agent socket has no quarantine routes; Sarina cannot see, open, raise or decide one. (Enforced by Limbo's per-socket route tables; tested.) |
| Client refuses any socket not named `agent.sock` | A misconfigured path fails loudly rather than reaching a more privileged socket. |

**A passing run proves the command behaved in the sandbox, nothing more.** It
never applies anything to the real system; a human approves that. The tool
description says so to the model, and a result is returned labelled as such.

**Output is untrusted.** stdout/stderr come from inside a guest that may be
running something hostile: stripped of control characters, truncated, and
rendered under a banner saying it is evidence, not instructions. A failing
command is a *result* (that is what validation is for), not an exception.

Verified against a fake agent socket in `tests/test_chymaera_limbo.py` and, from
Limbo's side, with a real daemon, real sockets and real unprivileged users. Not
verified: Sarina's model actually choosing to call these tools well, since no
model backend runs here.
