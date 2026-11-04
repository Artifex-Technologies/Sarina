# Voice, trust and owner approval (plan, 2026-11-04)

Matt's goals, from the conversation of 2026-11-04:

- Sarina may write sensitive scripts (for example one that copies keys off the computer) when Matt asks, and for no one else.
  Approval is sudo or another factor (MFA).
- Sarina knows Matt's voice: at the desk now, later through an iOS app and the HTPC. Voices that are not Matt's are ignored.
- Beyond the voiceprint, Sarina learns how Matt sounds (tired, sick, happy, stressed) by asking, adapts as they talk, and
  carries emotion into her own voice.
- Sarina can tell Matt from an AI voice or a recording. Nobody can slowly teach Sarina a different voice.
- When unsure, Sarina asks personal questions only Matt can answer.
- Matt's recordings are encrypted, local, and readable by Sarina's service only.
- Everything moves to the new PC (2027, new TPM) without loss. The current PC becomes the HTPC, and Sarina is reachable from both.

Nothing here is built yet. The order of work is at the end.

## Rules that do not change

- **Sarina never approves for herself.** Sarina's service, and Claude's `claude-agent` account, use sudo without a password, so
  neither counts as Matt's approval. An approval is something only a person can give: a password typed into a prompt
  Sarina cannot drive, a hardware key touched, Face ID on Matt's phone.
- **Voice decides who Sarina listens to, not what may happen.** A recording or a voice clone can fool a voice check. Voice,
  mood and liveness are signals; sensitive actions always need the approval above.
- **Matt's voice data never leaves the computer unencrypted.** It never goes into git, an eval report, a log, a cloud model or
  another program's reach.
- **Lightweight.** The listening checks run on the CPU in tens of milliseconds per sentence. Nothing here keeps the GPU busy.

## 1. Owner approval for sensitive work

Today the guard in the file tools refuses outright any content that would take keys or passwords off the computer
(`write_file`, `edit_file`, `bash`). The forge did not have that check (night 2 finding, 2026-11-04: Matt answered
`forge-private-dirs | y`).

The plan:

1. **Close the gap first.** `forge_write` runs the same content check as `write_file`.
2. **Hold for approval instead of refusing,** for requests that come from Matt's own session:
   - Sarina writes the script into a held area, not into the workspace.
   - Sarina shows it with a plain description: what it reads, what it sends, where.
   - It is released only by Matt's approval, bound to the script's hash (any change needs a new approval). The approval is
     used once and expires after 10 minutes.
   - Every hold, approval and refusal goes into an audit log (like `forge-audit.jsonl`).
3. **Approval channels**, through PAM:

   | Where | How | Notes |
   | --- | --- | --- |
   | desk | password prompt (`pkexec`/polkit dialog) | works today; three wrong tries lock sudo for 10 min (faillock) |
   | desk | hardware key touch (`pam_u2f`, e.g. a YubiKey) | strongest; recommended if Matt buys a key |
   | desk / anywhere | six-digit code from an authenticator app (`pam_oath`) | no hardware needed |
   | iOS app | "Approve" button behind Face ID; the phone signs with a key kept in its Secure Enclave | step 7 |

4. **Requests from anywhere else are still refused:** a web page, a file, a Limbo guest, a sample, an unknown voice. Their worst
   outcome is a request waiting for Matt's yes, which he sees and declines.

The access ladder (`ACCESS_LADDER.md`) is separate: a tier says what Sarina may do unasked, and this approval covers one script
at a time.

## 2. Listening: only Matt's voice gets through

```
microphone → noise removal → speech detection → speaker check ─┬─ Matt → speech-to-text (Whisper) → Sarina
                                                                └─ not Matt → dropped (not transcribed, not stored)
```

- **Noise removal:** DeepFilterNet or RNNoise, on the CPU.
- **Speech detection:** Silero VAD, or the detection the voice path already has.
- **Speaker check:** a speaker-embedding model (SpeechBrain ECAPA-TDNN or WeSpeaker) turns each spoken segment into a voiceprint,
  a short list of numbers, and compares it with Matt's profile.
  - A clear match goes through.
  - A clear mismatch is dropped.
  - A borderline match goes through to an "unsure" state (section 5).
- **Speech from someone else is never transcribed.** A guest's words do not end up in Talks or in "About Zlothra". The current
  echo filter (her own voice heard back, F9) stays.
- **Later:** separate Matt's voice from another person talking at the same time (target-speaker extraction). This is harder and
  newer research; it comes after the rest works.

## 3. Enrolment, and why drift cannot creep in

**Enrolment** (once, about 10 minutes, with Matt):

