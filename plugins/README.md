# skillsmith — plugins & agent integrations

Everything plugin-related lives under this `plugins/` directory (the repo root stays
clean). Every route drives the same `skillsmith` binary — install it once with
`cargo install --path .` (on PATH, no env var; home defaults to `~/.skillsmith`, demo
auto-seeds). Three install routes:

1. **Claude Code marketplace plugin** — `plugins/skillsmith/.claude-plugin/marketplace.json`
   + `plugins/skillsmith/.claude-plugin/plugin.json`.
2. **Codex marketplace plugin** — `plugins/skillsmith/.agents/plugins/marketplace.json`
   + `plugins/skillsmith/.codex-plugin/plugin.json`.
3. **Symlink install** (`just install`) — symlinks the command/skill into each
   agent's config dir. The only route for **Gemini** (no marketplace), and a
   no-marketplace fallback for Claude / Codex. Edits are live.

The command + skill are defined **once** under `plugins/skillsmith/` and reused by
all of them — no duplication, no copy drift. (Mirrors SkillOpt's all-under-`plugins/`
layout, but kept single-source instead of duplicating the SKILL.md per agent.)

**Project layout & interactive authoring.** A project lives either repo-local in a
`<repo>/.skillsmith/` (auto-discovered like `.git`; bare `skillsmith new` inside a repo
is repo-local + auto-named) or centrally under `~/.skillsmith`. When driven from an agent, you don't hand-write
`config.toml`: tell the agent *"optimize this repo with skillsmith"* and — because the
host agent can read your repo and ask you, while skillsmith's internal judge agent is
blind — it scans for a function + test, **confirms the target with you (HITL)**, and
generates the project (answer withheld from the intent), then `--dry-run`s (no tokens)
and runs. The SKILL's "Author a project (interactive)" steps are the source of truth.

| Agent | Mechanism | Install | Invoke |
|-------|-----------|---------|--------|
| **Claude Code** | marketplace plugin **or** symlink | `/plugin marketplace add <abs-path>/plugins/skillsmith` → `/plugin install skillsmith@skillsmith` | `/skillsmith run --project demo` |
| **Codex** | marketplace plugin **or** symlinked skill | `codex plugin marketplace add ./plugins/skillsmith` → `codex /plugins` (Install) | `@skillsmith` (or describe the task) |
| **Gemini CLI** | custom command (`.toml`) | `just install-gemini` → `/commands reload` | `/skillsmith run --project demo` |

**No API key needed** (default provider = installed `claude` CLI; see the root
README "Providers").

## Install

```bash
# one time: put the engine on PATH (every route below relies on it)
cargo install --path .                # -> ~/.cargo/bin/skillsmith  (or: just install-bin)

# Claude Code marketplace — point at the plugin dir (ABSOLUTE path; the "Add Marketplace"
# prompt rejects a bare "."):
/plugin marketplace add /abs/path/to/skillsmith/plugins/skillsmith   # the plugin dir in your clone
/plugin install skillsmith@skillsmith

# Codex marketplace (point at the plugin dir):
codex plugin marketplace add ./plugins/skillsmith   # finds .agents/plugins/marketplace.json there
codex /plugins                                       # open the list → Install "skillsmith"

# Or symlink (needed for Gemini; a fallback for Claude / Codex):
just install            # every available agent
just install-claude     # or one at a time
just install-codex
just install-gemini
just uninstall          # remove all symlinks
```

## Files (single source of truth)

- `plugins/skillsmith/skills/skillsmith/SKILL.md` — the skill. **Claude + Codex
  share it** (both use the `name` + `description` SKILL.md format).
- `plugins/skillsmith/commands/skillsmith.md` — the Claude Code slash command.
- `plugins/skillsmith/.claude-plugin/marketplace.json` + `plugin.json` — Claude marketplace
  manifests (the marketplace root is `plugins/skillsmith/`; published install uses a
  `git-subdir` source pointing at `plugins/skillsmith`).
- `plugins/skillsmith/.agents/plugins/marketplace.json` + `.codex-plugin/plugin.json` —
  Codex marketplace manifests.
- `plugins/gemini/skillsmith.toml` — the Gemini CLI custom command (its own format).

## Notes

- **Install is path-based** (no `add owner/repo` by-name): the marketplace manifest lives
  under `plugins/skillsmith/`, not the repo root, so point `add` at that dir. You already
  have the clone from `cargo install`, so it's no extra step. (A root manifest would enable
  by-name install, but is omitted to keep the repo root clean.)
- **Engine on PATH**: every route calls `skillsmith` (from `cargo install`). A
  marketplace copies only the command/skill markdown into a cache; the engine +
  your `projects/` live under `~/.skillsmith`, so nothing cached goes stale. Refresh
  the markdown with `/plugin marketplace update` (Claude) or
  `codex plugin marketplace upgrade` (Codex) after editing it.
- **Codex**: the marketplace lives at `plugins/skillsmith/.agents/plugins/marketplace.json`
  (Codex also reads `.claude-plugin/marketplace.json` as legacy). The symlink fallback puts
  the skill at `~/.agents/skills/skillsmith/`; custom prompts are deprecated.
- **Gemini**: after install, run `/commands reload` (or restart) to pick it up.
