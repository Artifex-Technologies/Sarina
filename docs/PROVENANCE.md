# Provenance

Sarina began as a fork of [ClawCode](https://github.com/HarnessLab/claw-code-agent),
which has **no license** (all rights reserved by default). The derived code
below has since been replaced by a clean-room rewrite, and the owner chose **MPL 2.0**
for the current tree on 2026-10-06 (see `LICENSE`). The old git history that
contained the derived code has since been removed (2026-10-08). The ClawCode
maintainers were contacted several times without a reply; silence is not
permission, which is why the code was replaced rather than relicensed.

## Audit (2026-10-04)

Method: every non-blank, non-comment line of each Sarina Python file compared
against every file in upstream ClawCode HEAD (`src/`, `tests/`). "Shared" is the
share of a file's lines that also appear in some upstream file. It detects
copies and renames, not a rewrite that keeps the same design, and says nothing
about how closely ClawCode itself follows the proprietary Claude Code npm source.

| Area | Result |
|---|---|
| `sarina/` (97 files, ~31,800 lines) | ~85% of lines shared; 55 files >=90%, 4 files 60-90%, 6 files 25-60%, 29 files <25% |
| `tests/` (96 files, ~16,300 lines) | ~72% of lines shared; 56 files >=90% |
| Removed in this change (byte-identical to upstream, unreferenced) | `images/logo.png`, `images/demo1.gif`, `images/demo_2.gif`, `install_dockor.sh` |
| Lightly modified from upstream | `harbor_adapter.py`, `sarina/gui/static/{app.js,app.css,index.html}`, `TESTING_GUIDE.md` (about half) |
| Original | `sarina/{chymaera,desktop,voice,ml}/`, `memory_*.py`, `session_search_fts.py`, `analysis_cache.py`, `sentry_governance.py`, `escalation_runtime.py`, `system_resources.py`, most of `service/`, `packaging/arch/`, `tools/voice_clone/`, `docs/ECOSYSTEM.md` |

Everything marked Original still carries a small hook into a derived file
(`agent_runtime.py`, `agent_tools.py`, `main.py`); those hooks have to be
re-made in whatever replaces the core.

## Remediation rules

1. **Do not add new features to a derived file** (>=25% shared). Put new code in
   a new, original module, with at most a one-line hook into the derived file.
2. **Replacement is clean-room**: the replacement is designed from a written
   behaviour spec (what a module must do, its interfaces and observable
   behaviour), not by porting or paraphrasing the existing file. A line-by-line
   translation into another language is still derived.
3. The planned replacement core is written in Go (matches Chymaera_Sentry and
   Limbo; single static binary for Chymaera_Linux). The ML and voice code stays
   Python as optional helper processes.
4. Re-run the audit before any public release. (The git history that contained
   the derived code has been removed.)

## Progress (2026-10-05)

The Go replacement in `go/` is feature-complete against the specs (see
CLAUDE.md log). Derived Python (`sarina/` files rated verbatim/heavy, and the
matching `tests/`) is still in the tree and is deleted only after the
conformance runs pass and the owner approves. Re-run the audit before any
license decision; git history still contains the derived code.

## Derived Python deleted (2026-10-05)

Removed 165 files: every `sarina/` module that shared >=25% of its lines with
upstream (except the owner's original `sentry_governance.py` and
`system_resources.py`), the old `sarina/gui/` and `sarina/service/`, the
matching derived tests, `benchmarks/`, `harbor_adapter.py` and
`TESTING_GUIDE.md`. `pyproject.toml` no longer declares console scripts or web
dependencies.

**Legacy originals removed afterwards (2026-10-06):** the modules that no longer
imported (`sarina/chymaera/analysis.py`, `chymaera/limbo_tools.py`, `desktop/*`,
`escalation_runtime.py`, `system_resources.py`, `ml/*`, `session_search_fts.py`,
`voice/loop.py`) and the tests that covered only them. Each is superseded by a Go
port (`go/chymaera`, `go/desktop`, `go/tiers`, `go/sessionsearch`, `go/limbo`) or by
`python/sarina_helpers`. The owner's still-working originals remain in `sarina/`
as reference (`chymaera/` console, firmware, fleet and limbo client, `memory_*`,
`voice/*`, `analysis_cache.py`, `sentry_governance.py`); all 234 remaining Python
tests pass. They can go once the Go side has been used in the field.

## Re-audit after the rewrite (2026-10-05)

Method: every 10-word sequence in the current files compared against upstream
ClawCode (`src/`, `tests/`, docs) and against every derived file deleted in
9508cc0 (taken from git history). Word sequences rather than lines, because Go
and Python cannot be compared line by line.

| Area | Files | 10-grams | Matching |
|---|---:|---:|---:|
| `go/` (source, tests, embedded UI) | 186 | 216,376 | 31 (0.01%) |
| `python/sarina_helpers` | 19 | 8,741 | 44 (0.50%) |
| `docs/` + README | 11 | 51,506 | 17 (0.03%) |
| `sarina/` legacy originals | 31 | 22,856 | 126 (0.55%) |

Every match inspected is generic boilerplate: the HTML doctype header, the
standard system-font stack, API-key prefix formats in the redaction patterns,
Python import lines, a `glob`/`mtime` sort idiom, and a flag default from the
owner's own voice-chat design. No copied logic, prompts or UI text was found.
This is a textual check; it does not prove independence of design. The git
history containing the derived code has since been removed, and MPL 2.0 was chosen
(2026-10-06).

## Per-file table

### Source (`sarina/`)

| File | Lines | Shared with upstream | Closest upstream file |
|---|---:|---:|---|
| `sarina/agent_context_usage.py` | 312 | 100% | `src/agent_context_usage.py` |
| `sarina/agent_manager.py` | 279 | 100% | `src/agent_manager.py` |
| `sarina/agent_registry.py` | 616 | 100% | `src/agent_registry.py` |
| `sarina/context.py` | 40 | 100% | `src/context.py` |
| `sarina/gui/background_routes.py` | 66 | 100% | `src/gui/background_routes.py` |
| `sarina/gui/mcp_routes.py` | 69 | 100% | `src/gui/mcp_routes.py` |
| `sarina/gui/memory_routes.py` | 123 | 100% | `src/gui/memory_routes.py` |
| `sarina/plan_runtime.py` | 369 | 100% | `src/plan_runtime.py` |
| `sarina/session_env_vars.py` | 31 | 100% | `src/session_env_vars.py` |
| `sarina/task.py` | 119 | 100% | `src/task.py` |
| `sarina/token_budget.py` | 130 | 100% | `src/token_budget.py` |
| `sarina/plugin_runtime.py` | 595 | 100% | `src/plugin_runtime.py` |
| `sarina/remote_runtime.py` | 514 | 100% | `src/remote_runtime.py` |
| `sarina/account_runtime.py` | 430 | 100% | `src/account_runtime.py` |
| `sarina/agent_prompting.py` | 363 | 100% | `src/agent_prompting.py` |
| `sarina/remote_trigger_runtime.py` | 336 | 100% | `src/remote_trigger_runtime.py` |
| `sarina/workflow_runtime.py` | 286 | 100% | `src/workflow_runtime.py` |
| `sarina/lsp_runtime.py` | 1030 | 100% | `src/lsp_runtime.py` |
| `sarina/agent_slash_commands.py` | 2222 | 99% | `src/agent_slash_commands.py` |
| `sarina/gui/server.py` | 632 | 99% | `src/gui/server.py` |
| `sarina/hook_policy.py` | 307 | 99% | `src/hook_policy.py` |
| `sarina/worktree_runtime.py` | 409 | 99% | `src/worktree_runtime.py` |
| `sarina/agent_plugin_cache.py` | 133 | 99% | `src/agent_plugin_cache.py` |
| `sarina/openai_compat.py` | 380 | 99% | `src/openai_compat.py` |
| `sarina/agent_session.py` | 716 | 99% | `src/agent_session.py` |
| `sarina/search_runtime.py` | 553 | 99% | `src/search_runtime.py` |
| `sarina/bundled_skills.py` | 210 | 99% | `src/bundled_skills.py` |
| `sarina/team_runtime.py` | 353 | 99% | `src/team_runtime.py` |
| `sarina/paste_refs.py` | 86 | 99% | `src/paste_refs.py` |
| `sarina/agent_context.py` | 407 | 99% | `src/agent_context.py` |
| `sarina/builtin_agents.py` | 317 | 99% | `src/builtin_agents.py` |
| `sarina/session_memory_compact.py` | 389 | 99% | `src/session_memory_compact.py` |
| `sarina/mcp_runtime.py` | 817 | 99% | `src/mcp_runtime.py` |
| `sarina/ask_user_runtime.py` | 291 | 99% | `src/ask_user_runtime.py` |
| `sarina/compact.py` | 500 | 99% | `src/compact.py` |
| `sarina/gui/search_routes.py` | 70 | 99% | `src/gui/search_routes.py` |
| `sarina/microcompact.py` | 181 | 98% | `src/microcompact.py` |
| `sarina/tokenizer_runtime.py` | 172 | 98% | `src/tokenizer_runtime.py` |
| `sarina/gui/plugins_routes.py` | 51 | 98% | `src/gui/plugins_routes.py` |
| `sarina/__init__.py` | 128 | 98% | `src/__init__.py` |
| `sarina/task_runtime.py` | 564 | 97% | `src/task_runtime.py` |
| `sarina/gui/__main__.py` | 137 | 97% | `src/gui/__main__.py` |
| `sarina/gui/worktree_routes.py` | 65 | 97% | `src/gui/worktree_routes.py` |
| `sarina/background_runtime.py` | 349 | 96% | `src/background_runtime.py` |
| `sarina/gui/account_routes.py` | 52 | 96% | `src/gui/account_routes.py` |
| `sarina/gui/remote_trigger_routes.py` | 76 | 96% | `src/gui/remote_trigger_routes.py` |
| `sarina/gui/team_routes.py` | 76 | 96% | `src/gui/team_routes.py` |
| `sarina/gui/ask_user_routes.py` | 75 | 96% | `src/gui/ask_user_routes.py` |
| `sarina/gui/remote_routes.py` | 50 | 96% | `src/gui/remote_routes.py` |
| `sarina/agent_runtime.py` | 4448 | 95% | `src/agent_runtime.py` |
| `sarina/agent_tools.py` | 3497 | 94% | `src/agent_tools.py` |
| `sarina/config_runtime.py` | 262 | 94% | `src/config_runtime.py` |
| `sarina/gui/workflow_routes.py` | 47 | 94% | `src/gui/workflow_routes.py` |
| `sarina/gui/plans_routes.py` | 69 | 91% | `src/gui/plans_routes.py` |
| `sarina/agent_types.py` | 177 | 91% | `src/agent_types.py` |
| `sarina/gui/tasks_routes.py` | 148 | 88% | `src/gui/tasks_routes.py` |
| `sarina/session_store.py` | 299 | 87% | `src/session_store.py` |
| `sarina/main.py` | 1955 | 73% | `src/main.py` |
| `sarina/agent_result_serialize.py` | 28 | 71% | `src/gui/server.py` |
| `sarina/service/__main__.py` | 73 | 51% | `src/gui/__main__.py` |
| `sarina/gui/__init__.py` | 3 | 33% | `src/agent_runtime.py` |
| `sarina/service/app.py` | 142 | 30% | `src/gui/server.py` |
| `sarina/service/session_pool.py` | 100 | 28% | `src/gui/server.py` |
| `sarina/sentry_governance.py` | 51 | 25% | `src/agent_tools.py` |
| `sarina/system_resources.py` | 131 | 25% | `src/agent_runtime.py` |
| `sarina/escalation_runtime.py` | 158 | 23% | - |
| `sarina/desktop/packages.py` | 161 | 19% | - |
| `sarina/service/auth.py` | 16 | 19% | - |
| `sarina/session_search_fts.py` | 161 | 18% | - |
| `sarina/analysis_cache.py` | 46 | 17% | - |
| `sarina/chymaera/agent_fleet.py` | 196 | 17% | - |
| `sarina/ml/__init__.py` | 32 | 16% | - |
| `sarina/memory_security.py` | 110 | 15% | - |
| `sarina/voice/stt.py` | 53 | 15% | - |
| `sarina/voice/loop.py` | 62 | 15% | - |
| `sarina/chymaera/__init__.py` | 35 | 14% | - |
| `sarina/chymaera/firmware.py` | 269 | 14% | - |
| `sarina/desktop/entries.py` | 221 | 14% | - |
| `sarina/memory_review.py` | 259 | 14% | - |
| `sarina/chymaera/agent_usage.py` | 225 | 14% | - |
| `sarina/ml/session_search.py` | 89 | 13% | - |
| `sarina/memory_runtime.py` | 240 | 13% | - |
| `sarina/voice/tts.py` | 94 | 13% | - |
| `sarina/chymaera/agent_launch.py` | 174 | 13% | - |
| `sarina/desktop/propose.py` | 378 | 12% | - |
| `sarina/voice/audio_io.py` | 95 | 12% | - |
| `sarina/chymaera/analysis.py` | 132 | 11% | - |
| `sarina/voice/__init__.py` | 53 | 11% | - |
| `sarina/service/__init__.py` | 9 | 11% | - |
| `sarina/desktop/__init__.py` | 73 | 11% | - |
| `sarina/chymaera/console.py` | 138 | 11% | - |
| `sarina/chymaera/limbo.py` | 162 | 10% | - |
| `sarina/ml/embeddings.py` | 117 | 10% | - |
| `sarina/desktop/categories.py` | 148 | 7% | - |

### Tests (`tests/`)

| File | Lines | Shared with upstream | Closest upstream file |
|---|---:|---:|---|
| `tests/test_benchmark_artifacts.py` | 114 | 100% | `tests/test_benchmark_artifacts.py` |
| `tests/test_benchmark_download_datasets.py` | 160 | 100% | `tests/test_benchmark_download_datasets.py` |
| `tests/test_agent_manager.py` | 473 | 100% | `tests/test_agent_manager.py` |
| `tests/test_agent_plugin_cache.py` | 372 | 100% | `tests/test_agent_plugin_cache.py` |
| `tests/test_session_store.py` | 469 | 100% | `tests/test_session_store.py` |
| `tests/test_gui_server.py` | 374 | 99% | `tests/test_gui_server.py` |
| `tests/test_agent_registry.py` | 108 | 99% | `tests/test_agent_registry.py` |
| `tests/test_secret_redaction_on_tool_ingestion.py` | 99 | 99% | `tests/test_secret_redaction_on_tool_ingestion.py` |
| `tests/test_gui_tasks_api.py` | 89 | 99% | `tests/test_gui_tasks_api.py` |
| `tests/test_terminal_bench_local.py` | 167 | 99% | `tests/test_terminal_bench_local.py` |
| `tests/test_gui_account_api.py` | 76 | 99% | `tests/test_gui_account_api.py` |
| `tests/test_gui_worktree_api.py` | 72 | 99% | `tests/test_gui_worktree_api.py` |
| `tests/test_gui_memory_api.py` | 71 | 99% | `tests/test_gui_memory_api.py` |
| `tests/test_gui_plans_api.py` | 67 | 99% | `tests/test_gui_plans_api.py` |
| `tests/test_openai_compat.py` | 125 | 98% | `tests/test_openai_compat.py` |
| `tests/test_gui_teams_api.py` | 62 | 98% | `tests/test_gui_teams_api.py` |
| `tests/test_lsp_runtime.py` | 59 | 98% | `tests/test_lsp_runtime.py` |
| `tests/test_gui_remote_triggers_api.py` | 56 | 98% | `tests/test_gui_remote_triggers_api.py` |
| `tests/test_external_slash_commands.py` | 111 | 98% | `tests/test_external_slash_commands.py` |
| `tests/test_churn_and_bash_banner.py` | 109 | 98% | `tests/test_churn_and_bash_banner.py` |
| `tests/test_cost_exit_diff.py` | 163 | 98% | `tests/test_cost_exit_diff.py` |
| `tests/test_edit_loop_and_self_authored.py` | 102 | 98% | `tests/test_edit_loop_and_self_authored.py` |
| `tests/test_new_slash_commands.py` | 254 | 98% | `tests/test_new_slash_commands.py` |
| `tests/test_settings_slash_commands.py` | 143 | 98% | `tests/test_settings_slash_commands.py` |
| `tests/test_web_fetch_binary_refusal.py` | 95 | 98% | `tests/test_web_fetch_binary_refusal.py` |
| `tests/test_agent_tools_secret_path_guard.py` | 93 | 98% | `tests/test_agent_tools_secret_path_guard.py` |
| `tests/test_task_runtime.py` | 223 | 98% | `tests/test_task_runtime.py` |
| `tests/test_remote_slash_commands.py` | 89 | 98% | `tests/test_remote_slash_commands.py` |
| `tests/test_gui_background_api.py` | 85 | 98% | `tests/test_gui_background_api.py` |
| `tests/test_extended_tools.py` | 126 | 98% | `tests/test_extended_tools.py` |
| `tests/test_benchmark_temp_workspaces.py` | 35 | 97% | `tests/test_benchmark_temp_workspaces.py` |
| `tests/test_gui_remote_api.py` | 67 | 97% | `tests/test_gui_remote_api.py` |
| `tests/test_worktree_runtime.py` | 166 | 97% | `tests/test_worktree_runtime.py` |
| `tests/test_gui_ask_user_api.py` | 66 | 97% | `tests/test_gui_ask_user_api.py` |
| `tests/test_paste_refs.py` | 99 | 97% | `tests/test_paste_refs.py` |
| `tests/test_gui_search_api.py` | 64 | 97% | `tests/test_gui_search_api.py` |
| `tests/test_gui_workflow_api.py` | 62 | 97% | `tests/test_gui_workflow_api.py` |
| `tests/test_plan_runtime.py` | 175 | 97% | `tests/test_plan_runtime.py` |
| `tests/test_misc_slash_commands.py` | 115 | 97% | `tests/test_misc_slash_commands.py` |
| `tests/test_mcp_runtime.py` | 327 | 96% | `tests/test_mcp_runtime.py` |
| `tests/test_agent_tools_security.py` | 208 | 96% | `tests/test_agent_tools_security.py` |
| `tests/test_gui_mcp_api.py` | 75 | 96% | `tests/test_gui_mcp_api.py` |
| `tests/test_session_env_vars.py` | 47 | 96% | `tests/test_session_env_vars.py` |
| `tests/test_team_runtime.py` | 69 | 96% | `tests/test_team_runtime.py` |
| `tests/test_gui_plugins_api.py` | 45 | 96% | `tests/test_gui_plugins_api.py` |
| `tests/test_background_runtime.py` | 100 | 95% | `tests/test_background_runtime.py` |
| `tests/test_compact.py` | 329 | 95% | `tests/test_compact.py` |
| `tests/test_remote_trigger_runtime.py` | 77 | 95% | `tests/test_remote_trigger_runtime.py` |
| `tests/test_agent_prompting.py` | 269 | 94% | `tests/test_agent_prompting.py` |
| `tests/test_agent_context_usage.py` | 35 | 94% | `tests/test_agent_context_usage.py` |
| `tests/test_remote_runtime.py` | 112 | 94% | `tests/test_remote_runtime.py` |
| `tests/test_token_budget.py` | 91 | 93% | `tests/test_token_budget.py` |
| `tests/test_agent_slash_commands.py` | 520 | 93% | `tests/test_agent_slash_commands.py` |
| `tests/test_account_runtime.py` | 79 | 92% | `tests/test_account_runtime.py` |
| `tests/test_search_runtime.py` | 114 | 91% | `tests/test_search_runtime.py` |
| `tests/test_agent_runtime.py` | 3046 | 91% | `tests/test_agent_runtime.py` |
| `tests/test_ask_user_runtime.py` | 48 | 90% | `tests/test_ask_user_runtime.py` |
| `tests/test_workflow_runtime.py` | 69 | 88% | `tests/test_workflow_runtime.py` |
| `tests/test_agent_context.py` | 225 | 87% | `tests/test_agent_context.py` |
| `tests/test_tokenizer_runtime.py` | 58 | 84% | `tests/test_tokenizer_runtime.py` |
| `tests/test_config_runtime.py` | 91 | 77% | `tests/test_config_runtime.py` |
| `tests/test_main.py` | 445 | 54% | `tests/test_main.py` |
| `tests/test_session_search_fts.py` | 90 | 37% | `tests/test_agent_runtime.py` |
| `tests/test_service_server.py` | 105 | 36% | `tests/test_gui_server.py` |
| `tests/test_analysis_cache.py` | 60 | 25% | `tests/test_compact.py` |
| `tests/test_voice_clone_clean_checkpoint.py` | 26 | 23% | - |
| `tests/test_voice_loop.py` | 149 | 23% | - |
| `tests/test_memory_runtime.py` | 188 | 22% | - |
| `tests/test_escalation_runtime.py` | 149 | 21% | - |
| `tests/test_service_tool_allowlist.py` | 84 | 21% | - |
| `tests/test_service_session_pool.py` | 53 | 19% | - |
| `tests/test_voice_clone_isolate_vocals.py` | 51 | 18% | - |
| `tests/test_system_resources.py` | 117 | 17% | - |
| `tests/test_voice_stt.py` | 41 | 17% | - |
| `tests/test_session_memory_compact.py` | 36 | 17% | - |
| `tests/test_memory_review.py` | 141 | 16% | - |
| `tests/test_ml_session_search.py` | 147 | 14% | - |
| `tests/test_voice_clone_build_corpus.py` | 99 | 14% | - |
| `tests/test_blackarch_compat.py` | 178 | 12% | - |
| `tests/test_sentry_governance.py` | 96 | 11% | - |
| `tests/test_chymaera_console.py` | 181 | 10% | - |
| `tests/test_microcompact.py` | 79 | 10% | - |
| `tests/test_memory_security.py` | 81 | 10% | - |
| `tests/test_desktop_propose.py` | 312 | 10% | - |
| `tests/test_desktop_entries.py` | 190 | 9% | - |
| `tests/test_ml_embeddings.py` | 96 | 9% | - |
| `tests/test_chymaera_agent_launch.py` | 145 | 9% | - |
| `tests/test_chymaera_agent_fleet.py` | 170 | 9% | - |
| `tests/test_voice_tts.py` | 106 | 8% | - |
| `tests/test_desktop_packages.py` | 131 | 8% | - |
| `tests/test_chymaera_agent_usage.py` | 155 | 8% | - |
| `tests/test_voice_audio_io.py` | 112 | 7% | - |
| `tests/test_chymaera_analysis.py` | 105 | 7% | - |
| `tests/test_voice_clone_transcribe_samples.py` | 263 | 6% | - |
| `tests/test_chymaera_firmware.py` | 207 | 4% | - |
| `tests/test_chymaera_limbo.py` | 224 | 4% | - |