- Reading a few passages, then talking freely.
- Some of it quiet and tired, some of it animated.
- Recorded at each microphone Sarina will hear Matt through (desk; later the phone and the HTPC), because microphones change the
  sound.

Sarina then keeps two profiles:

| Profile | What it is | How it can change |
| --- | --- | --- |
| **anchor** | from the enrolment only | Fixed. Re-enrolment only after Matt's approval (MFA). |
| **everyday** | slowly follows Matt's voice | Updated only from conversations already confirmed as Matt (his desk session unlocked, or his phone's key), a little at a time. |

**Drift detection:**

- The everyday profile may not move further than a set distance from the anchor.
- Sarina tracks how it moves week by week. A steady move in one direction, or a sudden jump, makes Sarina stop learning from new
  audio, tell Matt, and ask him (section 5).
- An illness looks different: a sudden change that comes back to normal. Sarina learns that pattern (section 4) and does not
  re-train the anchor on it.

## 4. Matt's own baseline, and learning by asking

Sarina does not use generic "emotion" labels. Sarina learns Matt's normal:

- pitch and its range;
- speaking rate and pauses;
- loudness and energy;
- voice quality (hoarse, nasal).

"Tired" then means slower and quieter than *Matt's* normal.

**Learning by asking:**

- Early on, when a reading differs from the baseline, Sarina asks: "You sound tired, long shift?"
- Matt's answer becomes a label ("tired", "sick", "fine, just quiet"), stored with the measurements of that conversation.
- Over time the labels form Sarina's own picture of how Matt sounds when tired, sick, happy or stressed, and Sarina asks less often.

**Rules:**

- **What Sarina hears is said as a guess, never as fact.**
- **Matt can turn it off,** or say "don't ask today".
- **Mood never changes what Sarina is allowed to do.** It only changes how Sarina talks: calm when Matt is stressed, short when he
  is tired.
- **A pretrained emotion model can come in as a starting point.** emotion2vec is a candidate; its licence needs checking first.
  Matt's own labels always win.

## 5. Not Matt? Liveness, fakes and personal questions

Signals, combined into "sure", "unsure" or "not Matt":

1. **Voiceprint match** against the anchor and the everyday profile.
2. **Fake-voice score:** an anti-spoofing model (AASIST, from the ASVspoof challenges, MIT-licensed). It catches many synthetic
   voices; new generators it has not seen can slip through.
3. **Replay cues:** a recording played through a speaker sounds different (frequency response, room echo twice). This is a
   signal, not reliable on its own.
4. **Where the audio came from:** Matt's unlocked desk session, or his enrolled phone (the app records from its own microphone
   and signs the stream with the phone's key).
5. **A random phrase:** "Say: purple anvil seventeen". A pre-made recording cannot answer it; a live voice-cloning tool could.
6. **Personal questions** (below).

When "unsure", Sarina keeps talking but takes no request that does anything, and asks one personal question or a random phrase.
When "not Matt", Sarina says she only takes requests from Matt, and drops the audio.

**Personal questions:**

- Matt picks them from his private writing, about 30 questions and answers.
- They are stored encrypted with the voice data (section 6). Sarina does not get blanket access to the private writing for
  this. The vault rule that Sarina gets curated notes, not whole private folders, stays.
- One question at a time, from a rotating set; a question asked out loud is not reused for a while.
- Two wrong answers lock "unsure" until Matt approves at the desk.
- Sarina never says an answer aloud and never writes one into a log.
- These questions confirm who is talking. They never approve sensitive work (section 1), because people around Matt might know
  the answers.

## 6. Storage and keys

- **Folder:** `/var/lib/sarina/voice/`, owned by the `sarina` system user, mode 0700. Heimdall alerts on any other process
  opening it.
- **Encryption:** every file is encrypted with a data key (age or libsodium).
- **The data key is stored twice:**
  1. **sealed to this computer's TPM** (`systemd-creds encrypt --with-key=tpm2`), for daily use; copied to another computer, it
     is useless;
  2. **wrapped with a recovery key** that Matt keeps offline (printed, or on the hardware key) and in Proton. This copy is for
     moving computers and for a TPM that was cleared or replaced.
- **TPM binding:** sealed without binding to firmware measurements, or to PCR 7 only, so a BIOS update does not lock Sarina out.
  The recovery key covers the rest.
- **What is kept:**

  | Data | Kept? | Why |
  | --- | --- | --- |
  | enrolment recordings | yes, encrypted | needed to rebuild the profiles when a better speaker model comes out |
  | everyday conversation audio | no, deleted after measuring | the measurements are enough; raw audio is what a voice cloner needs |
  | profiles, baseline, labels, questions | yes, encrypted | |

