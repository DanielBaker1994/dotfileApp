# App bugs found by the automated testing

One row per real bug in an app feature (not MCP plumbing). "Verified" means Claude re-checked it.

| # | Feature | Bug | Evidence | Found by | Verified | Status |
|---|---|---|---|---|---|---|
| 1 | Files | `**` in the Files filter ("search every folder below", `commands.toml:477`) always shows nothing: the query parses as `QueryMode::Recursive` but the listing returns an empty `Vec` | `rust/ws-rs/src/views/files.rs:1593` `QueryMode::Recursive { .. } => Vec::new()`; no other implementation in the tree | flow table (static read), flow `files-recursive-search` | yes (code) | open |
