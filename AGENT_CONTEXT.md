# kitchen-sink — agent context moved to AGENTS.md

**The current, authoritative agent-context document is `AGENTS.md` at the repo
root. Read that first.** `CLAUDE.md` is a symlink to this file, so this
pointer exists only to send Claude/agent readers to `AGENTS.md`.

This file previously held the full ~120 KB context doc. Its deep-detail
sections (screenshot/compare/jira/prose behavior, theme notes, historical
change logs) had grown large and partly stale, so the agent context was
rewritten as `AGENTS.md` (Oct 2026). The previous revision is preserved in git
history:

```bash
git log -- AGENT_CONTEXT.md          # commits that touched this file
git show <commit>:AGENT_CONTEXT.md   # the old full document
```

## Where the current detail lives

- `AGENTS.md` — the single current context doc: rules, build/run/test, code
  map, architecture, the Rust port, common tasks, gotchas.
- `rule.md` — the non-negotiable UX/git/SwiftTerm rules.
- `BACKLOG.md`, `PRD-*.md`, `PRODUCT.md`, `DESIGN.md`, `README.md` — deep
  dives for the parts they cover.
