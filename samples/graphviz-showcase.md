<div class="doc paper clean"></div>

# Graphviz showcase

Themed `dot` fences: **no colors, fonts or sizes** — the style marker on line 1
does that. For a fully styled standalone version (colors, HTML tables, records,
ports, clusters, legend) see `graphviz-showcase.dot` next to this file.

Render the standalone file anywhere:

```bash
dot -Tsvg graphviz-showcase.dot -o graphviz-showcase.svg
dot -Tpng -Gdpi=150 graphviz-showcase.dot -o graphviz-showcase.png
```

## 1. Decision flow with labelled edges

```dot
digraph G {
  rankdir=TB
  start  [label="Request", shape=oval]
  token  [label="Token valid?", shape=diamond]
  rate   [label="Under rate limit?", shape=diamond]
  serve  [label="Serve from cache"]
  origin [label="Fetch from origin"]
  reject [label="401 / 429", shape=oval]
  done   [label="Response", shape=oval]
  start -> token
  token -> rate   [label="yes"]
  token -> reject [label="no"]
  rate  -> serve  [label="yes"]
  rate  -> reject [label="no"]
  serve -> origin [label="miss", style=dashed]
  serve -> done   [label="hit"]
  origin -> done
}
```

## 2. Architecture with clusters

```dot
digraph G {
  rankdir=LR
  compound=true
  user [label="Browser", shape=oval]
  subgraph cluster_web {
    label="Web tier"
    lb  [label="Load balancer"]
    app1 [label="App 1"]
    app2 [label="App 2"]
    lb -> app1
    lb -> app2
  }
  subgraph cluster_data {
    label="Data tier"
    db    [label="Postgres", shape=cylinder]
    cache [label="Redis", shape=cylinder]
  }
  user -> lb [label="HTTPS"]
  app1 -> cache [label="read"]
  app2 -> cache
  app1 -> db [label="write", lhead=cluster_data]
}
```

## 3. Records and ports (data model)

```dot
digraph G {
  rankdir=LR
  node [shape=record]
  orders    [label="{orders|<id>id|<cust>customer_id|total|status}"]
  customers [label="{customers|<id>id|name|email}"]
  items     [label="{order_items|<oid>order_id|sku|qty}"]
  orders:cust -> customers:id [label="N:1", arrowhead=crowodot]
  items:oid   -> orders:id    [label="N:1", arrowhead=crowodot]
}
```

## 4. Pipeline with same-rank stages and a back edge

```dot
digraph G {
  rankdir=LR
  commit [label="Commit", shape=oval]
  build  [label="Build"]
  { rank=same; unit [label="Unit tests"]; lint [label="Lint"]; }
  stage  [label="Deploy: staging"]
  smoke  [label="Smoke test", shape=diamond]
  prod   [label="Deploy: prod", shape=oval]
  commit -> build
  build -> unit
  build -> lint
  unit -> stage
  lint -> stage
  stage -> smoke
  smoke -> prod  [label="pass"]
  smoke -> build [label="fail: rollback", style=dashed, constraint=false]
}
```

## 5. Dependency graph (undirected + HTML label)

```dot
graph G {
  layout=neato
  overlap=false
  a [label=<<B>core</B>>]
  b [label="net"]
  c [label="ui"]
  d [label="storage"]
  e [label="auth"]
  a -- b
  a -- d
  b -- e
  c -- a
  c -- e [style=dashed]
}
```

## Cheat sheet

| Feature | Syntax |
|---|---|
| Direction | `rankdir=LR` (also TB, BT, RL) |
| Group | `subgraph cluster_x { label="X"; ... }` |
| Same row/column | `{ rank=same; a; b; }` |
| Edge between clusters | `compound=true` + `lhead=cluster_x` / `ltail=` |
| Ignore edge in layout | `constraint=false` |
| Port on record/HTML | `a:port -> b:port` |
| Invisible spacer | `a -> b [style=invis]` |
| Other engines | `layout=neato / fdp / circo / twopi` |
