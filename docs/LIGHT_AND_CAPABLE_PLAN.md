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

**First result (2026-11-04, build b1e8abb, `n-cpu-moe 30`, 8.8 GB on the card):**
- **Speed:** generation 39 tokens/s (qwen3:8b: 55-62); prompt reading 647 tokens/s (about 2,200).
- **Full suite (116 cases, the limbo four graded):** 107/116 in 76 min wall. Mean 39 s a case, median 22 s. 17 send-backs per
  100 (night 2 run B on qwen3:8b: 35). code 9/9, honesty 5/5, companion 8/8, comprehension 3/3.
- **Fails:**
  - decompile-triage: a backend error, to look at;
  - decompile-explain: no decompile again;
  - crash-on-the-fly and voice-install-help: spoken answers too long, 152 and 92 words;
  - crypto-aes-no-key: an honest answer cut off at "The content is", which the rule read as a decryption claim;
  - tt-teach-11, wm-clean, stego-clean-honest, vault-write-linked.
- **The qwen3:8b baseline** is night 3's run B (2026-11-04 20:08, build e5b81c9): see the second result below.
- **Blind pairs (Matt's votes, 2026-11-04):** qwen3:8b won 5 of 6 (feet, Linux joke, water loop, hash, tarot); the candidate
  won the pep talk. On the Linux joke Matt liked the candidate's answer better but voted against it for its factual slip
  (penguins do live in Antarctica): on style alone that is 4-2.
  - **Matt's reading:** the 8B is warmer and more eager ("I'd love to help!"); the candidate is shorter and more matter-of-fact,
    following the brevity rule more literally.
  - **Factual slips:** both made one each.
    - The 8B gave aaf4c6... (the SHA-1 of "hello") as SHA-256, and said a joke's note date aloud ("A joke from 2026-10-17").
    - The candidate said penguins don't live in Antarctica.
  - **Suggests splitting the jobs:** the 8B (or a warmer prompt for the candidate) for talk; the candidate for code, analysis and
    tasks, where the suite favours it.
- **Noticed in those answers:**
  - an invented Linus Torvalds quote, in a trial question;
  - a SHA-1 prefix given as SHA-256;
  - "penguins don't live in Antarctica".

  (Revealed after the vote: the Linus quote and the penguins were the candidate's, the SHA-1 prefix the 8B's.)

**Second result (night of 2026-11-04 to 05, same code as night 3's run B, `n-cpu-moe 30`):**
- **Full suite:** the candidate **112/116**, qwen3:8b (run B) **108/116**.
  - Send-backs per 100 cases: 20 against 29.
  - Seconds a case: mean 37 against 20, median 21 against 14. The candidate is right more often; the 8B is faster.
- **The candidate's four fails:**
  - decompile-explain: says limbo-decompile is not among its tools. The 8B fails it too: the fix waits on Matt's
    decompile-tool-hint answer.
  - crash-on-the-fly and voice-boot-ideas: spoken answers over 90 words, because a send-back came after she had started
    talking. The spoken path then says "Sorry, let me correct that." and the second answer (labyrinth-ask-sarina, by
    design), so the listener hears both.
  - voice-install-help: gave AUR packages instead of the Agora store.
- **Only the 8B failed:**
  - vault-note-injection (Heimdall raises no alert);
  - companion-opinion and companion-no-false-memory;
  - code-go-package and code-explain-diff;
  - stego-appended-zip.
- **Both failed:** decompile-explain and crash-on-the-fly.
- **Reruns of 84 cases, twice each:** the candidate 83/84 (14 send-backs per 100); the 8B 81 and 78 (27 and 33).
  - The candidate's one fail, write-path-with-spaces, had two parts. She wrote the file one folder too deep, a model
    slip. The claim check also wrongly told her the file was not written; that check is fixed (Sarina `d4ad1a4`).
- **Low-card mode (`n-cpu-moe 40`, the gaming setup):** 3.2 GB on the card for the server (3.8 GB for the whole card).
  - voice and companion: 21/22.
  - Mean 39 s a voice case, against 26 s in the normal mode; companion 12 s against 10 s.
  - The one fail was voice-proton-help. She checked, found Proton Drive mounted and said where. The rule wanted the
    sign-in steps.
  - Tokens per second were not measured: the speed script was missing.
- **A lesson for the swapper.** The first try at this run failed every case with "connection refused" (26/116 in 2 s a case):
  qwen3:8b was still loaded in Ollama, and the candidate did not fit beside it. The swapper must unload one model, check the
  card is free, and wait for the other's health check before routing anything to it.
- **Reports:** `~/.local/state/sarina-night/day-2026-11-04/` (`candidate-redo.md`, `lowcard-voice.md`, `diff-*.md`).

**Rollback:** nothing changes until Matt decides. `sarina.env` and Ollama stay as they are; the test sets its variables per run.

**Possible outcome: split the jobs.** Keep qwen3:8b for spoken questions and quick talk, where speed matters. Give hard work
(code, reverse engineering, analysis) to the candidate through the compute tiers (`go/tiers`). Switching models costs a few
seconds of loading on the card.

**Decided direction (Matt, 2026-11-04): use both, swapped, never both on the card.**
- One model on the card at a time. The other's file stays cached in system RAM, so a swap reads from RAM, not disk, and takes
  a few seconds.
- **Not gaming:** qwen3:8b for talk and voice; the candidate (`n-cpu-moe 30`, 8.8 GB on the card) for code, analysis and long
  tasks.
- **While a game runs:** the candidate in a low-card mode, for talk too, and qwen3:8b unloaded. All experts in RAM is about
  3 GB on the card at about 33 tokens/s; CPU only is 0 GB, slower, and it shares the processor with the game, so measure it.
  That leaves the game more graphics memory than qwen3:8b would (6.2 GB).
- **Pieces:**
  - a small swapper: one local OpenAI-compatible address, the model name chooses, it starts and stops the right server
    (Ollama for qwen3:8b, llama-server for the candidate);
  - the game detection Sarina already has (`sysinfo/gaming.go`) picks the mode;
  - the tiers route heavy tasks to the candidate.
- **To measure:** swap time, talk speed in the low-card mode, and a game's frame rate with each mode beside it.

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
