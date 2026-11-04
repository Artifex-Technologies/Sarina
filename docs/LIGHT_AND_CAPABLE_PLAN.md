# Light and capable: the model test, hardware tiers, and what to take from Odysseus (plan, 2026-11-04)

Matt's goals, from the conversation of 2026-11-04:

- Sarina should be as capable as possible, and also lighter and faster, so that other people can run her locally too.
- More tools are welcome, but **she needs more training before she gets more tools**.
- He is ready to try a bigger model (the MoE candidate below) on today's PC. The upgrade (Ultra 7 270K Plus, RTX 5090) is months
  to a year away.

## Principles

1. **The model thinks; the code does the work.** A tool that computes exactly (crypto, stego, Trachtenberg, crash records) works
   as well under a small model as under a big one, and costs almost nothing. Capability grows mostly through tools and checks.
2. **Optional parts are off unless installed.** Voice, Limbo, Heimdall integration, the vault, the council: Sarina without them
   is one Go binary and a model.
3. **Measured, not guessed.** Every model, setting or feature change runs the eval suite before and after (`evals/run_evals.py`).
4. **Training before tools.** Homework, lessons and (later) fine-tuning come first; new tools follow.

## 1. The model test (now)

**Candidate:** Qwen3.6-35B-A3B (Apache-2.0).
- 35B parameters in all, about 3B used per token: a mixture of experts.
- File: unsloth's `Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf` (22.4 GB), in `~/.local/share/sarina-models/`.

**How it runs on the 2080 Ti (11 GB):**
- Server: Ollama's own `llama-server` (`/usr/lib/ollama/llama-server`; it finds CUDA when started from
  `/usr/lib/ollama/cuda_v13`), so nothing new is installed.
- `--n-cpu-moe N` keeps the experts of N layers in system RAM; the attention layers and the shared expert stay on the card.
- N is tuned so the card keeps room for the voice (about 1 GB) at Sarina's 16k context.
- Sarina reaches it through its OpenAI-compatible API (`OPENAI_BASE_URL`, `OPENAI_MODEL`), set for the test runs only.

**Protocol (day sessions only, card idle, never during a night run):**
1. Tune N. Measure tokens per second (prompt and generation) and memory on the card.
2. Run the full eval suite on the candidate. Compare with the same build on qwen3:8b: pass set, mean seconds, send-backs.
3. Measure spoken-turn timing (`LABYRINTH_VOICE_TIMING=1`, the stage medians of night 2).
4. Blind comparison for what the evals cannot score (tone, personality, how natural she sounds). Matt reads answer pairs to
   the same questions with the model names hidden, picks the better one, and the votes are logged.

**Rollback:** nothing changes until Matt decides. `sarina.env` and Ollama stay as they are; the test sets its variables per run.

**Possible outcome: split the jobs.** Keep qwen3:8b for spoken questions and quick talk, where speed matters. Give hard work
(code, reverse engineering, analysis) to the candidate through the compute tiers (`go/tiers`). Switching models costs a few
seconds of loading on the card.

## 2. Hardware tiers: `sarina fit` (now)

From Odysseus's hardware-aware model picker; the idea only, written in Go.

**What it does:** reads the GPU (nvidia-smi, AMD sysfs) and RAM (/proc/meminfo), then checks a short shipped list of models
that passed her eval suite. For each model it works out whether it fits at a 16k context, and if so how:
- fully on the card;
- with experts in RAM;
- or not at all.

It prints the Ollama tag or llama-server settings, and `sarina doctor` warns when the configured model will not fit.

**Rough tiers:**

| Hardware | Model |
| --- | --- |
| No GPU or a laptop | about 4B |
| 8-12 GB card | 8B, or the MoE with experts in RAM if there is plenty of RAM |
| 24 GB and up | the MoE fully on the card |

A feature that needs a stronger model says so, instead of failing quietly. The eval suite is also run on the small tier, so
other people are told honestly what a small Sarina can and cannot do.

## 3. From Odysseus (ideas only)

Odysseus (github.com/odysseus-dev/odysseus) is a self-hosted AI workspace. It is AGPL-3.0, so no code, prompts or text are
taken; only the behaviour, described in our own words. Surveyed 2026-11-04.

| Idea | Sarina today | Plan | When |
| --- | --- | --- | --- |
| Test whether a lesson helps | `sarina lessons`, the evals | `sarina lessons audit` at night: rerun the cases linked to each active lesson with and without its rule, graded by the eval checks (not a model judge). Write the result into the lesson's front matter; propose retiring rules that change nothing (Matt decides). Keeps the 20-rule cap useful. | now (training) |
| A stronger model's fix becomes a lesson | tiers escalation, `council_ask` | After repeated send-backs or an escalated turn, she offers "make a lesson". Only after the owner's yes does the sanitised trace (`tiers.Sanitize`) go to the stronger tier. The proposed rule comes back as untrusted text and is saved as a `status: proposed` lesson. Training data for the fine-tune stays her own approved answers, not the stronger model's text. | now (training) |
| Match models to the hardware | `tiers/resource.go`, `doctor` | `sarina fit` (section 2) | now |
| Blind comparison | `sarina council` names the models | `council --blind`, local members only, the vote logged (section 1, step 4) | with the model test |
| Web research loop (plan, search, read, extract, merge, with a source floor) | none (ECOSYSTEM §9: "not built") | `go/research`, off by default:<br>- the owner approves each round's queries;<br>- fetching refuses local and LAN addresses and is byte-capped;<br>- every excerpt is labelled untrusted;<br>- the report goes to the vault's `Sources/` with citations, and the claims check matches quotes against fetched pages. | later (a tool: after training) |
| Background jobs that give way to games | the night timer, `sysinfo/gaming.go` | Jobs as files in `~/.sarina/jobs`, run by a user timer that skips while a game runs or the card is busy; results in the Daily note | later (with research) |
| A record of each action, written before it runs | `answerPending`, session files | An append-only per-session `effects.jsonl` for writes, edits, shell and system_fix; after a crash, an unfinished entry reads "interrupted, may have changed things" | later |
| Tools chosen by meaning | per-prompt providers, `sarina route` | More optional tools behind route topics | later |

**Not worth having:**
- **Email:** heavy, a flood of injection-prone input, and it sends mail off the machine.
- **Calendar, notes and tasks app, document editor, image generation, themes:** the vault covers notes, and image generation
  competes for the one GPU.
- **Downloading and serving models with vLLM or SGLang:** heavy Python stacks; Ollama is enough.
- **The Docker/ChromaDB stack:** `vecindex` does this.
- **A small router model:** `sarina route` does this.
- **Multiple users and logins:** single owner; phone approval is planned in VOICE_AND_TRUST_PLAN.
- **Subscription logins to online assistants:** data leaves the machine, and their terms are a risk.
- **A YouTube handler:** Huginn covers it.

## Order of work

1. The model test (section 1), with the blind comparison.
2. `sarina fit` (section 2).
3. `sarina lessons audit`, then "make a lesson" from a stronger model's fix (section 3).
4. After more training: web research, then background jobs.
5. With the 5090: the candidate fully on the card, and the first fine-tune (QLoRA) on her approved homework.
