---
name: skillsmith
description: "Use when the user wants to optimize or improve a coding agent's skill or system prompt for a project, and measure whether a better prompt actually helps — eval-gated skill/prompt optimization that grades by executing the repo's own tests in isolated git worktrees. Triggers: 'optimize my skill', 'improve the agent skill/prompt for <project>', 'measure if a better prompt helps', 'tune the skill against tests', 'run skillsmith', 'eval-gated skill optimization'. Drives the skillsmith binary: propose skill edits -> eval (execute tests) -> gate on measured lift -> stage; nothing live changes until the user adopts."
---

# skillsmith — eval-gated skill optimizer

skillsmith improves a coding agent's **skill** (a markdown instruction sheet)
for a target repo, keeping only edits that *measurably* raise success — graded
by an **execution judge** that applies the agent's code changes in an isolated
`git worktree` and runs the repo's own tests (exit 0 = pass). The oracle is the
target repo's own suite — your tests, run for real, not a benchmark scorer or a
transcript match.

(Shared by the Claude Code and Codex integrations — both use the
`name` + `description` SKILL.md format.)

## When to use

- "optimize / improve the skill (or system prompt) for `<project>`"
- "does a better prompt actually help — measure the lift"
- "run the skill optimizer" / "run skillsmith"

## How to drive it

The binary is the single source of truth (do not mirror its flags). Run
`skillsmith` (on PATH via `cargo install --path .`; no env var — home defaults to
`~/.skillsmith`, demo auto-seeds):

```bash
skillsmith list
skillsmith eval   --project <name>   # one eval pass (--watch = cheap re-eval loop on input changes)
skillsmith run    --project <name>   # full optimize loop (--watch = re-run on a skill/config/target edit)
skillsmith check  --project <name>   # token-0 drift: did repo inputs change since the last run? (no LLM) — detect-only, never re-runs/adopts
skillsmith bench  --project <name>   # k-seed variance scorecard (mean ± stddev); spends N× tokens — measurement, not adoption
skillsmith adopt  --project <name>   # copy skill.staged.md over the live skill (the ONE live-changing step)
skillsmith deploy --project <name>   # place the adopted skill where an agent reads it (.claude/skills or CLAUDE.md) — no tokens
```

