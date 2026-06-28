# skillsmith — Claude Code plugin

Ships the `/skillsmith` slash command + skill so you can drive the
[skillsmith](../../) eval-gated skill optimizer from inside Claude Code.

> **This plugin is the command surface, not the engine.** skillsmith's optimizer
> is a Rust binary. Install it once with `cargo install --path .` (puts `skillsmith`
> on PATH). No env var — home is a repo-local `.skillsmith/` if found above your cwd
> (like `.git`), else `~/.skillsmith` (the demo auto-seeds there).

## Install (marketplace)

```bash
# 1) build + install the engine on PATH (one time)
git clone <repo-url> skillsmith && cd skillsmith
cargo install --path .                    # -> ~/.cargo/bin/skillsmith

# 2) add this repo as a marketplace and install the plugin
#    Use the clone's ABSOLUTE path — the "Add Marketplace" prompt rejects a bare ".".
/plugin marketplace add /abs/path/to/skillsmith/plugins/skillsmith   # the plugin dir (or "./plugins/skillsmith" if cwd = repo)
/plugin install skillsmith@skillsmith
#   (you already have the clone from `cargo install` — point `add` at its plugins/skillsmith)
```

Then, from any project:

```
/skillsmith list
/skillsmith run --project demo
```

**Optimize your own repo — no hand-written config.** Tell Claude *"optimize this repo with
skillsmith"* and it scaffolds a repo-local project, picks a target with you (HITL), writes the
config + seed skill, and runs — full walkthrough in the
[root README](../../README.md#run-from-your-agent-cli).

## What it contains

```
plugins/skillsmith/                     # the Claude/Codex marketplace root is THIS dir
├── .claude-plugin/
│   ├── marketplace.json           # Claude marketplace manifest (git-subdir source when published)
│   └── plugin.json                # Claude plugin manifest
├── .agents/plugins/marketplace.json   # Codex marketplace manifest
├── .codex-plugin/plugin.json          # Codex plugin manifest (shares the skill)
├── commands/skillsmith.md             # the /skillsmith slash command
└── skills/skillsmith/SKILL.md         # the skill (model-invoked guidance; Claude + Codex)
```

The command + skill are the **single source of truth** — the legacy `just install`
symlink path (Codex / Gemini, and a no-marketplace Claude option) points at these
same files. See [`plugins/README.md`](../README.md) for all routes.

## Why the engine isn't bundled

A plugin is **copied to a cache** on install (`~/.claude/plugins/cache`), and a
marketplace can't compile Rust — so the engine isn't shipped here. The command just
calls `skillsmith` on your PATH (kept current by `cargo install`), and your
`projects/` + staged outputs live under `~/.skillsmith`, not the plugin cache — so
nothing the plugin caches goes stale.
