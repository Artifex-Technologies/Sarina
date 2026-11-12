# sarina-voiced

Sarina's voice service in Rust: the port of Labyrinth OS's `labyrinth-voiced` (Python, 272 lines) and its Chatterbox Turbo
engine `chatterbox_engine.py` (140 lines). Same socket, same requests and answers, same files, same environment variables,
and for the same seed the same speech, bit for bit (measured below).

**Status (2026-11-11): work in progress.** See "Resume here" at the end.

```bash
cd packaging/voiced
cargo build --release                       # target/release/sarina-voiced
cargo build --release --features parity     # also the side-by-side driver (never installed)
CUDA_VISIBLE_DEVICES= nice -n 19 ~/.local/share/labyrinth/tts/cb-venv/bin/python -I packaging/voiced/tests/parity.py   # from the repository root
```

## Resume here

- Done: text side, NumPy's sampling, the engine, the service with the pipeline, the parity driver and `tests/parity.py`.
- Next: engine and request parity on the processor, the measurements, the switch branch in Labyrinth_OS.
