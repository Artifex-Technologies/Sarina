# Python helpers (python/sarina_helpers, go/helpers)

The ML and voice features are the owner's original Python (onnxruntime, faster-whisper, Piper) and stay Python,
as a separate package with no code derived from the old core. Go reaches them through a documented protocol:
`python/sarina_helpers/PROTOCOL.md`.

## Python side (`python/sarina_helpers`, own `pyproject.toml`, extras `ml`, `voice`, `test`)

- `embeddings.py`, `audio_io.py`, `stt.py`, `tts.py`: moved from the original `sarina/ml` and `sarina/voice` (only
  docstrings/imports touched). Importing the package imports no third-party module; constructing models without the
  extra raises `MLUnavailable` / `VoiceUnavailable`.
- `embed_group.py` (capture grouping), `classify.py` (prototypes + fingerprint), `serve.py` (JSON lines),
  `memory_search.py` (reads Go session format, strips `<system-reminder>`), `voice_bridge.py`, `__main__.py`.
- voice-chat drives Go per turn with `sarina chat -- <text>` then `sarina resume <id> <text>`; session id is found
  from the session file that appeared. Memory review runs once at the end via `sarina memory-review`.
- Tests: `python -m pytest python/` (57, offline, fakes for models/audio/Go binary). The four original test files
  (embeddings, audio_io, stt, tts) were ported by renaming imports; loop/session-search tests were rewritten.

## Go side

- `go/helpers`: `Python()` / `Command()` (env `SARINA_HELPERS_PYTHON`, `SARINA_HELPERS_PATH`), `Available()`,
  `Passthrough()`, `StartServer()` -> `Server{Classify, Group, Close}`. Commands `memory-search` and `voice-chat`
  are thin pass-throughs registered via `cli.Register`; wired by one import line in `cmd/sarina/main.go`.
  Without the helper they print `sarina: the Python helper is not importable: ...` plus an install hint, exit 1.
- `desktop.HelperScorer` now starts the helper; it refuses a helper whose prototype fingerprint differs from
  `PrototypesFingerprint()`. Unavailable -> the existing "semantic inference unavailable: ... Python helper" message
  and deterministic-only audit. The scorer is closed at the end of the run.
- `chymaera-analyze --model-dir` calls `group`; groups are rendered as `## Semantic groups (similar captures)`
  (`CaptureAnalysis.SemanticGroups`). On failure the old "Semantic grouping unavailable: ..." line, now with the cause.
  `--similarity-threshold` is now honoured (it was accepted and ignored).
- Tests: `go/helpers/fakehelper` lets a test binary act as the helper (set `SARINA_HELPERS_PYTHON` to
  `os.Executable()` and `SARINA_FAKE_HELPER=1`; call `fakehelper.MaybeRun()` from `TestMain`). No Python needed.

## Not done / caveats

- Not run against a real ONNX model or real audio hardware here; the Python tests use fakes, as the originals did.
- `voice-chat` does not mirror the agent's backend/permission flags; they go through `--agent-arg=...`.
- The prototype list exists twice (Python `classify.py`, Go `desktop/semantic.go`); the fingerprint check catches drift.
- `docs/ECOSYSTEM.md` and the repo-level pyproject/PKGBUILD packaging of the helper were not touched (the Arch
  package for `sarina-helpers` is a Chymaera_Linux follow-up).