- `run` stages `skill.staged.md` + `report.md` + machine-readable `results.json` **incrementally** (after each accepted round, so a later transient `claude` error can't discard an improvement already won); **nothing live changes** until adoption.
- Always surface the **baseline → best (lift)** and the gate decision before suggesting adoption.
- **Adoption is a separate, explicit step** — `skillsmith adopt` is the only thing that overwrites the live skill. Drive it as a HITL confirm (see step 7), never silently.
- **`deploy` is the last mile** — after adopt, the skill is still project-local and no agent auto-reads it. `skillsmith deploy` (pure file ops, no tokens) wraps it as a `.claude/skills` SKILL.md or injects it into a context file. Drive it as a HITL confirm too (see step 8).
- No API key needed by default (uses your installed `claude` CLI auth); only the `genai` provider needs `ANTHROPIC_API_KEY`. New flags → check `--help`, don't guess.

## Author a project (interactive — when one doesn't exist)

If `skillsmith list` doesn't show the project the user wants, **don't make them
hand-write `config.toml`** — author it for them, with them in the loop. This is
*your* job precisely because **you can read the repo and ask the user; skillsmith's
internal judge agent is blind** (it gets only the skill + intent, no repo access). So
you write a good config; the binary runs the blind gate against it.

1. **Scaffold repo-local** from inside the target repo — bare `skillsmith new` is enough
   (no name/`--local` needed inside a git repo): it makes `<repo>/.skillsmith/projects/<name>/`
   auto-named after the repo, `repo_path` auto-defaults, `skill.staged.md`/`report.md` gitignored.
   (Pass a name only if you want a custom one: `skillsmith new <name>`.)
2. **Recommend the skill *purpose*, ranked — don't just grab the most-tested function.**
   The target isn't merely "a function with a test"; it's *what skill is worth teaching the
   agent in this repo*. Scan with Grep/Glob/Read and rank candidates on **two axes, each scored
   from concrete signals (not vibes)**:
   - **Measurability** — can a held-out oracle gate it? A focused, file-scoped test pinning exact
     behavior (pure helpers, converters, validators, parsers). No test yet but green/stable → a
     **fixture** can supply one.
   - **Generalization value** — would mastering this skill *transfer*? Score it from:
     - **Call-site count**: grep the function/symbol/pattern across `src/` — many callers = a
       recurring convention (a docstring that says *SSOT / reused / "the common case"* self-declares it).
     - **Import weight of the target *file***: stdlib-only / pure = a portable convention worth
       teaching; a file that imports **servers, stores, clients, or wires subsystems together =
       integration glue** → low transfer.

   Rank by **measurability × generalization × your confidence — NOT test count.** ⚠️ The trap: the
   most-tested subsystem scores high on measurability but is often glue. **Disqualify integration
   glue by its *shape* — files that import servers/stores/clients or wire subsystems — regardless of
   the directory name.** (A small, pure, convention-bearing module that happens to live under an
   `orchestrator/`-style package is still an excellent target; a 400-line server file is not,
   wherever it lives.) Keep targets **small + single-purpose** (the judge rewrites the *whole* target
   file → big files = collateral damage). **Green tree** → build a **fixture**: a separate committed
   git repo that stubs the function (`raise NotImplementedError`) + holds the **real** test as a
   held-out oracle. Encode the *real* convention, not a synthetic one, so the skill stays portable.
   Point `repo_path` at the fixture.
3. **Present the top 3 and let the user choose (HITL).** For each candidate show: the **purpose**
   (what the skill teaches), the **target function + its test**, and a **measurability /
   generalization read** — high/med/low **derived from the signals above** (call-site count, import
   weight, a convention-declaring docstring), each with one line of why — plus *what the intent will
   withhold*. Ask which to optimize, or let them name their own / adjust scope. **Never silently pick
   the #1**, and never default to the test-densest subsystem.
4. **Generate `config.toml` + seed `skill.md`** (Write), following the hard rules:
   - `intent`: say WHAT to implement, but **withhold the answer** — no formulas, exact
     strings, or magic constants. Those are what the optimizer must *discover*; put the
     general convention (not the answer) in the seed `skill.md`.
   - `verify_cmd`: run **only that test**, file-scoped (fast; some suites hang on a
     full run). `target_files`: the function's file. `context_files`: usually `[]`.
   - **Splits** (≥2 tasks): `holdout = true` (alias `split = "val"`) = the gate's held-out
     validation; `split = "test"` holds a task out of optimization entirely, scored **once** at
     the end for an unbiased number. Default `train` = the optimizer learns from its failures.
   - **Cost knob (optional)**: to run the cheap agent stage on a smaller model, set
     `agent_provider_cmd` (e.g. `["claude","-p","--model","claude-haiku-4-5"]`); `genai` tiers by
     `agent_model`. Unset = base `provider`.
5. **Preflight free:** `skillsmith run --project <name> --dry-run` (no LLM/tokens) —
   fix any config/`verify_cmd` error before spending agent calls.
6. **Run:** `skillsmith run --project <name>`; surface baseline→best + the gate decision.
7. **Adopt (HITL) — required after an improving run:** if `best > baseline`, show the
   `skill.staged.md` vs live `skill.md` diff, then **ask the user to adopt** (AskUserQuestion
   or an explicit yes/no). On **yes**, run `skillsmith adopt --project <name>` (copies the
   staged proposal over the live skill and clears it). On **no**, leave it staged. Never copy
   it over without that confirm. If `best == baseline`, there's nothing to adopt — say so.
8. **Deploy (HITL) — make the adopted skill actually usable:** the adopted `skill.md` is
   project-local and **no agent auto-reads it**. After adopt, offer to deploy. On **yes**,
   author a real `description` (trigger phrases — what the user would *say* to invoke it) and
   run `skillsmith deploy --project <name> --desc "<phrases>"`. Default `--as skill` writes
   `.claude/skills/<name>/SKILL.md` (Claude auto-loads on match); `--as context --agents
   claude,codex,gemini` injects a short rule into `CLAUDE.md`/`AGENTS.md`/`GEMINI.md` instead
   (the only path all three read automatically). Pure file ops, **no tokens**. Pick **context**
   for a short always-on rule, **skill** for a longer task-specific playbook. On **no**, stop
   at adopt.

## Hard rules

- Never hand-edit the live skill file. Adoption goes through `skillsmith adopt` (the binary
  copies `skill.staged.md` over the live skill), and only after an explicit user confirm.
- Test files are held out from the agent — **never** add a task's test file to its
  `context_files`, and never leak the answer (formula/exact strings) into `intent`.
  (skillsmith prints a `warning:` if a `verify_cmd`'s test is also a `context_file`,
  but you own correctness — heed it.)
- When authoring, confirm the target + scope with the user before writing files; prefer
  `new --local` so the project is committed *with* the repo.
- `skillsmith deploy` is the only command that writes **outside** the project dir (into the
  repo's `.claude/skills/` or a context file); run it only after an explicit deploy confirm.
  The `description` trigger phrases are yours to author well — a placeholder won't auto-trigger.
- Extending the CLI is a skillsmith repo change, not something to fake here.
