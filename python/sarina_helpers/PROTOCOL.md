# sarina_helpers protocol

The Go `sarina` binary calls this package as `python -m sarina_helpers <mode> ...`. Nothing is imported
across the language boundary. Interpreter: `SARINA_HELPERS_PYTHON`, else `python3`, else `python`.
If the package is not installed, set `SARINA_HELPERS_PATH` to the directory containing `sarina_helpers/`
(it is prepended to `PYTHONPATH`). Running the module with no arguments prints
`usage: python -m sarina_helpers ...` and exits 2; Go uses that to tell "importable" from "not".

Install: `pip install python/sarina_helpers[ml]` (embeddings) and/or `[voice]` (faster-whisper, Piper;
`arecord`/`aplay` from alsa-utils are external programs). Importing the package never needs either extra.
No model is ever downloaded; model paths are operator-provided local files.

## Modes

| Mode | Used by | Style |
| --- | --- | --- |
| `serve --model-dir D` | `chymaera-analyze --model-dir`, `desktop-audit --model-dir` | JSON lines on stdin/stdout, long-lived |
| `memory-search --model-dir D [--session-dir D] [--cwd D] [--top-k N] <query...>` | Go `memory-search` | one-shot, plain text, stdio passed through |
| `voice-chat ...` | Go `voice-chat` | interactive, stdio passed through |

## `serve` (JSON lines)

One JSON object per line in each direction, UTF-8, strictly request/response in order. EOF on stdin ends the
process (exit 0). Each response echoes the request's `id` if it had one. Every response has `"ok"`; when false,
`"error"` is a human-readable string. A bad request line never kills the server.

If the model cannot be loaded, the server writes one line `{"ok":false,"error":"..."}` and exits 1 before
reading anything (Go sees that line, or EOF plus the last stderr line, as the "unavailable" reason).

| Request | Response |
| --- | --- |
| `{"op":"ping"}` | `{"ok":true,"protocol":1,"prototypes":"<sha256>"}` |
| `{"op":"embed","texts":["..."]}` | `{"ok":true,"vectors":[[...]]}` (L2-normalised, one per text) |
| `{"op":"classify","text":"..."}` | `{"ok":true,"label":"Network;Security"\|null,"score":0.62}` |
| `{"op":"group","captures":[{...}],"threshold":0.75}` | `{"ok":true,"groups":[["a.nfc","b.nfc"]]}` |

- `protocol`: the Go side requires exactly 1.
- `prototypes`: sha256 of the desktop category prototype set serialised as
  `"<cat1>;<cat2>|<sentence>"` lines joined by `\n`. Go compares it to its own copy and refuses a helper whose
  set differs, so cached scores can never silently change meaning.
- `classify` returns only the threshold-independent half: the nearest prototype's label (`;`-joined categories,
  `null` for blank text) and cosine score. Go applies `--similarity-threshold` and its own content-hash cache.
- `group` takes `capture.list` records (`name`, `type`, `meta` JSON string). Text embedded per capture:
  `name type deviceType=.. protocol=.. preset=.. uid=.. frequency=..`. Greedy: each ungrouped capture anchors
  a group of later captures with cosine >= `threshold`; only groups of two or more are returned, first-seen order.

## `memory-search`

Reads Go session files `<session-dir>/<id>.json` (default `<cwd>/.port_sessions/agent`), where `messages` is an
array of objects with string `role`/`content`. Excerpt = first user message left non-empty after stripping
`<system-reminder>...</system-reminder>` blocks (task) + last non-empty assistant message (outcome), each capped at
800 characters. Output, per result: `<score 3dp>  <session_id>`, `  task:    <text>`, `  outcome: <text>`.
No sessions: `No saved sessions found.` (exit 0). Missing extra or model: the error text, exit 1.

## `voice-chat`

Flags as the old command (`--stt-model-dir`, `--tts-model` required; `--tts-config`, `--stt-device`,
`--stt-compute-type`, `--input-device`, `--output-device`, `--max-record-seconds`, `--resume-session-id`,
`--no-auto-memory-review`) plus `--cwd`, `--session-dir`, `--sarina-bin` and repeatable `--agent-arg=<flag>`.
The Go wrapper adds `--sarina-bin <its own path>` unless given. Backend/permission flags are not mirrored:
pass them as `--agent-arg=--allow-write`, `--agent-arg=--model=...` and they reach every turn.

The bridge owns audio only (push-to-talk = Enter to start, Enter to stop; arecord/aplay; faster-whisper;
Piper, sentence-split). Per turn it runs, capturing stdout as the reply (printed, then spoken):

- first turn: `sarina chat --cwd C --session-dir S --no-auto-memory-review [--agent-arg ...] -- <text>`
- later turns: `sarina resume --cwd C --session-dir S --no-auto-memory-review [...] <session_id> <text>`

`chat` does not print its session id, so the bridge takes it from the session file in `S` that appeared or was
modified during the first call. A non-zero exit with no stdout is shown as `(agent turn failed, exit N: <last
stderr line>)` and not spoken. At the end (`/exit`, `/quit`, a spoken-prompt quit word, EOF) it prints
`chat_ended=user_exit|eof`, then unless `--no-auto-memory-review` runs `sarina memory-review --cwd C
--session-dir S <id>` once. Ctrl-C at the prompt: `chat_ended=interrupt`, exit 130. Missing ALSA tools,
faster-whisper, piper-tts or model files: one message naming everything missing, exit 1, before any turn.

Trade-offs of this design: one process start per turn (the model-side session is on disk, so nothing is lost), and
streaming output is not spoken sentence-by-sentence. Spoken text starting with `/` reaches the agent's slash-command
handling, as typed text would.
