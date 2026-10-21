# Memory, routing and training: plan (2026-10-20)

Goal: fully capable, and light when the owner is gaming or working (nothing resident that is not asked for).

| Idea (source, licence) | What it gives Sarina | Where it lands | State |
|---|---|---|---|
| **vecindex**, a Go rewrite of TurboQuant (paper arXiv 2504.19874; reference library turbovec, MIT) | semantic search over her vault, the book vault and saved sessions at 2 or 4 bits per number; no training, add/remove any time; plain Go, no cgo, no background process | `go/vecindex` | built, tested: 7.8x smaller than float32 at 768 numbers, recall@10 0.83 (4-bit) on deliberately hard synthetic data, about 20 ms per search over 100k vectors on one thread |
| **Laya** (Apache-2.0): a small encoder that answers typed choices (which topic? yes/no? how urgent?) in one pass, no text generation | a "System 1" gate in front of the 8B model: decide system_status vs vault vs small talk vs refuse, without waking the big model | optional Python helper (ONNX, loaded on demand, freed after) | not started: measure first (RAM, latency against the current routing) |
| **Soup** (Apache-2.0): YAML-driven LoRA fine-tuning with layer streaming (8B on a 4 GB card) | train her on the owner's own sessions and persona (her voice model, or Laya's head) | offline, run by hand, never while a game runs | not started |
| **Prime Agent** (MIT, Prime Intellect; a Rust agent) | the *Continual Harness* idea: after a session, small evidence-backed lessons (a note for her prompt, a memory, a skill description) that can be reviewed and rolled back, never touching her base prompt; also background sessions you can reattach to, goals and schedules for long jobs | extends her `memory-review` (owner approves each proposal) with snapshots and rollback; background sessions later | ideas only, no code copied: its model runs generated Python with your permissions and says itself it is not a sandbox, which Sarina must not do |

Order: wire vecindex into vault_search/memory-search (needs an embedder: the helper's ONNX model, loaded only while indexing/searching); then try Laya as a router on her saved sessions; then Soup, once there are enough good examples (reviewed by the owner, nothing private leaves the machine).


Shipping: Sarina is optional. The image only carries her for this build; for everyone else she is a Marketplace component (`sarina` in components.json), installed by choice. Nothing above may become a dependency of the OS or of Yggdrasil.
