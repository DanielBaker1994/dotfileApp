from __future__ import annotations

import importlib
import threading

from . import HelperError, method

# ONE bad module/JSON must not kill the worker: import every method group
# through this guard. A failed import becomes an _Unavailable placeholder -
# its methods answer with a clear HelperError while ping and every other
# group keep working.
_MODULES = {
    "ai_format": "ai_format",
    "confluence_pages": "confluence_pages",
    "config_text": "config_text",
    "file_ops": "fileops",
    "ignore_rules": "ignore",
    "jira_boards": "jira_boards",
    "jira_directory": "jira_directory",
    "jira_fields": "jira_fields",
    "jira_search": "jira_search",
    "jira_setup": "jira_setup",
    "jsonmgr": "jsonmgr",
    "shelf": "shelf",
    "status": "ws_status",
}


class _Unavailable:
    def __init__(self, name, error):
        self._name = name
        self._error = error

    def __getattr__(self, attr):
        raise HelperError("helper module %s is unavailable (%s: %s)" %
                          (self._name, type(self._error).__name__, self._error))


for _mod, _alias in _MODULES.items():
    try:
        globals()[_alias] = importlib.import_module(_mod)
    except Exception as _e:  # noqa: BLE001 - deliberately broad: keep the worker alive
        globals()[_alias] = _Unavailable(_mod, _e)

_IGNORE_RULES = {}
_IGNORE_NEXT = [1]

# Requests run concurrently now; handle allocation must not race.
_HANDLE_LOCK = threading.Lock()


@method("ai.tables_as_text")
def _ai_tables_as_text(params: dict) -> dict:
    return {"text": ai_format.tables_as_text(params.get("md") or "")}


@method("ai.markdown")
def _ai_markdown(params: dict) -> dict:
    return {"text": ai_format.markdown_for(params.get("md") or "", params.get("target") or "outlook")}


@method("ai.pandoc_html")
def _ai_pandoc_html(params: dict) -> dict:
    return {"html": ai_format.pandoc_html(params.get("md") or "", bool(params.get("highlight")),
                                          params.get("pandoc") or "")}


@method("ai.styled")
def _ai_styled(params: dict) -> dict:
    return {"html": ai_format.styled(params.get("fragment") or "", params.get("target") or "outlook")}


@method("ai.html")
def _ai_html(params: dict) -> dict:
    return {"html": ai_format.html(params.get("md") or "", params.get("target") or "outlook",
                                   params.get("pandoc") or "")}


@method("ai.code_guard")
def _ai_code_guard(params: dict) -> dict:
    return ai_format.code_guard(params.get("text") or "")


@method("ai.code_restore")
def _ai_code_restore(params: dict) -> dict:
    return ai_format.code_restore(params.get("codes") or [], params.get("s") or "")


@method("ai.estimate")
def _ai_estimate(params: dict) -> dict:
    return {"tokens": ai_format.estimate(params.get("s") or "")}


@method("ai.part_budget")
def _ai_part_budget(params: dict) -> dict:
    return {"budget": ai_format.part_budget(params.get("instructions") or "",
                                            int(params.get("context") or 4096))}


@method("ai.parts")
def _ai_parts(params: dict) -> dict:
    return {"parts": ai_format.parts(params.get("s") or "", int(params.get("budget") or 200))}


@method("ai.reflow")
def _ai_reflow(params: dict) -> dict:
    return {"text": ai_format.reflow_instructions(params.get("s") or "")}


@method("ai.unwrap_fence")
def _ai_unwrap_fence(params: dict) -> dict:
    return {"text": ai_format.unwrap_fence(params.get("s") or "")}


@method("ai.rule_load")
def _ai_rule_load(params: dict) -> dict:
    return {"rule": ai_format.rule_load(params.get("path") or "")}


@method("ai.rule_chain")
def _ai_rule_chain(params: dict) -> dict:
    return {"rules": ai_format.rule_chain(params.get("path") or "")}


