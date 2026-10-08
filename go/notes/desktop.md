# desktop-audit (go/desktop)

Clean-room-safe port of the original `sarina/desktop/*` and `tests/test_desktop_*` to Go (stdlib only).
Command: `sarina desktop-audit` (registered via `cli.Register`; wired by one import line in `cmd/sarina/main.go`).

- Ported exactly: freedesktop category tables, BlackArch group map (52, incl. `reocn`), description rules (first match wins),
  .desktop parser (locale keys and action groups ignored), `RewriteCategories` (one line of a COPY; marker kept above the
  group separator), fresh render, pacman `-Qi`/`-Ql` parsing (two subprocess calls total, never errors), application-shape
  filter, explicit-only filter, NoDisplay/Hidden never touched, audit report, apply (user dir, `X-Sarina-Generated`),
  remove-generated (reads each file, hand-written files untouched).
- Semantic fallback is behind `Scorer`/`Classifier` interfaces. `SemanticClassifier` wraps a Scorer with
  `go/analysiscache` (sha256 content keys, namespace = sha256 of the prototype set, same serialisation as Python),
  caches only (label, score) and applies `--similarity-threshold` fresh after lookup. Cache: `~/.sarina/cache/desktop_classify.json`
  (`Env.Home` honoured), skipped with `--no-cache`, only used when a model is in use.
- ONNX embeddings are Python-only and no helper module with a defined protocol exists in the repo, so
  `desktop.HelperScorer` (the hook for `--model-dir`) returns "needs the Python helper"; the command prints
  `semantic inference unavailable: <reason>` and continues deterministically. Nothing was invented. `EmbeddingScorer`
  (cosine over prototypes via an `Embedder` interface) is ready for a real helper.
- Tests: `desktop/desktop_test.go`, offline (fake pacman runner, temp dirs, call-counting fake scorer and embedder).
- Deviations: cosine similarity is my own (the original `rank_by_similarity` was not readable); report-only footer wording
  is mine ("Nothing was written. Re-run with --apply ...") since the spec gives no exact text.
