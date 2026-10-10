from __future__ import annotations

import base64
import threading

import ai_format
import compare_folder
import compare_text
import config_text
import doc_templates
import file_ops as fileops
import ignore_rules as ignore
import jira_data
import jira_pages
import prose_pdf
import status as ws_status

from . import HelperError, method, script_bridge

_IGNORE_RULES = {}
_IGNORE_NEXT = [1]

_COMPARE_MODELS = {}
_COMPARE_NEXT = [1]

# Requests run concurrently now; handle allocation must not race.
_HANDLE_LOCK = threading.Lock()


def _compare_model(params):
    model = _COMPARE_MODELS.get(params.get("handle"))
    if model is None:
        raise HelperError("no such compare model")
    return model


@method("compare.new")
def _compare_new(params: dict) -> dict:
    model = compare_text.TextCompare(params.get("left"), params.get("right"),
                                     params.get("importance"),
                                     bool(params.get("ignoreUnimportant")))
    with _HANDLE_LOCK:
        handle = _COMPARE_NEXT[0]
        _COMPARE_NEXT[0] += 1
    _COMPARE_MODELS[handle] = model
    snap = model.snapshot()
    snap["handle"] = handle
    return snap


@method("compare.replace")
def _compare_replace(params: dict) -> dict:
    model = _compare_model(params)
    eols = params.get("eols")
    model.replace(params.get("side") or "left", int(params.get("start") or 0),
                  int(params.get("count") or 0), params.get("lines") or [], eols)
    return model.snapshot()


@method("compare.undo")
def _compare_undo(params: dict) -> dict:
    model = _compare_model(params)
    model.undo(params.get("side"))
    return model.snapshot()


@method("compare.redo")
def _compare_redo(params: dict) -> dict:
    model = _compare_model(params)
    model.redo()
    return model.snapshot()


@method("compare.align")
def _compare_align(params: dict) -> dict:
    model = _compare_model(params)
    model.align(int(params.get("l") or 0), int(params.get("r") or 0))
    return model.snapshot()


@method("compare.clear_alignment")
def _compare_clear_alignment(params: dict) -> dict:
    model = _compare_model(params)
    model.clear_alignment(params.get("row"))
    return model.snapshot()


@method("compare.set_importance")
def _compare_set_importance(params: dict) -> dict:
    model = _compare_model(params)
    model.set_importance(params.get("importance") or {})
    return model.snapshot()


@method("compare.set_ignore_unimportant")
def _compare_set_ignore(params: dict) -> dict:
    model = _compare_model(params)
    model.set_ignore_unimportant(bool(params.get("on")))
    return model.snapshot()


@method("compare.swap_sides")
def _compare_swap(params: dict) -> dict:
    model = _compare_model(params)
    model.swap_sides()
    return model.snapshot()


@method("compare.recompute")
def _compare_recompute(params: dict) -> dict:
    model = _compare_model(params)
    model.recompute()
    return model.snapshot()


@method("compare.set_side")
def _compare_set_side(params: dict) -> dict:
    model = _compare_model(params)
    model.set_side(params.get("side") or "left", params.get("text") or {})
    return model.snapshot()


@method("compare.copy_rows")
def _compare_copy_rows(params: dict) -> dict:
    model = _compare_model(params)
    model.copy_rows(int(params.get("lo") or 0), int(params.get("hi") or 0),
                    params.get("from") or "left")
    return model.snapshot()


@method("compare.copy_section")
def _compare_copy_section(params: dict) -> dict:
    model = _compare_model(params)
    model.copy_section(int(params.get("index") or 0), params.get("from") or "left")
    return model.snapshot()


@method("compare.trim")
def _compare_trim(params: dict) -> dict:
    model = _compare_model(params)
    count = model.trim_trailing_whitespace(params.get("side") or "left")
    snap = model.snapshot()
    snap["count"] = count
    return snap


@method("compare.convert")
def _compare_convert(params: dict) -> dict:
    model = _compare_model(params)
    count = model.convert_line_endings(params.get("side") or "left", int(params.get("eol") or 1))
    snap = model.snapshot()
    snap["count"] = count
    return snap


@method("compare.decode")
def _compare_decode(params: dict) -> dict:
    data = base64.b64decode(params.get("data") or "")
    side = compare_text.decode(data)
    return {"binary": side is None, "side": side}


@method("compare.encode")
def _compare_encode(params: dict) -> dict:
    data = compare_text.encode(params.get("side") or {})
    return {"data": base64.b64encode(data).decode("ascii") if data is not None else None}


@method("compare.marks")
def _compare_marks(params: dict) -> dict:
    return compare_text.char_marks(params.get("a") or "", params.get("b") or "",
                                   params.get("importance") or {})


@method("compare.char_diff")
def _compare_char_diff(params: dict) -> dict:
    return {"ops": compare_text.char_diff(params.get("a") or "", params.get("b") or "")}


@method("compare.char_changes")
def _compare_char_changes(params: dict) -> dict:
    return {"count": compare_text.char_changes(params.get("ops") or [])}


@method("compare.binary_first_difference")
def _compare_binary_first(params: dict) -> dict:
    a = base64.b64decode(params.get("a") or "")
    b = base64.b64decode(params.get("b") or "")
    return {"at": compare_text.binary_first_difference(a, b)}


@method("compare.text_equal")
def _compare_text_equal(params: dict) -> dict:
    return {"equal": compare_text.text_equal_under_rules(
        params.get("a") or "", params.get("b") or "",
        params.get("importance") or {}, int(params.get("limit") or (4 << 20)))}


