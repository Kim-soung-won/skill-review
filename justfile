# skillsmith — dev tasks + Claude Code integration.
# https://github.com/casey/just

home := justfile_directory()

# default: list recipes
default:
    @just --list

# release build
build:
    cargo build --release

# validate the Claude Code marketplace + plugin manifests
validate:
    claude plugin validate plugins/skillsmith

# list discovered projects (home defaults to ~/.skillsmith; demo auto-seeds)
list:
    cargo run --release --quiet -- list

# full optimize loop for a project — e.g. `just run demo`
run project:
    cargo run --release --quiet -- run --project "{{ project }}"

# one eval pass — e.g. `just eval demo`
eval project:
    cargo run --release --quiet -- eval --project "{{ project }}"

# seed the bundled demo into ~/.skillsmith (idempotent)
init:
    cargo run --release --quiet -- init

# install the binary on PATH (~/.cargo/bin) — zero-config: no env var, works from
# any cwd, demo auto-seeds. This is what the marketplace plugin's /skillsmith calls.
install-bin:
    cargo install --path .

# Install agent integrations: symlink this repo's integration files into each
# agent's config dir (edits here are live, version-controlled, no copy drift).
# Idempotent — re-run to re-sync. See plugins/README.md.
install: build install-claude install-codex install-gemini
    chmod +x "{{ home }}/bin/skillsmith"
    @echo ""
    @echo "✓ skillsmith integrations symlinked (edits in-repo are now live)"
    @echo "→ one-time: put the binary on PATH (zero-config, no env var):"
    @echo "    just install-bin        # cargo install --path . -> ~/.cargo/bin/skillsmith"
    @echo "  (or instead: export SKILLSMITH_HOME=\"{{ home }}\")"
    @echo "→ Codex: optionally hint ~/.codex/AGENTS.md (see plugins/README.md)"
    @echo "→ Gemini: run /commands reload inside the CLI to pick it up"
    @echo "→ No API key needed (default provider = installed claude CLI)."

# Claude Code: /skillsmith slash command + skill.
install-claude:
    mkdir -p ~/.claude/commands ~/.claude/skills/skillsmith
    ln -sf "{{ home }}/plugins/skillsmith/skills/skillsmith/SKILL.md" ~/.claude/skills/skillsmith/SKILL.md
    ln -sf "{{ home }}/plugins/skillsmith/commands/skillsmith.md" ~/.claude/commands/skillsmith.md
    @echo "✓ Claude Code: /skillsmith command + skill (or use the marketplace plugin)"

# Codex: user-level skill at ~/.agents/skills (prompts are deprecated in Codex).
install-codex:
    mkdir -p ~/.agents/skills/skillsmith
    ln -sf "{{ home }}/plugins/skillsmith/skills/skillsmith/SKILL.md" ~/.agents/skills/skillsmith/SKILL.md
    @echo "✓ Codex: skillsmith skill (~/.agents/skills/skillsmith)"

# Gemini CLI: custom command at ~/.gemini/commands (run /commands reload after).
install-gemini:
    mkdir -p ~/.gemini/commands
    ln -sf "{{ home }}/plugins/gemini/skillsmith.toml" ~/.gemini/commands/skillsmith.toml
    @echo "✓ Gemini CLI: /skillsmith command (~/.gemini/commands)"

# Remove all agent integration symlinks.
uninstall:
    rm -f ~/.claude/commands/skillsmith.md ~/.claude/skills/skillsmith/SKILL.md \
          ~/.agents/skills/skillsmith/SKILL.md ~/.gemini/commands/skillsmith.toml
    @echo "removed skillsmith integrations (claude, codex, gemini)"
