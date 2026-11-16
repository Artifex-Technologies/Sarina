# Rust migration: what was ported and what it measured

The detailed record of each port: [RUST_PORT.md](RUST_PORT.md) (the Go core, `sarina-rs`) and [VOICE_RUST_PORT.md](VOICE_RUST_PORT.md) (the spoken-question programs).

Sarina's spoken-question programs, Python in Labyrinth's image until now, are ported to one Rust binary,
[`sarina-voice`](../packaging/voice/README.md). Measured on my desktop, old against new on the same input (2026-11-11; a minus
is a gain):

| Program | Was | Measured | Details |
| --- | --- | --- | --- |
| `labyrinth-ask-sarina` (Super+S) | Python | a spoken question 52-80 ms → 4.2-7.1 ms (−91 to −92 %, 48-73 ms sooner), 19-30 MB → 5.7-6.3 MB | [sarina-voice](../packaging/voice/README.md#why-rust-measured) |
| `labyrinth-sarina-checkup` | Python | 54-57 ms → 2.1-3.6 ms (−94 to −96 %), 26 MB → 4-5 MB | [sarina-voice](../packaging/voice/README.md#why-rust-measured) |
| `labyrinth-sarina-speak-voiced` (per spoken sentence) | Python | 24-26 ms → 0.7-1.1 ms (−96 to −97 %), 13-14 MB → 3.5-3.8 MB | [sarina-voice](../packaging/voice/README.md#why-rust-measured) |
| `labyrinth-voiced` (the voice service, Chatterbox) | Python | on the processor: a sentence the same (−0.8 %; it is all ONNX Runtime), peak memory 2.48 → 2.21 GB (−11 %), waiting 13 → 4 MB; the card's numbers are still to come | [sarina-voiced](../packaging/voiced/README.md#why-rust-measured) |
| `sarina` (the Go core, being ported: `sarina-rs`) | Go | a chat turn 68 → 24 ms (−65 %), 23 → 11 MB (−54 %); `memory-status` 7.4 → 1.4 ms, 21.5 → 5.2 MB; the idle service 23 → 5.2 MB (−78 %), the busy one **+7-10 MB (worse)**; the console (`gui`) starts in 1.3 ms instead of 7.8, idles at 5.7 MB instead of 23.1 and holds 12 MB instead of 28 after 100 runs; `doctor` 65.5 → 10.0 ms (−85 %), 22.8 → 8.1 MB (−64 %); a chat turn with a `system_status` call 72.7 → 29.0 ms (−60 %), 23.8 → 11.9 MB, with a `trachtenberg` call 67.0 → 23.6 ms (−65 %); `sarina quantum` (6 qubits, 1,000 shots) 6.4 → 0.7 ms (−89 %), 19.4 → 13.7 MB (2026-11-16); 12 → 6.9 MB on disk (2026-11-13) | [RUST_PORT](RUST_PORT.md#why-rust-measured) |

Every case gave the same output in both. **Worse in Rust:** the size on disk (2.3 MB against 74 KB of scripts; Python stays
installed for the voice service either way).
