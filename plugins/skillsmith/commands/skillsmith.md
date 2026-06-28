---
description: Run skillsmith — eval-gated skill optimizer (optimize an agent skill against a repo's real tests)
argument-hint: "[run --project <name> [--dry-run|--watch] | eval --project <name> | check --project <name> | bench --project <name> [--seeds N] | adopt --project <name> | deploy --project <name> | list | new [name] | init]  (default: list; or describe a repo in plain language to author + optimize)"
allowed-tools: Bash, Read, Grep, Glob, Write, Edit, AskUserQuestion
---

# /skillsmith

Drive **skillsmith** (eval-gated skill optimizer). The binary is the single
source of truth — do NOT hardcode or mirror its flags here. If unsure of an
option, run it with `--help`.

## Requested action: $ARGUMENTS

(If `$ARGUMENTS` is empty, treat it as `list`.)

## How to run

Run `skillsmith` (installed on PATH via `cargo install --path .`). No env var
needed — home defaults to `~/.skillsmith` and the demo auto-seeds on first run:

```bash
if command -v skillsmith >/dev/null 2>&1; then
  skillsmith $ARGUMENTS
else
  "${SKILLSMITH_HOME:?install: run 'cargo install --path .' in the skillsmith repo (puts skillsmith on PATH), OR set SKILLSMITH_HOME}/bin/skillsmith" $ARGUMENTS
fi
```

| action                  | what it does |
|-------------------------|--------------|
| `list`                  | show discovered projects (`projects/<name>/config.toml`) |
| `eval --project <name>` (`--watch` = cheap re-eval loop) | one eval pass under the current skill — no changes |
| `run --project <name>` (`--watch` = re-run on a skill/config/target edit) | full loop; **stages** `skill.staged.md` + `report.md` + machine-readable `results.json` (nothing live changes) |
| `run --project <name> --dry-run` | preflight: validate config + run each `verify_cmd`, **no LLM / no tokens** |
| `check --project <name>` | token-0 **drift check**: did the repo inputs change since the last `run` (skill maybe stale)? exits non-zero on drift — never re-runs / adopts |
| `bench --project <name> [--seeds N]` | run the optimization N× and write a variance scorecard (mean ± stddev) + `sweep.jsonl` under `bench/` — **spends N× a run's tokens** |
| `adopt --project <name>` | copy `skill.staged.md` over the live skill (the **only** live-changing step) |
| `deploy --project <name>` | place the **adopted** skill where an agent reads it — `--as skill` (`.claude/skills/<name>/SKILL.md`) or `--as context` (inject into `CLAUDE.md`/`AGENTS.md`/`GEMINI.md`). Pure file ops, **no tokens** |
| `new` | scaffold a **repo-local** `./.skillsmith/projects/<name>/` — bare inside a git repo (auto-named, auto-local); `new <name>` for a custom name |
| `init` | seed the bundled **demo** project into `<home>` (idempotent) |

## If the project doesn't exist yet → author it interactively

If the user wants to optimize a repo but `skillsmith list` has no such project (or
they say "optimize this repo / set up skillsmith here"), **do not make them hand-write
`config.toml`.** Author it for them — you can read the repo and ask them; skillsmith's
internal judge agent is blind. Follow the **"Author a project (interactive)"** steps in
the skillsmith SKILL:

1. `skillsmith new` from the repo (bare: repo-local, auto-named, `repo_path` auto-defaults).
2. Read the repo (Grep/Glob/Read) and **rank candidate skill *purposes* by measurability ×
   generalization — NOT test count.** Score generalization from concrete signals: **call-site count**
   (grep the symbol across `src/`) and **import weight** (stdlib-only/pure = portable convention;
   imports servers/stores/clients = integration glue → low transfer). Disqualify glue by its *shape*,
   not the directory name — a small pure module under `orchestrator/` is a fine target; a 400-line
   server file isn't. Prefer a small single-purpose function whose convention generalizes and has (or
   can get via a fixture) a focused held-out test.
3. **Present the top 3 and let the user choose (AskUserQuestion)** — purpose + target + a
   measurability/generalization read (derived from those signals) for each; never silently pick the
   test-densest one.
4. `Write` `config.toml` + seed `skill.md` — intent says WHAT but **withholds the answer**;
   the test file is **held out** (never a `context_file`).
5. `skillsmith run --project <name> --dry-run` to validate (no tokens), then `run`.

## Steps to follow (running an existing project)

1. Run the requested action via the launcher above; capture stdout.
2. **For `run`:** `Read` the `report.md` at the path skillsmith prints (under `<home>/projects/<name>/`, default `~/.skillsmith`), and show the user:
   - baseline → best score, and the **lift**
   - the gate decision (accept/reject) per round
   - where the improved skill is staged
3. **If `run` produced an improvement (best > baseline):** show the `skill.staged.md` vs live `skill.md` **diff**, then **ask the user to adopt (HITL — AskUserQuestion / explicit yes)**. On **yes**, run `skillsmith adopt --project <name>` (copies the staged proposal over the live skill and clears it). On **no**, leave it staged. If best == baseline, say there's nothing to adopt.
4. **After adopt → offer to deploy (HITL):** the adopted `skill.md` is project-local and **no agent auto-reads it**. Ask the user to deploy. On **yes**, author a good `description` (real trigger phrases — what the user would say) and run `skillsmith deploy --project <name> --desc "<phrases>"` — default `--as skill` writes `.claude/skills/<name>/SKILL.md` (Claude auto-loads it); use `--as context --agents claude,codex,gemini` to promote a short rule into `CLAUDE.md`/`AGENTS.md`/`GEMINI.md` instead. Pure file ops, **no tokens**. On **no**, stop at adopt.
5. **Never** edit the live skill yourself, and never run `skillsmith adopt`/`deploy` without that explicit confirm — adoption and deployment are the user's call.

## Notes

- **No API key needed** by default — skillsmith uses your installed agent CLI's auth (the default `provider = "claude"` shells out to `claude -p`). The `genai` provider is the only one that needs `ANTHROPIC_API_KEY`. Tokens are spent either way.
- Unknown flags / new subcommands → run `skillsmith --help` rather than guessing. The binary is authoritative.