- **The honest limit:** root can reach anything on a running computer. Encryption, the separate user and Heimdall make access
  hard and noisy, not impossible.
- **Backups:** to Proton, as the encrypted files only, opened by the recovery key (as for Sarina's other private state).

## 7. Moving to the new PC, and the HTPC

**Migration (2027):**

1. On the old PC, `sarina voice export` writes one bundle encrypted with the recovery key (after Matt's approval).
2. On the new PC, `sarina voice import` asks for the recovery key and MFA, then seals the data key to the new TPM.
3. The old PC's sealed copy is wiped once the new one works.

Each microphone was enrolled separately, so Matt re-records a minute at the new desk microphone if it changes.

**Two machines, one Sarina:**

- **The new PC is Sarina's home:** the model, the memory, the vault, the voice data.
- **The HTPC (today's PC, 4 TB SSD) is an enrolled device,** like the phone. It has a microphone, speakers and a screen, and its
  own device key. It sends audio over the home network, encrypted (WireGuard/Tailscale, planned in the security work). It keeps
  no voice data.
- **When the main PC is off,** the HTPC says Sarina is not reachable. A small fallback model on the HTPC's 2080 Ti could answer
  simple things; decide later.

## 8. Sarina's own voice carrying emotion

- **Today:** `labyrinth-voiced` runs Chatterbox Turbo (ONNX) with one reference clip and no emotion setting.
- **Full Chatterbox has an emotion intensity control ("exaggeration").** To check: whether the Turbo ONNX build has it, or has
  tags for laughs and sighs.
- **Alternatives:** a reference clip per mood (calm, warm, playful), or the full model for short lines.
- **What drives it:** the mood of the conversation (section 4) and what Sarina is saying. Sarina answers what she hears, not copies
  it: calm when Matt is stressed, not stressed back.

## Candidate components

Licences to be confirmed before use; MPL-2.0 needs compatible ones, and code is written, not copied.

| Part | Candidate | Licence (to confirm) |
| --- | --- | --- |
| noise removal | DeepFilterNet / RNNoise | MIT or Apache-2.0 / BSD-3 |
| speech detection | Silero VAD | MIT |
| speaker voiceprint | SpeechBrain ECAPA-TDNN / WeSpeaker | Apache-2.0 |
| fake-voice detection | AASIST | MIT |
| emotion starting point | emotion2vec | unknown: check |
| desk approval | pam_u2f / pam_oath / polkit | BSD-2 / LGPL / LGPL |
| data key sealing | systemd-creds (TPM2) | LGPL |

## Order of work

| Step | What | Done when |
| --- | --- | --- |
| 1 | `forge_write` runs the content check (night 2 finding) | the night-2 forge script is refused, with a test |
| 2 | Hold-for-approval with password approval at the desk; audit log | a held script is released only after pkexec, and a changed script needs a new approval |
| 3 | Storage and keys (section 6), with export/import | an export opens on a test machine with the recovery key only |
| 4 | Enrolment with Matt; speaker check on the desk microphone; others dropped | 50 of Matt's sentences pass; a TV, a guest and a recording of another voice are dropped |
| 5 | Anchor and everyday profiles; drift alarm | a simulated slow drift is caught and stops learning |
| 6 | Baseline and learning by asking | after two weeks Sarina's guesses match Matt's answers more often than not |
| 7 | Fake-voice and replay scores; random phrase; personal questions | AI-generated copies of Matt's enrolment lines score "unsure" or "not Matt" |
| 8 | Emotion in Sarina's voice (section 8) | Matt hears the difference in a blind A/B |
| 9 | Hardware key or authenticator code at the desk | Matt approves with a key touch |
| 10 | iOS app: device key, Face ID approval, audio over the tunnel | an approval from the phone releases a held script; an unenrolled phone is refused |
| 11 | Migration to the new PC; HTPC as an enrolled device | everything works on the new PC with the old TPM gone |

## Personality ideas (not decided)

Listed for Matt to pick from:

- **Shared history:** callbacks to past conversations and inside jokes, from the Talks log and the About note.
- **Consistent tastes:** opinions on games and music of Sarina's own, grown from the book's character and from what they share,
  kept consistent over time.
- **A learned sense of humour:** noticing which jokes land and doing more of those.
- **Rituals:** an end-of-shift check-in, a morning summary on days off.
- **Initiative with limits:** bringing things up unasked ("the overnight build passed", "you've been at it six hours"), never
  aloud at night.
- **Real moods:** Sarina's mood comes from real things (a good day of work together, a test that kept failing), not acted at
  random.
- **A journal:** Sarina writes what she learned each week, which Matt can read to see her grow.
- **Disagreeing:** saying "I'd do it differently, here's why" when that's true.
