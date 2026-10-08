---
name: doc-template
description: Start a polished, shareable document from the reusable professional markdown template (style marker, title block, summary callout, metrics table, findings, recommendation, next steps) and export it to PDF. Use when asked to write a report, proposal, brief, review or any document meant to be shared as a PDF.
---

# doc-template — a shareable document in one marker line

1. Copy `templates/professional-report.md` (repo root) to the new note's path.
2. Pick the style: set the marker on line 1, `<div class="doc NAME"></div>`.
   Names: tokyo-night, paper (default for sharing), executive, terminal,
   catppuccin-mocha, catppuccin-latte, dracula, nord, gruvbox-dark,
   gruvbox-light, solarized-dark, solarized-light, rose-pine, rose-pine-dawn.
   Keep `clean` (no icon glyphs: copied text is plain). `data-foot` = footer.
3. Fill the sections. Conventions the CSS relies on:
   - `# Title` then ONE paragraph directly under it = subtitle (also the
     running page header). One `#` per document.
   - Alerts only as `> [!NOTE|TIP|IMPORTANT|WARNING|CAUTION]`.
   - Link sources instead of pasting; short paragraphs; tables for numbers.
   - `<div class="pagebreak"></div>` only for a deliberate new page.
   - Developer callouts: `<div class="callout KIND">` + blank line + markdown +
     blank line + `</div>`; KIND = decision risk breaking deprecated action
     example question rollback perf (the label is drawn by CSS). Badges:
     `<span class="badge ok|warn|bad|info|muted|accent">text</span>`.
   - Skeletons: snippets `markdown_doc_adr|postmortem|runbook|pr|status` (nvim `<leader>i`).
4. Export: in the app, Prose view ▸ ⌘P (PDF lands in `[notes] pdf-path`), or
   from a shell:
   ```bash
   pandoc -s -f gfm -t html5 --syntax-highlighting=tango -V lang=en \
     --metadata pagetitle="Title" \
     --include-in-header=$HOME/.dotfiles/markdown_generator/friendly_document_styling.css \
     -o /tmp/doc.html note.md && weasyprint --pdf-tags /tmp/doc.html note.pdf
   ```
5. Check the PDF: title block on page 1, running header + "Page n of N" after,
   no heading stranded at a page bottom, no table or code block split.

The CSS lives only in `~/.dotfiles/markdown_generator/friendly_document_styling.css`
(shared by the app and `EXTERNAL_BUILD_AND_OPEN_PDF`). A new style = copy one
`:root:has(.name) { … }` palette block there and add the name to
`[notes] doc-templates` in `commands.toml`.
