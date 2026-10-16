# omarchyvr: Claude Code entry point

Claude Code loads this file in every session. The binding public rules are in `ai/CLAUDE.md` (imported below);
`AGENTS.md` is the entry point for other agents and `ai/README.md` lists everything else an agent uses.

@ai/CLAUDE.md
@AGENTS.md

## Private context (never tracked)
- `.claude/` and `CLAUDE.local.md` are ignored by git: private lab notes, data and machine facts live there.
  The pre-commit and pre-push hooks (`tools/git-hooks/`, enabled with `git config core.hooksPath tools/git-hooks`)
  refuse any staged `.claude/` path and run the leak scanners (serials, keys, e-mail addresses, home paths,
  eye-chip bytes, the login name) on the staged diff and on the commits being pushed.
- Tests that replay private lab data run only when that data is present (`tools/lab-data.sh`); otherwise they are
  ignored with a reason.
