# Training Sarina on her own good answers

1. `python3 sarina_training.py review` writes a review sheet into `~/Documents/Obsidian/Project Docs/`.
2. Open it, tick `[x]` the answers that are right and sound like her (edit an answer to improve it). Only ticked ones count.
3. `python3 sarina_training.py export "<the sheet>.md"` writes `~/.local/share/sarina/training/approved.jsonl`.
4. When there are a few hundred: train a small LoRA with Soup using `soup.yaml` (hours of GPU; not while gaming), turn it into an Ollama model,
   and run the evals (`evals/run_evals.py`) with `OPENAI_MODEL` set to it before switching her over. Keep the old model if the evals get worse.

Credentials, eval runs, and answers her honesty checks sent back are never offered for training.
