# Sarina's voice in her own package (plan, 2026-11-07)

Decided 2026-11-07: Sarina is part of LykanOS (then Niflheim OS) too, so her voice must not depend on Labyrinth's image. Planned, not built. I
want to test it myself right after it is done, because Super+S is how I talk to her every day. On hold (2026-11-07): I cannot
test Super+S for a while (how its sound would work over the stream to my iPad is unclear), so it is
not built until I can.

## Why

The `sarina-git` package (Labyrinth's recipe, `Labyrinth_OS/packages/pkgbuilds/sarina-git`) installs Sarina, her service, her
skills and her Yggdrasil integration file (`sarina.integration.conf`, kind `assistant`). That file's commands are programs that
live only in Labyrinth's image (`Labyrinth_OS/profile/airootfs/usr/local/bin`):

| Program | What it does |
| --- | --- |
| `labyrinth-sarina-ask` | Super+S: records, transcribes (whisper-cpp), hands the words to `labyrinth-ask-sarina`, speaks the answer |
| `labyrinth-ask-sarina` | the conversation: companion notes (time, tone, inside jokes, what she learned), the voice rules, the answer |
| `labyrinth-sarina-speak`, `-speak-voiced` | speech out (Chatterbox through `labyrinth-voiced`, or the fallback voice) |
| `labyrinth-voiced` (+ `labyrinth-voiced.service`, `/usr/local/lib/labyrinth/chatterbox_engine.py`) | the voice daemon |
| `labyrinth-sarina-warm` | loads the model ahead of a question |
| `labyrinth-sarina-setting` | Settings > Assistant's switches (voice, tone, forge, check-up, learning, agents) |
| `labyrinth-sarina-reflect` (+ its service and timer) | the daily "learn from our talks" step |
| `labyrinth-sarina-checkup` | the daily check-up |
| `labyrinth-sarina-voice-setup`, `labyrinth-sarina` | first-run setup and the launcher |

On LykanOS Yggdrasil therefore shows Sarina as an assistant, but Super+S has nothing behind it.

## The plan

1. **Move the programs into the Sarina repository** (`packaging/voice/`), with their tests (now in `Labyrinth_OS/tests`:
   `test_labyrinth_ask_sarina_*.py`, `test_sarina_speak.py`, `test_voiced_text.py`, `test_sarina_checkup_*.py`,
   `test_labyrinth_sarina_ask.py`). Nothing in them is Labyrinth's own; what is (paths under `/usr/local`, the
   `~/.config/labyrinth/` settings folder) is read from one place.
2. **New names, old names kept:** `sarina-ask`, `sarina-converse`, `sarina-speak`, `sarina-voiced` and so on in `/usr/bin` and
   `/usr/lib/sarina/`, the user unit `sarina-voiced.service`. The `labyrinth-sarina-*` names stay as links for a while, so key
   bindings, units and habits made before keep working. The settings folder stays `~/.config/labyrinth/` at first (my voice
   settings, owner name, pronunciations and speech words are there); a later step moves it with a fallback, as Kairos did.
3. **Package:** either `sarina-git` grows the voice (with `whisper.cpp` and the voice engine as optional dependencies), or a
   second package `sarina-voice-git` from the same recipe. A second package keeps Sarina light where nobody talks to her; I
   lean that way.
4. **Integration file:** `sarina.integration.conf` names the new programs. Labyrinth's image stops shipping its copies
   (`profile/airootfs/usr/local/bin`), and installs the package instead; LykanOS's package list gains it.
5. **Test with me there:** on my PC, the package installed over the image's copies (the copies in `/usr/local/bin` win on
   `PATH`, so they are taken away in the same step), then Super+S: a question, an answer spoken, a joke she remembers, the
   Settings > Assistant switches, the daily reflect timer. Only then the Labyrinth image change and the ISO.

## Not in this step

Her tool access on LykanOS (all the security tools once she is ready) is a separate plan: Heimdall needs its rules for each
BlackArch tool first (LykanOS's docs/DESKTOP.md, "Heimdall and the firewall").