@method("ai.rule_instructions")
def _ai_rule_instructions(params: dict) -> dict:
    return {"text": ai_format.rule_instructions(params.get("rule") or {},
                                                bool(params.get("guarded")))}


@method("ai.rule_wrap")
def _ai_rule_wrap(params: dict) -> dict:
    return {"text": ai_format.rule_wrap(params.get("rule") or {}, params.get("text") or "")}


@method("ai.rule_prepare")
def _ai_rule_prepare(params: dict) -> dict:
    return {"text": ai_format.rule_prepare(params.get("rule") or {}, params.get("text") or "")}


@method("ai.rule_accept")
def _ai_rule_accept(params: dict) -> dict:
    return ai_format.rule_accept(params.get("rule") or {}, params.get("input") or "",
                                 params.get("answer") or "")


@method("ai.rule_arguments")
def _ai_rule_arguments(params: dict) -> dict:
    return {"args": ai_format.rule_arguments(params.get("rule") or {}, bool(params.get("guarded")))}


@method("ai.rule_preview")
def _ai_rule_preview(params: dict) -> dict:
    return {"text": ai_format.rule_preview(params.get("rule") or {})}


@method("ai.rule_runnable")
def _ai_rule_runnable(params: dict) -> dict:
    return {"text": ai_format.rule_runnable(params.get("rule") or {}, params.get("input") or "")}


@method("ai.csv_cells")
def _ai_csv_cells(params: dict) -> dict:
    return {"cells": ai_format.csv_cells(params.get("line") or "")}


@method("ai.csv_convert")
def _ai_csv_convert(params: dict) -> dict:
    return {"text": ai_format.csv_convert(params.get("s") or "")}


@method("ai.words")
def _ai_words(params: dict) -> dict:
    return {"words": ai_format.word_words(params.get("s") or "")}


@method("ai.word_check")
def _ai_word_check(params: dict) -> dict:
    return ai_format.word_check(params.get("input") or "", params.get("output") or "")


@method("fileops.stack_new")
def _fileops_stack_new(params: dict) -> dict:
    return {"handle": fileops.stack_new(int(params.get("limit") or 50))}


@method("fileops.stack_drop")
def _fileops_stack_drop(params: dict) -> dict:
    fileops.stack_drop(params.get("handle"))
    return {}


@method("fileops.stack_state")
def _fileops_stack_state(params: dict) -> dict:
    return fileops.stack_state(params.get("handle"))


@method("fileops.stack_forget")
def _fileops_stack_forget(params: dict) -> dict:
    fileops.stack_forget(params.get("handle"))
    return {}


@method("fileops.stack_collapse")
def _fileops_stack_collapse(params: dict) -> dict:
    fileops.stack_collapse(params.get("handle"), int(params.get("since") or 0),
                          params.get("what") or "")
    return {}


@method("fileops.transfer")
def _fileops_transfer(params: dict) -> dict:
    return fileops.transfer(params.get("paths") or [], params.get("into") or "",
                            bool(params.get("move")), params.get("stack"))


@method("fileops.duplicate")
def _fileops_duplicate(params: dict) -> dict:
    return fileops.duplicate(params.get("paths") or [], params.get("stack"))


@method("fileops.trash")
def _fileops_trash(params: dict) -> dict:
    return fileops.trash(params.get("paths") or [], params.get("stack"))


@method("fileops.create")
def _fileops_create(params: dict) -> dict:
    return fileops.create(params.get("name") or "", params.get("dir") or "",
                          bool(params.get("folder")), params.get("stack"))


@method("fileops.record_rename")
def _fileops_record_rename(params: dict) -> dict:
    fileops.record_rename(params.get("from") or "", params.get("to") or "",
                          params.get("stack"))
    return {}


@method("fileops.place")
def _fileops_place(params: dict) -> dict:
    return fileops.place(params.get("items") or [], bool(params.get("move")),
                         params.get("clash") or "replace", params.get("stack"))


