#!/usr/bin/env bash
#
# jira-poll.sh - the polling ORCHESTRATOR: refreshes the cache via
# jira-api.sh --sync, then publishes JSON files the jira window reads.
#
#  1. Derive the poll window from the LAST run time (state file) minus a
#     safety margin — a missed poll window is covered automatically, because
#     the window stretches back to whenever we last succeeded.
#  2. Run `jira-api.sh --sync WINDOW` to refresh the cache (drip-feed).
#  3. Publish outputs to OUT_DIR (default ~/.cache/workspace-switcher/jira_json):
#       all.json          generic "all jiras" category (every cached issue)
#       <PROJECT>.json    one specialized file per project when configured
#                         (each file becomes its own tab in the jira window)
#  4. Record poll state: LAST_POLL (local time — the next run's window base),
#     WINDOW, STATUS, ITEMS, OUTPUTS, ERROR.
#
# Config (in ~/.config/jira/config, written by jira-api --init):
#   JIRA_POLL_MARGIN      minutes subtracted from LAST_POLL for the window
#                         (covers Jira's search-index lag); default 5
#   JIRA_POLL_STATE       state file; default ~/.cache/jira/poll-state
#   JIRA_POLL_OUT_DIR     output dir; default ~/.cache/workspace-switcher/jira_json
#   JIRA_POLL_PROJECTS    comma list -> specialized per-project files
#                         ("all" -> one file per project found in the cache)
#
# Launching at start (macOS): launchd, NOT cron (launchd survives sleep/wake
# and retries StartInterval on its own):
#   cp aerospace/jira/com.jira.poll.plist ~/Library/LaunchAgents/
#   launchctl load ~/Library/LaunchAgents/com.jira.poll.plist
#
# Usage:
#   jira-poll.sh                 # poll since last run (state file), publish
#   jira-poll.sh --init          # force a full sync (first run / reset)
#   jira-poll.sh --window 2h     # explicit window override (e.g. 2h)
#   jira-poll.sh --projects SAM1,KAN   # also publish per-project files
#   jira-poll.sh --projects all  # per-project files for every project found
#   jira-poll.sh --dry-run       # transform without writing anything
#   jira-poll.sh --quiet         # suppress jira-api output (for launchd)
#
# Exit codes: 0 = success, 1 = API failure after retries, 2 = usage/config error
#

set -o pipefail

CONFIG_FILE="${JIRA_CONFIG_FILE:-$HOME/.config/jira/config}"
JIRA_API_SH="${JIRA_API_SH:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/jira-api.sh}"
CACHE_FILE="$HOME/.cache/jira/jiras.json"

if [[ -f "$CONFIG_FILE" ]]; then
    # shellcheck disable=SC1090
    source "$CONFIG_FILE"
fi
MARGIN="${JIRA_POLL_MARGIN:-5}"
STATE_FILE="${JIRA_POLL_STATE:-$HOME/.cache/jira/poll-state}"
OUT_DIR="${JIRA_POLL_OUT_DIR:-$HOME/.cache/workspace-switcher/jira_json}"
POLL_PROJECTS="${JIRA_POLL_PROJECTS:-}"

WINDOW=""            # empty = derive from state file
WINDOW_OVERRIDE=0
FORCE_FULL=0
DRY_RUN=0
QUIET=0
PROJECTS="$POLL_PROJECTS"
MAX_TRIES=3

die() { printf 'jira-poll: %s\n' "$*" >&2; exit 2; }
say() { (( QUIET )) || printf 'jira-poll: %s\n' "$*" >&2; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --init) FORCE_FULL=1 ;;
        --window) WINDOW="${2:?--window needs a value}"; WINDOW_OVERRIDE=1; shift ;;
        --window=*) WINDOW="${1#*=}"; WINDOW_OVERRIDE=1 ;;
        --projects) PROJECTS="${2:?--projects needs a value}"; shift ;;
        --projects=*) PROJECTS="${1#*=}" ;;
        --dry-run) DRY_RUN=1 ;;
        --quiet) QUIET=1 ;;
        -h|--help) sed -n '2,/^$/p' "$0" | sed 's/^# //; /^#$/d'; exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
    shift
done

[[ -x "$JIRA_API_SH" ]] || die "jira-api.sh not found at $JIRA_API_SH"
command -v jq >/dev/null 2>&1 || die "jq is not installed (brew install jq)"
[[ "$MARGIN" =~ ^[0-9]+$ ]] || MARGIN=5

# --- window selection ---
if (( FORCE_FULL )); then
    WINDOW="full"
    say "full sync (--init)"
elif (( WINDOW_OVERRIDE )); then
    say "window override: $WINDOW"
elif [[ ! -f "$CACHE_FILE" ]]; then
    WINDOW="full"
    say "no cache yet - running full sync"
else
    # window = last successful poll time minus the margin, so a missed poll
    # window (sleep/wake, machine off) is covered automatically. First poll
    # with no state falls back to a 10m window.
    last_poll="$(awk -F= '/^LAST_POLL=/{v=$2} END{print v}' "$STATE_FILE" 2>/dev/null)"
    if [[ -n "$last_poll" ]]; then
        last_poll="${last_poll//T/ }"
        WINDOW="$(date -j -v-"$MARGIN"M -f '%Y-%m-%d %H:%M:%S' "$last_poll" '+%Y-%m-%d %H:%M:%S' 2>/dev/null)"
        if [[ -n "$WINDOW" ]]; then
            say "window from last poll ($last_poll) minus ${MARGIN}m: $WINDOW"
        else
            WINDOW="10m"
            say "unparseable last poll ($last_poll) - defaulting to 10m"
        fi
    else
        WINDOW="10m"
        say "no previous poll - defaulting to 10m window"
    fi
