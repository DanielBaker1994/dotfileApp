---
name: diagram
description: Draw flowcharts, architecture, sequence and ER diagrams in markdown notes as ```dot (Graphviz) or ```d2 fences that render themed in the prose view and PDF. Use when asked to diagram, chart or visualize a flow, system, process or schema in a note or document. Prefer this over Mermaid.
---

# diagram — text diagrams that render themed

`diagrams.lua` (dotfiles `markdown_generator/`, wired by `[notes] pdf-filter` and
`EXTERNAL_BUILD_AND_OPEN_PDF`) turns ```dot / ```graphviz / ```d2 fences into
inline SVG, coloured from the document's style marker. **Never write colors,
fonts or sizes** — the style marker does that, and a hand-set color breaks theming.

## Pick the language
- **dot** (Graphviz): flowcharts, decision trees, dependency graphs, anything
  where the layout should be automatic. Default choice.
- **d2**: architecture (grouped boxes, databases), sequence diagrams
  (`shape: sequence_diagram`), ER diagrams (`shape: sql_table`).
- Not Mermaid: it is not rendered in the PDF.

## Rules that make AI output render first time
1. Describe structure, never layout: nodes, edges, groups. No coordinates, no styling.
2. Short ids, human labels: `auth [label="Check token"]`; quote every label in dot.
3. One idea per diagram, at most ~12 nodes; split big ones.
4. dot: `digraph G { rankdir=LR ... }`; group with `subgraph cluster_x { label="X"; ... }`;
   shapes: `oval` start/end, `diamond` decision, `cylinder` store, default box.
5. d2: `direction: right`; containers are `name: Label { child: ... }`; edges `a -> b: label`.
6. Label every decision edge (`yes` / `no`).

## Validate before you hand it over
```bash
dot -Tsvg file.dot > /dev/null        # syntax errors come back with a line number
d2 file.d2 /tmp/out.svg               # same for D2
```
A bad fence renders as a red "diagram failed" box with the same message and the
author's own line number: fix that line, don't rewrite the diagram.

## Style
`<div class="doc paper clean"></div>` on line 1 picks the palette (14 styles).
No marker = neutral colors. Snippets: `markdown_dot_flowchart`, `_dot_architecture`,
`_d2_flowchart`, `_d2_architecture`, `_d2_sequence`, `_d2_erd` (nvim `<leader>i`, category Diagrams).
Needs `brew install graphviz d2`. Renders are cached in `~/.cache/kitchen-sink/diagrams`.