@method("fileops.undo")
def _fileops_undo(params: dict) -> dict:
    result = fileops.undo(params.get("stack"))
    return result if result is not None else {}


@method("fileops.redo")
def _fileops_redo(params: dict) -> dict:
    result = fileops.redo(params.get("stack"))
    return result if result is not None else {}


@method("status.gather")
def _status_gather(params: dict) -> dict:
    delay = params.get("delay")
    return ws_status.gather(params.get("notify_script") or "",
                            float(delay) if delay is not None else 0.3)


@method("ignore.new")
def _ignore_new(params: dict) -> dict:
    rules = ignore.Rules(home=params.get("home") or None,
                         shelf_file=params.get("shelfFile"),
                         git_excludes=params.get("gitExcludes"))
    with _HANDLE_LOCK:
        handle = _IGNORE_NEXT[0]
        _IGNORE_NEXT[0] += 1
    _IGNORE_RULES[handle] = rules
    return {"handle": handle}


@method("ignore.drop")
def _ignore_drop(params: dict) -> dict:
    _IGNORE_RULES.pop(params.get("handle"), None)
    return {}


@method("ignore.set_shelf")
def _ignore_set_shelf(params: dict) -> dict:
    rules = _IGNORE_RULES.get(params.get("handle"))
    if rules:
        rules.set_shelf(params.get("file"))
    return {}


@method("ignore.set_git_excludes")
def _ignore_set_git_excludes(params: dict) -> dict:
    rules = _IGNORE_RULES.get(params.get("handle"))
    if rules:
        rules.set_git_excludes(params.get("path"))
    return {}


@method("ignore.set_recheck")
def _ignore_set_recheck(params: dict) -> dict:
    rules = _IGNORE_RULES.get(params.get("handle"))
    if rules:
        rules.set_recheck(float(params.get("seconds") or 2))
    return {}


@method("ignore.ignored")
def _ignore_ignored(params: dict) -> dict:
    rules = _IGNORE_RULES.get(params.get("handle"))
    if rules is None:
        return {"ignored": False}
    return {"ignored": rules.ignored(params.get("path") or "", bool(params.get("isDir")))}


@method("config.line")
def _config_line(params: dict) -> dict:
    return {"line": config_text.config_line(params.get("key") or "", params.get("value") or "")}


@method("config.section_entries")
def _config_section_entries(params: dict) -> dict:
    lines = (params.get("text") or "").split("\n")
    entries = config_text.config_section_entries(lines, params.get("section") or "")
    return {"entries": [{"index": i, "key": k, "value": v} for i, k, v in entries]}


@method("config.setting")
def _config_setting(params: dict) -> dict:
    lines = (params.get("text") or "").split("\n")
    kv = []
    for pair in params.get("kv") or []:
        if isinstance(pair, list) and len(pair) == 2:
            kv.append((pair[0] if isinstance(pair[0], str) else "",
                       pair[1] if isinstance(pair[1], str) else None))
    return {"text": "\n".join(config_text.config_setting(lines, params.get("section") or "", kv))}


@method("config.decode")
def _config_decode(params: dict) -> dict:
    out = []
    for i, raw in enumerate((params.get("text") or "").split("\n")):
        record = {"index": i, "trimmed": raw.strip()}
        header = config_text.config_section_header(raw)
        if header is not None:
            record["header"] = header
        entry = config_text.config_entry(raw)
        if entry is not None:
            record["key"], record["value"] = entry
        out.append(record)
    return {"lines": out}


@method("jira.paths")
def _jira_paths(params: dict) -> dict:
    try:
        import jira_paths
    except Exception as e:
        raise HelperError("jira.paths: %s" % e)
    return {
        "configJson": jira_paths.CONFIG_JSON,
        "teamJson": jira_paths.TEAM_JSON,
        "legacyConfig": jira_paths.LEGACY_CONFIG,
        "cacheDir": jira_paths.CACHE_DIR,
        "outDir": jira_paths.OUT_DIR_DEFAULT,
        "cache": dict(jira_paths.CACHE),
        "tabs": dict(jira_paths.TABS),
        "sideDirs": dict(jira_paths.SIDE_DIRS),
    }


