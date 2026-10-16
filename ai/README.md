# ai/: everything an AI agent uses in this repository

This folder is for agents (Claude Code, Codex, any other) that work on omarchyvr: install it, diagnose a failure on a
user's machine, add support for a headset, GPU or compositor, and open a pull request. Humans can read it too.
User-facing documentation stays in [../docs/](../docs/).

Start with [../AGENTS.md](../AGENTS.md) (the entry point tools look for), then:

| File | What it is |
|---|---|
| [CLAUDE.md](CLAUDE.md) | The binding project rules (imported by the root `CLAUDE.md`, which Claude Code loads) |
| [AI-KNOWLEDGE.md](AI-KNOWLEDGE.md) | Reading order, system overview, hard rules, pitfalls, how to add hardware, how to diagnose a failure |
| [LESSONS.md](LESSONS.md) | The numbered pitfalls; read all of it before touching the stack |
| [VOICE-DECISIONS.md](VOICE-DECISIONS.md) | Why the voice loop is built the way it is |
| [test-plan-template.md](test-plan-template.md) | Template for a headset test plan that a user runs from the app |
| [scripts/](scripts/) | Helper scripts: `voice-test.sh`, `room-test.sh`, `guarded-audit.sh` (refuses to run, and aborts, while a VR session runs) |

`docs/AI-KNOWLEDGE.md`, `docs/LESSONS.md`, `docs/VOICE-DECISIONS.md` and the old script paths under `tools/` are
symbolic links to these files.
