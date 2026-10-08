# Behaviour specs (clean-room input)

These documents describe what each part of Sarina must DO, so it can be rebuilt
from scratch without reference to the existing derived code (see
`docs/PROVENANCE.md`). They are the only input an implementer may use besides
public protocol specs (OpenAI chat-completions, MCP, LSP, JSON-RPC, HTTP).

Rules for spec authors:
- Describe externally observable behaviour: CLI commands/flags, tool names and
  parameter schemas, JSON/file formats, env vars, ports, error conditions, limits.
- Do NOT paste or closely paraphrase source code, comments, docstrings, system
  prompts, tool descriptions or help text. Describe what they must convey; the
  new text is written fresh.
- No code beyond small wire-format / CLI examples.
- For every module, rate it: ESSENTIAL (Sarina cannot work without it),
  USEFUL, or DROP-CANDIDATE (parity feature a local-model security agent does
  not need), with one line why.
