<div class="doc paper clean" data-foot="Confidential · Prepared by Your Team"></div>

# Document Title

One-line subtitle: what this is, for whom, and the period it covers

| | |
|---|---|
| **Prepared by** | Name, Team |
| **Date** | YYYY-MM-DD |
| **Version** | 1.0 — Draft |
| **Audience** | Leadership · Engineering |

> [!IMPORTANT]
> **Bottom line.** State the conclusion and the decision you need in two sentences. Readers who stop here should still know what to do.

## 1. Summary

Two or three short paragraphs: the situation, what changed, and the recommendation. Keep paragraphs under six lines — long ones are the first thing readers skip.

- **Outcome:** the result in one line
- **Impact:** who or what it affects
- **Ask:** the decision, owner and date

## 2. Background

Context a new reader needs. Link sources instead of pasting them: [source document](https://example.com).

## 3. Key Metrics

| Metric | Target | Actual | Status |
|---|---|---|---|
| Availability | 99.9% | 99.95% | On track |
| p95 latency | < 300 ms | 340 ms | At risk |
| Cost / month | $12k | $10.4k | Done |

## 4. Findings

### 4.1 First finding

State the finding, then the evidence. Lead with the claim; keep supporting detail below it.

```bash
# commands and snippets get a copy button on screen
./deploy --env staging --dry-run
```

> [!NOTE]
> Use notes for context that is useful but not required.

### 4.2 Second finding

> [!WARNING]
> Use warnings for risks the reader must not miss.


## 5. Recommendation

1. **Do this first** — why, owner, date
2. **Then this** — why, owner, date
3. **Finally this** — why, owner, date

## 6. Next Steps

| Action | Owner | Due |
|---|---|---|
| Confirm scope | Name | YYYY-MM-DD |
| Ship phase one | Name | YYYY-MM-DD |

## Appendix

Supporting detail, glossary, and raw data go here.

<!--
HOW THIS TEMPLATE WORKS (delete this comment)
- Line 1 = the style marker. Swap `paper` for any template name — the ◐ chip on
  the Prose | nvim switch does it for you: tokyo-night, paper, executive,
  terminal, catppuccin-mocha/latte, dracula, nord, gruvbox-dark/light,
  solarized-dark/light, rose-pine, rose-pine-dawn.
- `clean` = no icon glyphs, so text copied out of the PDF is plain text.
  Remove it to get the language / alert icons.
- data-foot = footer text (left). "Page n of N" is added on the right.
- `# Title` + the paragraph right under it = page-1 title block and the
  running header on pages 2+.
- Put <div class="pagebreak"></div> before a heading to force a new page; otherwise headings, tables,
  code blocks and alerts are kept whole across page breaks automatically.
- Alerts: > [!NOTE] [!TIP] [!IMPORTANT] [!WARNING] [!CAUTION]
-->