fi

# --- 1. refresh the cache (retry with exponential backoff) ---
status="error"
err=""
attempt=1
while (( attempt <= MAX_TRIES )); do
    say "sync [$attempt/$MAX_TRIES]: window=$WINDOW"
    if "$JIRA_API_SH" --sync "$WINDOW"; then
        status="ok"
        err=""
        break
    else
        err="sync attempt $attempt failed"
        say "sync failed (attempt $attempt/$MAX_TRIES)"
        if (( attempt < MAX_TRIES )); then
            sleep=$((10 * 2 ** (attempt - 1)))
            say "retrying in ${sleep}s"
            sleep "$sleep"
        fi
    fi
    attempt=$((attempt + 1))
done

if [[ "$status" != "ok" ]]; then
    printf 'LAST_POLL=%s\nWINDOW=%s\nSTATUS=error\nITEMS=%s\nOUTPUTS=0\nERROR=%s\n' \
        "$(date '+%Y-%m-%d %H:%M:%S')" "$WINDOW" \
        "$(jq 'length' "$CACHE_FILE" 2>/dev/null || echo 0)" "$err" > "$STATE_FILE"
    say "giving up after $MAX_TRIES attempts: $err"
    exit 1
fi

[[ -f "$CACHE_FILE" ]] || die "cache file missing after sync: $CACHE_FILE"

# --- 2. publish outputs: all.json (generic) + per-project files ---
# shape matches the sample the window was built against:
#   [{key,title,status,assignee,release,releaseLabel,releaseDate,releaseStatus,
#     priority,labels,description,reporter,project}]
# sorted by updated DESC so the most recently touched issues come first
BASE_TRANSFORM='[ .[] | {key,title,status,assignee,release,releaseLabel,releaseDate,releaseStatus,priority,labels,description,reporter,project} ] | sort_by(.updated // "") | reverse'
PER_PROJECT_TRANSFORM='[ .[] | select(.project == $p) ] | sort_by(.updated // "") | reverse | map({key,title,status,assignee,release,releaseLabel,releaseDate,releaseStatus,priority,labels,description,reporter,project})'

write_json() {
    local name="$1" transform="$2" jqarg="${3:-}" input="${4:-$CACHE_FILE}" items count tmp
    if [[ -n "$jqarg" ]]; then
        # shellcheck disable=SC2086
        items="$(jq -c $jqarg "$transform" "$input")" || die "transform failed for $name.json"
    else
        items="$(jq -c "$transform" "$input")" || die "transform failed for $name.json"
    fi
    count="$(printf '%s' "$items" | jq 'length')"
    if (( DRY_RUN )); then
        say "dry-run: would write $count item(s) to $OUT_DIR/$name.json"
        return 0
    fi
    mkdir -p "$OUT_DIR"
    tmp="$(mktemp "$OUT_DIR/.$name.json.XXXXXX")" || die "cannot create temp file"
    printf '%s\n' "$items" | jq . > "$tmp" || { rm -f "$tmp"; die "cannot write $name.json"; }
    chmod 644 "$tmp"
    mv "$tmp" "$OUT_DIR/$name.json"
    say "wrote $count item(s) to $OUT_DIR/$name.json"
}

outputs=1
write_json "all" "$BASE_TRANSFORM"

if [[ -n "$PROJECTS" ]]; then
    declare -a projs
    if [[ "$PROJECTS" == "all" ]]; then
        while IFS= read -r p; do projs+=("$p"); done < <(jq -r '.[].project' "$CACHE_FILE" | sort -u)
        say "per-project files for: ${projs[*]}"
    else
        IFS=',' read -ra projs <<< "$PROJECTS"
    fi
    for p in "${projs[@]}"; do
        [[ -n "$p" ]] || continue
        write_json "$p" "$PER_PROJECT_TRANSFORM" "--arg p $p"
        outputs=$((outputs + 1))
    done
fi

# --- 2b. release list: every version across the projects as a "releases" tab.
# Shaped into the window schema (key = "<PROJECT>-<name>", status Released/
# Upcoming) so the checkbox copy + filters work for releases too.
if [[ -n "${JIRA_SITE:-}" ]]; then
    RELEASES_JSON="$( "$JIRA_API_SH" --releases --no-auth-check 2>/dev/null )"
    if [[ -n "$RELEASES_JSON" ]]; then
        RELEASES_TRANSFORM='[.[] | {
            key: (.project + "-" + .name),
            title: .name,
            status: (if .released then "Released" else "Upcoming" end),
            assignee: "",
            release: .name,
            releaseLabel: (if (.releaseDate // "") != "" then (.name + " (" + .releaseDate + ")") else .name end),
            releaseDate: (.releaseDate // ""),
            releaseStatus: (if .released then "Released" else "Upcoming" end),
            priority: "",
            labels: "",
            description: "",
            reporter: "",
            project: .project
        }] | sort_by(.releaseDate // "", .name) | reverse'
        write_json "releases" "$RELEASES_TRANSFORM" "" <(printf '%s' "$RELEASES_JSON")
        outputs=$((outputs + 1))
    else
        say "releases query failed - skipping releases.json"
    fi
fi

# --- 3. record poll state (LAST_POLL in LOCAL time: the next window base) ---
printf 'LAST_POLL=%s\nWINDOW=%s\nSTATUS=%s\nITEMS=%s\nOUTPUTS=%s\nERROR=%s\n' \
    "$(date '+%Y-%m-%d %H:%M:%S')" "$WINDOW" "$status" \
    "$(jq 'length' "$CACHE_FILE")" "$outputs" "" > "$STATE_FILE"
say "poll complete: window=$WINDOW, $outputs output file(s), state=$STATE_FILE"
exit 0