@method("jira.filter_kinds")
def _jira_filter_kinds(params: dict) -> dict:
    return {"kinds": jira_search.filter_kinds(params.get("catalog") or [])}


@method("jira.criteria")
def _jira_criteria(params: dict) -> dict:
    return jira_search.criteria(params)


@method("jira.directory")
def _jira_directory(params: dict) -> dict:
    return jira_directory.load(params.get("path") or "")


@method("jira.options")
def _jira_options(params: dict) -> dict:
    kind = params.get("kind") or ""
    if kind == "value":
        return {"options": jira_directory.value_options(params.get("values") or [])}
    d = jira_directory.load(params.get("path") or "")
    if kind == "project":
        return {"options": jira_directory.project_options(d, params.get("scope") or [])}
    if kind == "user":
        return {"options": jira_directory.user_options(d, params.get("me") is not False)}
    if kind == "status":
        return {"options": jira_directory.status_options(
            d, params.get("words") or {}, params.get("categoryNames") or None)}
    if kind == "label":
        return {"options": jira_directory.label_options(d, params.get("scope"))}
    if kind == "version":
        return {"options": jira_directory.version_options(d, params.get("scope"))}
    raise HelperError("jira.options: unknown kind: %s" % kind)


@method("jira.board_columns")
def _jira_board_columns(params: dict) -> dict:
    return jira_boards.board_columns(params)


@method("jira.columns_parse")
def _jira_columns_parse(params: dict) -> dict:
    return {"columns": jira_fields.parse_columns(params.get("spec") or "")}


@method("jira.columns_serialize")
def _jira_columns_serialize(params: dict) -> dict:
    return {"spec": jira_fields.serialize_columns(params.get("columns") or [],
                                                  params.get("titles") is not False)}


@method("jira.project_keys")
def _jira_project_keys(params: dict) -> dict:
    return jira_setup.project_keys(params.get("raw") or "")


@method("confluence.preview_html")
def _confluence_preview_html(params: dict) -> dict:
    return {"html": confluence_pages.preview_html(params)}


@method("config.tri")
def _config_tri(params: dict) -> dict:
    return {"value": config_text.tri(params.get("text"))}


@method("config.resolve_binary")
def _config_resolve_binary(params: dict) -> dict:
    return {"path": config_text.resolve_binary(params.get("name") or "")}


@method("shelf.canonical")
def _shelf_canonical(params: dict) -> dict:
    return {"result": shelf.canonical(params.get("path") or "")}


@method("shelf.normalize")
def _shelf_normalize(params: dict) -> dict:
    return {"path": shelf.normalize(params.get("path") or "")}


@method("shelf.bump")
def _shelf_bump(params: dict) -> dict:
    return {"items": shelf.bump(params.get("items") or [], params.get("path") or "",
                                params.get("why") or "modified",
                                float(params.get("at") or 0),
                                int(params.get("limit") or shelf.MAX_LIMIT))}


@method("shelf.dedup")
def _shelf_dedup(params: dict) -> dict:
    return {"items": shelf.dedup(params.get("items") or [])}


@method("shelf.load")
def _shelf_load(params: dict) -> dict:
    return {"items": shelf.load_candidates(params.get("raw") or [])}


@method("shelf.finalize")
def _shelf_finalize(params: dict) -> dict:
    return {"items": shelf.finalize(params.get("items") or [],
                                    int(params.get("limit") or shelf.MAX_LIMIT))}


@method("shelf.save")
def _shelf_save(params: dict) -> dict:
    shelf.save(params.get("path") or "", params.get("items") or [])
    return {}


@method("shelf.rekey")
def _shelf_rekey(params: dict) -> dict:
    return {"path": shelf.rekey(params.get("path") or "", params.get("old") or "",
                                params.get("new") or "")}