@method("folder.classify")
def _folder_classify(params: dict) -> dict:
    return compare_folder.classify(params.get("left"), params.get("right"),
                                   params.get("options") or {})


@method("folder.classify_many")
def _folder_classify_many(params: dict) -> dict:
    return {"results": compare_folder.classify_many(params.get("items") or [],
                                                    params.get("options") or {})}


@method("folder.content_check")
def _folder_content_check(params: dict) -> dict:
    sizes = params.get("sizes") or [0, 0]
    return {"answer": compare_folder.content_check(
        params.get("left") or "", params.get("right") or "",
        int(sizes[0]), int(sizes[1]), params.get("importance") or {})}


@method("folder.scan_start")
def _folder_scan_start(params: dict) -> dict:
    handle = compare_folder.scan_start(params.get("left") or "", params.get("right") or "",
                                       params.get("options") or {},
                                       bool(params.get("caseInsensitive")))
    return {"session": handle}


@method("folder.scan_step")
def _folder_scan_step(params: dict) -> dict:
    return compare_folder.scan_step(params.get("session"), int(params.get("maxDirs") or 24))


@method("folder.scan_finish")
def _folder_scan_finish(params: dict) -> dict:
    return compare_folder.scan_finish(params.get("session"))


@method("folder.drop")
def _folder_drop(params: dict) -> dict:
    compare_folder.tree_drop(params.get("handle"))
    return {}


@method("folder.path")
def _folder_path(params: dict) -> dict:
    return compare_folder.tree_path(params.get("handle"), int(params.get("id") or 0),
                                    params.get("side") or "left")


@method("folder.pending")
def _folder_pending(params: dict) -> dict:
    return compare_folder.tree_pending(params.get("handle"))


@method("folder.counts")
def _folder_counts(params: dict) -> dict:
    return compare_folder.tree_count(params.get("handle"))


@method("folder.rows")
def _folder_rows(params: dict) -> dict:
    return compare_folder.tree_rows(params.get("handle"), params.get("filter") or "all",
                                    params.get("nameFilter") or "", bool(params.get("flatten")),
                                    params.get("expanded") or [])


@method("folder.matches_name")
def _folder_matches_name(params: dict) -> dict:
    return {"matches": compare_folder.matches_name(params.get("name") or "",
                                                   params.get("filter") or "")}


@method("folder.rule_candidates")
def _folder_rule_candidates(params: dict) -> dict:
    return compare_folder.tree_rule_candidates(params.get("handle"))


@method("folder.sync_plan")
def _folder_sync_plan(params: dict) -> dict:
    return compare_folder.tree_sync_plan(params.get("handle"), params.get("mode") or "updateRight",
                                         params.get("nameFilter") or "")


@method("folder.settle")
def _folder_settle(params: dict) -> dict:
    return compare_folder.tree_settle(params.get("handle"), params.get("statuses"))


@method("folder.apply_answers")
def _folder_apply_answers(params: dict) -> dict:
    return compare_folder.tree_apply_answers(params.get("handle"), params.get("answers") or {})


@method("folder.restat")
def _folder_restat(params: dict) -> dict:
    return compare_folder.tree_restat(params.get("handle"), int(params.get("id") or 0),
                                      params.get("options") or {})


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


@method("doc_templates.names")
def _names(params: dict) -> dict:
    return {"names": doc_templates.names(params.get("configured"), params.get("css"))}


@method("doc_templates.current")
def _current(params: dict) -> dict:
    return {"template": doc_templates.current(params.get("text") or "")}


@method("doc_templates.apply")
def _apply(params: dict) -> dict:
    return {"text": doc_templates.apply(params.get("text") or "", params.get("template"))}


@method("doc_templates.menu")
def _menu(params: dict) -> dict:
    text = params.get("text") or ""
    return {"current": doc_templates.current(text),
            "names": doc_templates.names(params.get("configured"), params.get("css") or None)}


@method("doc_templates.edit")
def _edit(params: dict) -> dict:
    return doc_templates.edit(params.get("text") or "", params.get("template"))


@method("prose.screen_html")
def _screen_html(params: dict) -> dict:
    return {"html": prose_pdf.screen_html(params.get("note") or "", params.get("config"))}


@method("prose.css_content")
def _css_content(params: dict) -> dict:
    return {"css": prose_pdf.css_content(prose_pdf.config(params.get("config")))}


@method("prose.pdf_export")
def _pdf_export(params: dict) -> dict:
    return prose_pdf.export(params.get("note") or "", params.get("config"))


@method("prose.pandoc_args")
def _pandoc_args(params: dict) -> dict:
    c = prose_pdf.config(params.get("config"))
    return {"args": prose_pdf.pandoc_args(params.get("note") or "", params.get("css") or "",
                                          params.get("html") or "", c,
                                          bool(params.get("sourcepos")))}


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


@method("script.run")
def _script_run(params: dict) -> dict:
    return script_bridge.run(params)


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


@method("jira.style")
def _jira_style(params: dict) -> dict:
    return jira_data.style_rules(params.get("values") or {})


@method("jira.comments")
def _jira_comments(params: dict) -> dict:
    return jira_data.comments(params.get("path") or "", params.get("key") or "")


@method("jira.ticket_html")
def _jira_ticket_html(params: dict) -> dict:
    return {"html": jira_pages.ticket_html(params)}


@method("jira.comments_html")
def _jira_comments_html(params: dict) -> dict:
    return {"html": jira_pages.comments_html(params.get("comments") or [])}
