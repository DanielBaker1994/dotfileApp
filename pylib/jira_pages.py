"""Jira page data shared by the board columns and the directory's status
options: the status category names.

The ticket/comment HTML templates that also lived here are gone - nothing on
the Rust side called `jira.ticket_html` / `jira.comments_html` any more, and
the board page is generated in `rust/ws-rs/src/views/jira.rs` (`board_page`).
"""
from __future__ import annotations

import jsonmgr

# lazy (worker surface): a broken jira/defaults.json fails at the first
# method call and is retried on the next one; the board/directory pickers
# read CATEGORY_NAMES
jsonmgr.lazy_module(__name__, globals(), {
    "CATEGORY_NAMES": ("jira/defaults", ("category_names",), list),
})
