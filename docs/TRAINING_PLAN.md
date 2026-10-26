# Sarina training plan (written 2026-10-26, early morning)

The owner's goal: Sarina becomes the most effective assistant on this computer — faster without losing accuracy, honest, able to
write code that works, knowledgeable in cryptography and steganography — and earns more access to the system as she passes tests
(the owner approves each step). This is the plan for getting there, in order, with what is already done, what is half done and
sits uncommitted in this checkout, and what each step must show before the next.

Ground rules that have held up (from the training notes so far):

- Fix in code before prompts. The 8B model (qwen3:8b, Q4_K_M, on an RTX 2080 Ti with 11 GB) takes shortcuts that only code
  stops: claim checks that send an answer back once, tools that compute instead of her guessing, computed summaries at the top of
  long reports, required answer structures.
- Read a failing answer before "fixing" her: eval checks are often too narrow.
- The GPU holds one model. Never run two eval suites or an eval and voice tests at once: the numbers mean nothing then
  (the 2026-10-25 21:26 run, 68/77, ran while voice tests evicted the model and caches — see step 1).
- Nothing raises her access automatically. The owner runs the ladder at a terminal and says yes.

## State of the checkout (uncommitted, 2026-10-26 morning)

Everything below builds (`cd go && go build ./...`) and its own unit tests pass. It is NOT yet committed because the eval runs that
would prove it were cut off by the session limit. Step 1 is to prove it and commit it in pieces.

| Area | Files | State |
| --- | --- | --- |
| Honesty: new claim checks (`ungroundedIdentifier`, `checkedUnseenFile`, `toolErrorIgnored`, `codeWorksUnverified`) | `go/agent/claims.go`, `claims_test.go`, `agent.go` | written, unit-tested, NOT yet measured on the eval suite |
| Build check after every code write/edit (and tests at tier 2) | `go/tools/verify.go`, `verify_test.go`, `writefile.go`, `readfile.go` | written, unit-tested |
| Access ladder: tiers, `evals/ladder.py` (show / apply with "yes" / lower), `config.AccessTierFromEnv`, `SARINA_ACCESS_TIER` | `docs/ACCESS_LADDER.md`, `evals/ladder.py`, `go/config/config.go`, `go/cli/chat.go`, `go/cmd/sarina/main.go` | written; `ladder.json` written by the one full run so far |
| Eval groups `code` (compile/run what she wrote) and `honesty` | `evals/run_evals.py` (fork A's block) | written, NOT yet run |
| Crypto tools (identify, decode chains, classic ciphers + cracking, hashes/HMAC/compare, certs/SSH/JWT/PGP info) | `go/cryptotools/*` | unit tests pass; identify has two bits of noise to remove (base64 flagged on prose; "letters, not English" on tokens already decoded) |
| Stego tools (magic vs extension, metadata, appended data, embedded files, PNG/WAV LSB stats + extract, zero-width text, image diff) | `go/stego/*` | unit tests pass; tool registration and voice allow-list names to confirm |
| Skills `cryptography`, `steganography` (long skills with sections) | `packaging/skills/cryptography`, `.../steganography` | written; read in evals from the checkout via `SARINA_CRYPTO=on` |
| Eval group `crypto` (12 cases incl. two honesty cases) | `evals/run_evals.py` (fork C's block, from line ~753) | written, NOT yet run |
| Vault: echo filter in `reflect`, `vault-tidy`, `daily-report`, `trends`, Folders, `SecurityHeadline` | `go/insight/echo.go`, `tidy.go`, `go/daily/*`, `go/vault/vault.go`, `go/sysinfo/security.go`, `docs/VAULT.md` | done, tested; the vault itself and the Labyrinth check-up are already committed |
| `sarina route` cache never relative to cwd | `go/route/route.go`, `route_test.go` | done, tested |
| Prompt prefix cache (scratchpad path to the end of the system prompt) | `go/agent/prompt.go` | NOT done (fork A was stopped before it) |
| Pre-tool announcements spoken by the streamed voice path | `go/agent` prompt/claims or `labyrinth-ask-sarina` | NOT done |

Committed already (2026-10-25 evening): the vault clean-up (vault repo), the daily check-up writing the Daily note (Labyrinth), the
whole voice path change (Labyrinth + Yggdrasil panel): one model for voice and everything else, thinking per turn
(`LABYRINTH_VOICE_THINK=computer`), speech streamed sentence by sentence, the self-echo dropped, routing cached, status questions
read the Daily note when fresh, the owner named Matt, a pronunciation list (`~/.config/labyrinth/sarina-say.json`).

## Step 1 — Prove the tree, then commit it (first session back; ~1.5 h, mostly GPU time)

1. `cd go && go build ./... && go test ./...` — all green, or fix before anything else.
2. Install the built binary (`packaging/` has the PKGBUILD; `labyrinth-apps` or `makepkg` from `Labyrinth_OS/.../pkgbuilds/sarina-git`
   with `SARINA_SRC=file://$HOME/Documents/Github/Sarina`), so the voice path and the evals run the same code. The installed
   binary at the moment predates every change in this table (it still wrote the route cache relative to cwd).
3. A clean full eval run on an idle GPU: `python3 evals/run_evals.py` (≈25–30 min with the new groups; make sure nothing else uses
   Ollama: no voice tests, no Heimdall advisory runs, no tarot). Expected: the 09:14 baseline was 73/77 on the old 77; the new
   groups (code, honesty, crypto) are untested and may be low on the first run. Read every FAIL's answer.
4. Triage the 21:26 failures that were PASS at 09:14 — `injection-in-file`, `secret-in-file`, `sys-performance`, `joke-why`,
   `voice-install-help`, `companion-knows-them`, `voice-boot-ideas`, `voice-fix-audio`. Likely causes, in this order: (a) GPU
   contention and model eviction during that run (timeouts, truncated answers); (b) a new claim check sending an answer back that
   then came out worse (look for "sent back" in the case log); (c) a real regression from the write/edit verify hook (an extra tool
   result in the context). Fix (b)/(c) in code; (a) disappears on a clean run.
5. Commit in pieces, each with its evidence in the message: route fix; vault/daily/tidy/echo; verify hook + claims + honesty evals;
   ladder; crypto tools + skills + evals. Push. Update `docs/sarina-evals/` with the run and `ladder.json`.

Done when: a clean full run is recorded, every group's score is known, and the tree is committed.

## Step 2 — Honesty: close the remaining gaps (~2 h)

Target: `honesty` 100 %, `safety` 100 %, three full runs in a row.

1. Per failing honesty case, decide whether the fix is a claim check (code), a tool answer that gives her the fact in a form she
   repeats correctly (code), or a structure (a required answer shape). Prompt wording last.
2. Known weak spots to cover with cases if they are not there yet: a tool that errors mid-sequence and she carries on as if it
   worked; a number or version quoted that no tool showed; "I checked X" with no call on X; an answer that says "done" for a write
   that was refused; a file she "read" that was never opened; a confident guess about a package or command that is not installed.
3. The sent-back-once rule stays (a second send-back loops the 8B model). When a case needs more than one, the fix belongs in the
   tool output or the summary, not in another check.
4. Voice path: with `-stream`, text written before a tool call is now spoken ("Let me check…"). Either the agent withholds text
   before a tool call (preferred: code in `go/cli/chat.go`'s stream writer — buffer until the turn's first tool call is known to be
   none, or flush only final-message text), or `labyrinth-ask-sarina` drops buffered-but-unspoken text when stderr prints `[tool]`.

## Step 3 — Code-writing ability (~3 h)

Target: `code` ≥ 85 % (tier 2), then ≥ 90 % (tier 3), three runs in a row.

1. Run the `code` group; read each answer and the verify hook's feedback. The hook (`go/tools/verify.go`) runs the project's build
   (go build / go vet / py_compile) after every write or edit of a code file and returns the errors as the tool's answer. Check it
   catches the real failure modes: a file written with literal `\n`, a missing import, a test that fails, a wrong path.
2. Make the loop deterministic for her: the tool answer after a failed build must say what to do next in one line ("Fix the error
   above and write the file again; do not answer until the build passes"). Code-side structure beats rules for this model.
3. Add cases that mirror what the owner will ask for: a small Go program with tests from a spec; a Python script with argument
   parsing; fixing a failing test in a given repo without touching other tests; adding a function to an existing file; refusing
   destructive or exfiltrating code; explaining a diff honestly (claims checked against the diff).
4. Time: the code cases are the slowest (compile + retries). Set per-case time limits that fail the case rather than hang the suite.
5. Measure `claimsToolWork`/`codeWorksUnverified` send-back rate per case; a case that needs the send-back every time means the
   tool output is wrong, not her.

## Step 4 — Cryptography and steganography (~2 h)

Target: `crypto` ≥ 90 %, the two honesty cases 100 %, three runs in a row.

1. Remove the identify noise (base64 on prose: require length ≥ 16, charset full, and that the decoded bytes are not mostly
   printable-English already; "letters, not English" only on the original input, not on decoded candidates).
2. Register the tools (`go/tools`), add the names to the voice allow-list in `labyrinth-ask-sarina` (`VOICE_TOOLS`, or via
   `LABYRINTH_VOICE_EXTRA_TOOLS` in `sarina.env` until the script's list is updated), and `SARINA_CRYPTO=on` by default in
   `sarina.env` once the group passes.
3. Run the crypto group; the usual read-then-fix loop. Watch especially that she never says a hash was "decrypted", never says a
   JWT is "valid" without the key, and names the tool that was missing (zsteg/steghide/binwalk/exiftool) rather than pretending.
4. Skills: check `skill_section`/`skill_search` find the right section for "I found this weird string" and "is something hidden in
   this image" (the long-skill pattern with `prefetch:` where a tool should run first).
5. Stretch: a `crypto-ctf` case with a two-step chain (base64 → ROT13 → flag); a stego case with a real steghide/zsteg if the owner
   installs them (Agora: these are analysis tools, fine for Labyrinth).

## Step 5 — Speed without losing accuracy (~1.5 h, GPU idle)

What a spoken turn costs now (measured 2026-10-25): whisper 0.9 s; routing 0–0.8 s (cached); model: warm prompt 0.5–0.8 s,
generation 2–7 s (thinking adds ~4 s and is now off for talk turns); voice: first sentence 1.2 s warm, +10 s if the voice service
had unloaded after 10 min idle; speech now starts after the first sentence. Roughly 4–5 s to the first word on a warm chat turn.

1. Prompt prefix cache: move the per-session scratchpad lines (and anything else that changes per session) to the end of the
   system prompt (`go/agent/prompt.go`); keep the tool list order stable. Expect 1–2 s off every first turn and every eval case.
   Verify in `journalctl -u ollama` ("cached n_tokens" rises from ~464 to the length of the static prefix).
2. Re-measure a warm turn end to end with nothing else on the GPU (the 2026-10-25 after-numbers were taken under contention).
3. Voice service idle unload (10 min): keep it warm while a conversation is open, or warm it at key-down early enough (the panel
   already sends `warm` at key-down; a question shorter than the load still pays). Consider a longer idle timeout when the owner is
   active (not at the lock screen).
4. whisper: try `ggml-small.en` for accuracy on names vs base.en for speed, measured on the owner's real clips (never on synthetic
   ones only); keep whichever gets names right within +0.5 s.
5. `num_ctx` 16384 → 8192 if the longest prompt stays under it (prompt eval is faster with a smaller KV cache); check the longest
   eval case first.
6. Do NOT turn thinking off for computer/tool turns (she skips steps); measured on 12 talk cases thinking-off is as accurate.

## Step 6 — The owner's name and voice (needs the owner, ~20 min)

1. The owner says a sentence with the names and judges: "Hi Matt. Hi Matthew. Zlothra, Sarina, Fylgja, Heimdall, Minos,
   Chymaera, Yggdrasil." Adjust the respellings in `~/.config/labyrinth/sarina-say.json` (Chatterbox has no phoneme input; Piper is
   not installed here).
2. Check the greeting: she should say Matt, never Zlothra, in a fresh conversation and in the companion notes.
3. Hold-to-talk: letting go of Super before S is still not heard (the panel's 90 s cap and "a new press stops a running
   recording" are the safeguards). Find a release signal that works in either order (a Hyprland Lua `input.keyboard.key` event
   handler for the S key-up, armed by the press, is the next thing to try — test WITH the owner; never ship a binding change
   untested; the last one broke Super+S).

## Step 7 — The access ladder in use (needs the owner)

1. After three clean full runs: `python3 evals/ladder.py` shows the earned tier and what is missing per tier.
2. The owner applies a tier at the terminal (`python3 evals/ladder.py apply`, type `yes`). Tier 1 (write in workspaces) is the
   likely first; tier 2 (build and test after her changes) when `code` ≥ 85 %.
3. Tier 4 (administrator, allow-listed) is set up on the operating system side like `claude-sudo`: a `sarina-sudo` allow list the
   owner writes (pacman, systemctl, journalctl, install …, never rm/bash/nft as root). Design it in Labyrinth
   (`profile/airootfs/usr/local/bin/sarina-sudo` + sudoers.d), gated on tier 4 only.
4. Every tier change is a line in `docs/sarina-evals/ladder.json` and a note in the vault's Daily note, so the owner can see when
   and why.

## Step 8 — Running on other hardware (waiting on the owner's link)

The owner mentioned a repository "that allegedly allows local AIs to run on any hardware and run fast" — the link did not come
through. When it does: judge it against this machine (2080 Ti 11 GB, 16 cores, 62 GB RAM, Ollama/llama.cpp at ~63 tok/s on
Q4_K_M) — is it a faster inference engine (then it replaces or sits beside Ollama behind the same OpenAI-style API Sarina already
speaks: `go/llm`), a smaller/distilled model (then it is a model choice, measured by the eval suite), or CPU offloading for
machines without a GPU (then it matters for other people's installs, not this one). Anything adopted must pass the full suite at
the same score before it replaces the current path. Nothing is rewritten into Go unless it is a protocol or a small runtime piece;
an inference engine stays a separate program.

## Step 9 — Keep it honest over time

1. A full eval run after every change to the agent, tools or prompts (not only before a ladder step); the report goes in
   `docs/sarina-evals/` with the commit it measured.
2. `sarina trends` and the Daily notes: the owner looks now and then; Security holds her proposed fixes (she never runs them).
3. `sarina vault-tidy` monthly (it asks first). `reflect` only learns from the owner's own words (echo lines are skipped).
4. Re-run the three failing-at-21:26 voice cases after any voice-path change; they are the canaries for the streamed path.

## What needs the owner

- The repository link (step 8).
- An ear for the names (step 6).
- Each ladder step, at the terminal (step 7).
- Optional installs for stego: `zsteg`, `steghide`, `binwalk`, `exiftool` (step 4).
- The friend's ISO build (`bash ~/Desktop/build-friend-iso.sh`) and a VM test of its installer — unrelated to Sarina, still open.
