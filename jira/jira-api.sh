#!/usr/bin/env bash
#
# jira-api.sh - lightweight Jira Cloud REST API query tool
#
# Usage: jira-api.sh [OPTIONS] [ISSUE-KEY]
#
#   With no arguments, starts the interactive wizard.
#
# Discovery / setup:
#   --init            interactively write the config file (~/.config/jira/config)
#   --myself          print the authenticated user
#   --config FILE     use FILE instead of ~/.config/jira/config
#
# Filters (combined with AND into JQL):
#   -p, --project KEY      project = "KEY"
#   -a, --assignee NAME    assignee = "NAME"    NAME~ means fuzzy (assignee ~)
#   -r, --release NAME     fixVersion = "NAME"  (release == fix version)
#   -s, --status NAME      status = "NAME"
#   -u, --reporter NAME    reporter = "NAME"
#   -t, --text TERM        text ~ "TERM"
#   -d, --date EXPR        FIELD[:OP]VALUE, e.g. updated:-7d, due:>2024-05-01,
#                          created:startOfMonth(); FIELD defaults to "updated"
#   -j, --jql RAW          raw JQL, overrides all filters
#
# Ordering / limits:
#   -R, --recent           ORDER BY updated DESC (recently updated first)
#   -S, --sort EXPR        ORDER BY EXPR, e.g. "priority", "created ASC"
#   -n, --max N            max results (default 25)
#
# Output:
#   -o, --output table|json   table (default) or JSON shape:
#                             {key,title,status,assignee,release,priority,labels,description}
#   --releases                print ALL releases (versions) as JSON across every
#                             project (or just -p PROJECT):
#                             [{project,name,released,releaseDate,description}]
#   -verbose              append raw curl endpoints to /tmp/jira_api_dump.txt
#                         and write a bash trace (-x, PS4 with time+file:line)
#                         to /tmp/jira_api_trace.txt
#   --debug               print the JQL and request URLs
#   --no-auth-check           skip the /myself login verification at startup
#
# Sync (load recent changes into a local cache - trigger it however you like):
#   --sync WINDOW     load issues updated within WINDOW, merge into cache
#                     WINDOW: 30m | 2h | 7d | 1w | YYYY-MM-DD | full
#                             (datetime windows are in the site's local time)
#                     cache:  ~/.cache/jira/jiras.json (keyed by issue key,
#                             comments included, no history)
#                     snapshots: ~/.cache/jira/dumps/YYYY_MM_DD[_N].json
#
# Auth: JIRA_SITE, JIRA_EMAIL, JIRA_TOKEN from config file, environment,
#       or --site/--email/--token flags (API token from id.atlassian.com).
#       Every run verifies login via /rest/api/2/myself and aborts on failure.
#

set -o pipefail

CONFIG_FILE="${JIRA_CONFIG_FILE:-$HOME/.config/jira/config}"
JIRA_SITE="" JIRA_EMAIL="" JIRA_TOKEN="" JIRA_DEFAULT_PROJECT="" JIRA_MAX=25
CLI_SITE="" CLI_EMAIL="" CLI_TOKEN=""

PROJECT="" ASSIGNEE="" RELEASE="" STATUS="" REPORTER="" TEXT="" DATE_SPEC=""
JQL_RAW="" SORT="" RECENT=0 MAX="" OUTPUT="table" DEBUG=0 INTERACTIVE=0
DO_INIT=0 MYSELF=0 ISSUE_KEY="" VERBOSE=0 NO_AUTH_CHECK=0 SYNC_WINDOW="" RELEASES=0

CACHE_DIR="$HOME/.cache/jira"
CACHE_FILE="$CACHE_DIR/jiras.json"
STATE_FILE="$CACHE_DIR/state"
DUMP_DIR="$CACHE_DIR/dumps"
CURL_DUMP="/tmp/jira_api_dump.txt"
JIRA_TRACE="/tmp/jira_api_trace.txt"

die() { printf 'jira-api: %s\n' "$*" >&2; exit 1; }
debug() { (( DEBUG )) && printf 'jira-api: %s\n' "$*" >&2; }

usage() { sed -n '2,/^$/p' "$0" | sed 's/^# //; /^#$/d'; }

urlencode() { jq -rn --arg v "$1" '$v|@uri'; }

show_curl_ref() {
    local site email token u
    site="$JIRA_SITE"
    email="$JIRA_EMAIL"
    token="$JIRA_TOKEN"
    u="$email:$token"
    {
        printf -- '--- %s ---\n' "$(date '+%F %T')"
        printf 'jira-api.sh - raw curl endpoints (basic auth: -u "%s")\n\n' "$u"
        printf '1) Search issues (v3 JQL - used by every query; old /rest/api/2/search is gone)\n'
        printf '   GET /rest/api/3/search/jql?jql=<urlencoded>&fields=summary,status,assignee,fixVersions,description,updated,priority,labels&maxResults=<n>\n'
        printf '   curl -u "%s" "%s/rest/api/3/search/jql?jql=project%%20%%3D%%20SAM1%%20ORDER%%20BY%%20updated%%20DESC&fields=summary,status,assignee,fixVersions,description,updated,priority,labels&maxResults=25"\n' "$u" "$site"
        printf '2) Single issue (v2 - plain-text description)\n'
        printf '   GET /rest/api/2/issue/{key}\n'
        printf '   curl -u "%s" "%s/rest/api/2/issue/SAM1-1"\n' "$u" "$site"
        printf '3) List projects (interactive menu)\n'
        printf '   GET /rest/api/2/project\n'
        printf '   curl -u "%s" "%s/rest/api/2/project"\n' "$u" "$site"
        printf '4) Assignable users (interactive assignee suggestions)\n'
        printf '   GET /rest/api/2/user/assignable/search?project=KEY&maxResults=50\n'
        printf '   curl -u "%s" "%s/rest/api/2/user/assignable/search?project=SAM1&maxResults=50"\n' "$u" "$site"
        printf '5) Versions / releases (interactive release menu; maps to fixVersion)\n'
        printf '   GET /rest/api/2/project/{key}/versions\n'
        printf '   curl -u "%s" "%s/rest/api/2/project/SAM1/versions"\n' "$u" "$site"
        printf '6) Statuses (interactive status menu)\n'
        printf '   GET /rest/api/2/status\n'
        printf '   curl -u "%s" "%s/rest/api/2/status"\n' "$u" "$site"
        printf '7) Current user (login verification, runs on every invocation)\n'
        printf '   GET /rest/api/2/myself\n'
        printf '   curl -u "%s" "%s/rest/api/2/myself"\n' "$u" "$site"
        printf '\nNotes:\n'
        printf '  - Token: https://id.atlassian.com -> Security -> API tokens\n'
        printf '  - v3 search paginates with nextPageToken (no "total" field)\n'
        printf '  - v3 descriptions are Atlassian Document Format (ADF)\n'
        printf '\n'
    } >> "$CURL_DUMP"
    printf 'jira-api: curl reference appended to %s\n' "$CURL_DUMP" >&2
}

nextval() { [[ $# -ge 2 ]] || die "option $1 requires a value"; printf '%s' "$2"; }

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help) usage; exit 0 ;;
            --init) DO_INIT=1 ;;
            --myself) MYSELF=1 ;;
            --config) CONFIG_FILE="$(nextval "$@")"; shift ;;
            --config=*) CONFIG_FILE="${1#*=}" ;;
            --site) CLI_SITE="$(nextval "$@")"; shift ;;
            --site=*) CLI_SITE="${1#*=}" ;;
            --email) CLI_EMAIL="$(nextval "$@")"; shift ;;
            --email=*) CLI_EMAIL="${1#*=}" ;;
            --token) CLI_TOKEN="$(nextval "$@")"; shift ;;
            --token=*) CLI_TOKEN="${1#*=}" ;;
            -p|--project) PROJECT="$(nextval "$@")"; shift ;;
            --project=*) PROJECT="${1#*=}" ;;
            -a|--assignee) ASSIGNEE="$(nextval "$@")"; shift ;;
            --assignee=*) ASSIGNEE="${1#*=}" ;;
            -r|--release) RELEASE="$(nextval "$@")"; shift ;;
            --release=*) RELEASE="${1#*=}" ;;
            -s|--status) STATUS="$(nextval "$@")"; shift ;;
            --status=*) STATUS="${1#*=}" ;;
            -u|--reporter) REPORTER="$(nextval "$@")"; shift ;;
            --reporter=*) REPORTER="${1#*=}" ;;
            -t|--text) TEXT="$(nextval "$@")"; shift ;;
            --text=*) TEXT="${1#*=}" ;;
            -d|--date) DATE_SPEC="$(nextval "$@")"; shift ;;
            --date=*) DATE_SPEC="${1#*=}" ;;
            -j|--jql) JQL_RAW="$(nextval "$@")"; shift ;;
            --jql=*) JQL_RAW="${1#*=}" ;;
            -R|--recent) RECENT=1 ;;
            -S|--sort) SORT="$(nextval "$@")"; shift ;;
            --sort=*) SORT="${1#*=}" ;;
            -n|--max) MAX="$(nextval "$@")"; shift ;;
            --max=*) MAX="${1#*=}" ;;
            -o|--output) OUTPUT="$(nextval "$@")"; shift ;;
            --output=*) OUTPUT="${1#*=}" ;;
            -verbose|--verbose) VERBOSE=1 ;;
            --sync) SYNC_WINDOW="$(nextval "$@")"; shift ;;
            --sync=*) SYNC_WINDOW="${1#*=}" ;;
            --releases) RELEASES=1 ;;
            --no-auth-check) NO_AUTH_CHECK=1 ;;
            -i|--interactive) INTERACTIVE=1 ;;
            --debug) DEBUG=1 ;;
            -*)
                case "$1" in
                    -R|--recent|-i|--interactive|--debug|--init|--myself|-h|--help|-verbose|--verbose|--no-auth-check|--releases) ;;
                    *) die "unknown option: $1 (see --help)" ;;
                esac
                ;;
            *)
                [[ -n "$ISSUE_KEY" ]] && die "unexpected argument: $1"
                ISSUE_KEY="$1"
                ;;
        esac
        shift
    done
    [[ "$OUTPUT" == table || "$OUTPUT" == json ]] || die "--output must be 'table' or 'json'"
    if [[ -n "$MAX" && ! "$MAX" =~ ^[0-9]+$ ]]; then die "--max must be a number"; fi
}

load_config() {
    local env_site="$JIRA_SITE" env_email="$JIRA_EMAIL" env_token="$JIRA_TOKEN"
    if [[ -f "$CONFIG_FILE" ]]; then
        # shellcheck disable=SC1090
        source "$CONFIG_FILE"
    fi
    [[ -n "$env_site" ]] && JIRA_SITE="$env_site"
    [[ -n "$env_email" ]] && JIRA_EMAIL="$env_email"
    [[ -n "$env_token" ]] && JIRA_TOKEN="$env_token"
    JIRA_SITE="${CLI_SITE:-$JIRA_SITE}"
    JIRA_EMAIL="${CLI_EMAIL:-$JIRA_EMAIL}"
    JIRA_TOKEN="${CLI_TOKEN:-$JIRA_TOKEN}"
    JIRA_MAX="${JIRA_MAX:-25}"
    [[ "$JIRA_MAX" =~ ^[0-9]+$ ]] || JIRA_MAX=25
    [[ -z "$MAX" ]] && MAX="$JIRA_MAX"
    [[ "$MAX" =~ ^[0-9]+$ ]] || die "--max must be a number"
}

do_init() {
    local site email token proj max
    printf 'jira-api setup - writing config to %s\n' "$CONFIG_FILE"
    read -r -p "Jira site URL (e.g. https://your-org.atlassian.net): " site
    [[ -n "$site" ]] || die "site URL required"
    read -r -p "Email: " email
    [[ -n "$email" ]] || die "email required"
    read -r -s -p "API token (https://id.atlassian.com/manage-profile/security/api-tokens): " token
    printf '\n'
    [[ -n "$token" ]] || die "token required"
    read -r -p "Default project key (Enter for none): " proj
    read -r -p "Default max results [25]: " max
    max="${max:-25}"
    mkdir -p "$(dirname "$CONFIG_FILE")"
    {
        printf "JIRA_SITE='%s'\n" "$site"
        printf "JIRA_EMAIL='%s'\n" "$email"
        printf "JIRA_TOKEN='%s'\n" "$token"
        printf "JIRA_DEFAULT_PROJECT='%s'\n" "$proj"
        printf "JIRA_MAX='%s'\n" "$max"
        printf "JIRA_POLL_MARGIN='5'\n"
        printf "JIRA_POLL_STATE='%s'\n" "$HOME/.cache/jira/poll-state"
        printf "JIRA_POLL_OUT_DIR='%s'\n" "$HOME/.cache/workspace-switcher/jira_json"
        printf "JIRA_POLL_PROJECTS=''\n"
    } > "$CONFIG_FILE"
    chmod 600 "$CONFIG_FILE"
    printf 'Wrote %s (chmod 600). Test with: %s --myself\n' "$CONFIG_FILE" "$0"
}

jira_api() {
    local path="$1" qs="${2:-}" ver="${3:-2}" url outfile code
    url="$JIRA_SITE/rest/api/$ver$path"
    [[ -n "$qs" ]] && url="$url?$qs"
    debug "GET $url"
    outfile="$(mktemp)" || die "cannot create temp file"
    code="$(curl -sS -m 30 -u "$JIRA_EMAIL:$JIRA_TOKEN" \
        -H 'Accept: application/json' -o "$outfile" -w '%{http_code}' "$url")" \
        || { rm -f "$outfile"; die "curl failed: $url"; }
    if (( code >= 400 )); then
        local err
        err="$(cat "$outfile")"
        rm -f "$outfile"
        case "$code" in
            401) die "authentication failed (HTTP 401) - check JIRA_EMAIL / JIRA_TOKEN (run 'jira-api.sh --init')" ;;
            403) die "forbidden (HTTP 403) - your account lacks permission for this query" ;;
            404) die "not found (HTTP 404): $err" ;;
            *) die "API error HTTP $code: $err" ;;
        esac
    fi
    cat "$outfile"
    rm -f "$outfile"
}

jql_parts=()
jql_add() { jql_parts+=("$1"); }

parse_date_spec() {
    local spec="$1" field val op
    if [[ "$spec" == *:* ]]; then
        field="${spec%%:*}"; val="${spec#*:}"
    else
        field="updated"; val="$spec"
    fi
    if [[ "$val" =~ ^([<>]=?|=|~)(.*)$ ]]; then
        op="${BASH_REMATCH[1]}"; val="${BASH_REMATCH[2]}"
    elif [[ "$val" =~ ^-[0-9] ]]; then
        op=">="
    else
        op="="
    fi
    [[ -n "$val" ]] || die "empty value in date spec: $spec"
    jql_add "$field $op $val"
}

build_jql() {
    local q=""
    jql_parts=()
    [[ -n "$JQL_RAW" ]] && { printf '%s' "$JQL_RAW"; return; }
    [[ -n "$PROJECT" ]] && jql_add "project = \"$PROJECT\""
    if [[ -n "$ASSIGNEE" ]]; then
        if [[ "$ASSIGNEE" == *~ ]]; then
            jql_add "assignee ~ \"${ASSIGNEE%?}\""
        else
            jql_add "assignee = \"$ASSIGNEE\""
        fi
    fi
    [[ -n "$RELEASE" ]] && jql_add "fixVersion = \"$RELEASE\""
    [[ -n "$STATUS" ]] && jql_add "status = \"$STATUS\""
    [[ -n "$REPORTER" ]] && jql_add "reporter = \"$REPORTER\""
    [[ -n "$TEXT" ]] && jql_add "text ~ \"$TEXT\""
    [[ -n "$DATE_SPEC" ]] && parse_date_spec "$DATE_SPEC"
    if (( ${#jql_parts[@]} > 0 )); then
        local part
        q=""
        for part in "${jql_parts[@]}"; do
            [[ -n "$q" ]] && q="$q AND $part" || q="$part"
        done
    fi
    if (( RECENT )); then
        q="$q ORDER BY updated DESC"
    elif [[ -n "$SORT" ]]; then
        q="$q ORDER BY $SORT"
    fi
    printf '%s' "$q"
}

to_json() {
    jq -r 'def adf2text: if type == "object" then ([.. | objects | select(has("text")) | .text] | join("\n")) else . end;
    [.issues[] | {
        key: .key,
        title: (.fields.summary // ""),
        status: (.fields.status.name // ""),
        assignee: ((.fields.assignee.name // .fields.assignee.displayName) // ""),
        release: ([.fields.fixVersions[]?.name] | join(", ")),
        priority: (.fields.priority.name // ""),
        labels: ([.fields.labels[]?] | join(", ")),
        description: ((.fields.description | adf2text) // "")
    }]'
}

to_table() {
    jq -r '.issues[] | [
        .key,
        (.fields.status.name // "-"),
        (.fields.assignee.displayName // "-"),
        ([.fields.fixVersions[]?.name] | join(",") | if . == "" then "-" else . end),
        (.fields.updated[0:10] // "-"),
        (.fields.summary | gsub("\t"; " "))
    ] | @tsv'
}

render() {
    local body count is_last
    body="$(cat)"
    count="$(printf '%s' "$body" | jq -r '(.issues | length) // 0')"
    is_last="$(printf '%s' "$body" | jq -r 'if has("isLast") then .isLast else true end')"
    if (( count == 0 )); then
        printf 'No issues found\n'
        return 0
    fi
    if [[ "$OUTPUT" == json ]]; then
        printf '%s' "$body" | to_json
    else
        printf '%s' "$body" | to_table | {
            printf 'KEY\tSTATUS\tASSIGNEE\tRELEASE\tUPDATED\tTITLE\n'
            cat
        } | column -t -s $'\t'
        if [[ "$is_last" == "true" ]]; then
            printf 'Total: %s\n' "$count"
        else
            printf 'Showing %s (more pages available; raise -n to fetch more)\n' "$count"
        fi
    fi
}

run_query() {
    local jql qs out
    jql="$(build_jql)"
    debug "JQL: ${jql:-<empty>}"
    if [[ -z "$jql" ]]; then
        die "no filters given - Jira requires a bounded query (add -p/-a/-s/-d/... or -j)"
    fi
    qs="jql=$(urlencode "$jql")&fields=summary,status,assignee,fixVersions,description,updated,priority,labels&maxResults=$MAX"
    out="$(jira_api /search/jql "$qs" 3)" || exit 1
    render <<< "$out"
}

issue() {
    local out
    out="$(jira_api /issue/"$1")" || exit 1
    render <<< "$(printf '%s' "$out" | jq '{issues: [.]}')"
}

# Every release (version) as JSON: [{project,name,released,releaseDate,description}].
# With -p PROJECT only that project's releases; otherwise every visible project.
# One versions call per project — the release date lives on the version object,
# so no per-issue queries are needed.
releases() {
    local proj="$1" acc p out
    if [[ -n "$proj" ]]; then
        out="$(jira_api /project/"$proj"/versions)" || exit 1
        printf '%s' "$out" | jq --arg p "$proj" \
            '[.[] | {project:$p, name:.name, released:(.released // false), releaseDate:(.releaseDate // ""), description:(.description // "")}]'
        return
    fi
    out="$(jira_api /project)" || exit 1
    acc="$(mktemp)" || die "cannot create temp file"
    while IFS= read -r p; do
        [[ -n "$p" ]] || continue
        jira_api /project/"$p"/versions 2>/dev/null \
            | jq -c --arg p "$p" \
                '[.[] | {project:$p, name:.name, released:(.released // false), releaseDate:(.releaseDate // ""), description:(.description // "")}]' \
            >> "$acc" 2>/dev/null || true
    done < <(printf '%s' "$out" | jq -r '.[].key')
    jq -s 'add // []' "$acc"
    rm -f "$acc"
}

parse_window() {
    local w="$1" num re_date re_dt
    re_date='^[0-9]{4}-[0-9]{2}-[0-9]{2}$'
    re_dt='^[0-9]{4}-[0-9]{2}-[0-9]{2}[T ][0-9]{2}:[0-9]{2}(:[0-9]{2})?(Z)?$'
    case "$w" in
        full) SINCE="" ;;
        [0-9]*[smhdw])
            num="${w%[smhdw]}"
            [[ "$num" =~ ^[0-9]+$ ]] || die "invalid window: $w"
            SINCE="-$w"
            ;;
        *)
            if [[ "$w" =~ $re_date ]]; then
                SINCE="$w"
            elif [[ "$w" =~ $re_dt ]]; then
                SINCE="${w%Z}"
            else
                die "invalid window: $w (use 30m, 2h, 7d, 1w, 2026-09-10, or full)"
            fi
            ;;
    esac
}

snapshot() {
    local day n file
    day="$(date +%Y_%m_%d)"
    n=0
    while :; do
        if (( n == 0 )); then
            file="$DUMP_DIR/$day.json"
        else
            file="$DUMP_DIR/${day}_${n}.json"
        fi
        [[ -e "$file" ]] || break
        n=$((n + 1))
    done
    jq . "$CACHE_FILE" > "$file"
    printf '%s' "$file"
}

sync() {
    local window="$1" jql qs page token is_last
    local acc entries keyed tmp raw key comments vers_map
    local fetched=0 changed=0 total=0 snap
    parse_window "$window"
    if [[ -n "$SINCE" ]]; then
        jql="updated >= \"$SINCE\""
    else
        jql="project != null"
    fi
    debug "JQL: $jql"
    mkdir -p "$CACHE_DIR" "$DUMP_DIR"
    acc="$(mktemp)" || die "cannot create temp file"
    entries="$(mktemp)" || die "cannot create temp file"
    token=""
    while :; do
        qs="jql=$(urlencode "$jql")&fields=summary,status,assignee,fixVersions,description,updated,reporter,project,priority,labels&maxResults=100"
        [[ -n "$token" ]] && qs="$qs&nextPageToken=$token"
        page="$(jira_api /search/jql "$qs" 3)" || exit 1
        printf '%s' "$page" | jq -c '.issues[]' >> "$acc"
        is_last="$(printf '%s' "$page" | jq -r 'if has("isLast") then .isLast else true end')"
        [[ "$is_last" == "true" ]] && break
        token="$(printf '%s' "$page" | jq -r '.nextPageToken // ""')"
        [[ -n "$token" ]] || die "pagination: no nextPageToken but isLast=false"
    done
    fetched="$(jq -s 'length' "$acc")"
    # version name -> {released, date} per project. releaseDate + the released
    # flag live on the VERSION object, not on the issue's fixVersions — one
    # versions call per project (cheap) maps them for every fetched issue.
    vers_map="{}"
    while IFS= read -r proj; do
        [[ -n "$proj" ]] || continue
        m="$(jira_api /project/"$proj"/versions 2>/dev/null \
            | jq -c '[.[] | {(.name): {released: (.released // false), date: (.releaseDate // "")}}] | add // {}' 2>/dev/null)"
        [[ -n "$m" && "$m" != "{}" ]] || continue
        vers_map="$(printf '%s' "$vers_map" | jq -c --arg p "$proj" --argjson m "$m" '.[$p] = $m')"
    done < <(jq -r '.fields.project.key' "$acc" | sort -u)
    printf '%s\n' "$vers_map" > "$CACHE_DIR/versions.json"
    while IFS= read -r raw; do
        [[ -n "$raw" ]] || continue
        key="$(printf '%s' "$raw" | jq -r '.key')"
        comments="$(jira_api /issue/"$key" 'fields=comment' \
            | jq -c '.fields.comment.comments // [] | map({author: (.author.displayName // ""), body: (.body // ""), created: (.created // ""), updated: (.updated // "")})')"
        printf '%s' "$raw" | jq -c --argjson c "$comments" --argjson v "$vers_map" '{
            key: .key,
            title: (.fields.summary // ""),
            status: (.fields.status.name // ""),
            assignee: ((.fields.assignee.name // .fields.assignee.displayName) // ""),
            release: ([.fields.fixVersions[]?.name] | join(", ")),
            releaseLabel: ([.fields.fixVersions[]?.name as $n | ($v[.fields.project.key][$n].date // "") as $d | if $d == "" then $n else "\($n) (\($d))" end] | join(", ")),
            releaseDate: ([.fields.fixVersions[]?.name as $n | ($v[.fields.project.key][$n].date // "") | select(. != "")] | join(", ")),
            releaseStatus: ([.fields.fixVersions[]?.name as $n | $v[.fields.project.key][$n] | select(. != null) | .released] | if length == 0 then "" elif any(.[]; .) then "Released" else "Upcoming" end),
            priority: (.fields.priority.name // ""),
            labels: ([.fields.labels[]?] | join(", ")),
            description: ((.fields.description | if type == "object" then ([.. | objects | select(has("text")) | .text] | join("\n")) else . end) // ""),
            updated: (.fields.updated // ""),
            reporter: ((.fields.reporter.name // .fields.reporter.displayName) // ""),
            project: (.fields.project.key // ""),
            comments: $c
        }' >> "$entries"
        changed=$((changed + 1))
    done < "$acc"
    if (( changed > 0 )); then
        keyed="$(jq -s 'map({(.key): .}) | add' "$entries")"
        tmp="$(mktemp)" || die "cannot create temp file"
        if [[ -f "$CACHE_FILE" ]]; then
            jq -s '.[0] * .[1]' "$CACHE_FILE" - <<< "$keyed" > "$tmp"
        else
            printf '%s' "$keyed" > "$tmp"
        fi
        mv "$tmp" "$CACHE_FILE"
    fi
    total="$(jq 'length' "$CACHE_FILE" 2>/dev/null || echo 0)"
    {
        printf 'LAST_SYNC=%s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
        printf 'WINDOW=%s\n' "$window"
        printf 'UPDATED=%s\n' "$changed"
        printf 'TOTAL=%s\n' "$total"
    } > "$STATE_FILE"
    printf 'jira-api: sync %s: fetched %s, changed %s, cache now %s\n' \
        "$window" "$fetched" "$changed" "$total" >&2
    if (( total > 0 )); then
        snap="$(snapshot)"
        printf 'jira-api: snapshot: %s\n' "$snap" >&2
    fi
    rm -f "$acc" "$entries"
}

menu() {
    local label="$1" default="$2"; shift 2
    local -a options=("$@")
    local i reply choice d
    printf '%s [%s]\n' "$label" "${default:-any}" >&2
    if (( ${#options[@]} > 0 )); then
        for i in "${!options[@]}"; do
            d="${options[$i]}"
            if [[ "$d" == *\|* ]]; then
                printf '  %2d) %s (%s)\n' $((i + 1)) "${d%%|*}" "${d#*|}" >&2
            else
                printf '  %2d) %s\n' $((i + 1)) "$d" >&2
            fi
        done
        printf '  0) any\n' >&2
    fi
    if [[ -t 0 ]]; then
        read -r -e -p "> " reply
    else
        read -r -p "> " reply
    fi
    [[ -z "$reply" ]] && reply="$default"
    if [[ "$reply" == "0" ]]; then
        choice=""
    elif [[ "$reply" =~ ^[0-9]+$ ]] && (( reply >= 1 && reply <= ${#options[@]} )); then
        choice="${options[$((reply - 1))]%%|*}"
    else
        choice="$reply"
    fi
    printf '%s' "$choice"
}

interactive() {
    local p a r s days sort_sel m opt valid
    local -a projs users vers sts sorts
    printf 'jira-api interactive - Enter accepts the [default], 0 = any\n\n'

    mapfile -t projs < <(jira_api /project 2>/dev/null | jq -r '.[] | .key + "|" + .name' 2>/dev/null)
    while :; do
        p="$(menu "Project" "${PROJECT:-$JIRA_DEFAULT_PROJECT}" "${projs[@]}")"
        [[ -z "$p" || ${#projs[@]} -eq 0 ]] && break
        valid=0
        for opt in "${projs[@]}"; do
            [[ "$p" == "${opt%%|*}" ]] && { valid=1; break; }
        done
        (( valid )) && break
        printf 'Project "%s" not found on %s - choose a listed key or 0 for any\n' \
            "$p" "$JIRA_SITE" >&2
    done
    PROJECT="$p"

    if [[ -n "$p" ]]; then
        mapfile -t users < <(jira_api /user/assignable/search "project=$p&maxResults=50" 2>/dev/null \
            | jq -r '.[] | ((.name // .displayName) + "|" + .displayName)' 2>/dev/null)
        users=("me|currentUser()" "${users[@]}")
        a="$(menu "Assignee" "$ASSIGNEE" "${users[@]}")"
        [[ "$a" == "me" ]] && a="currentUser()"
        ASSIGNEE="$a"

        mapfile -t vers < <(jira_api /project/"$p"/versions 2>/dev/null \
            | jq -r '.[] | .name' 2>/dev/null | sort -u)
        r="$(menu "Release (fixVersion)" "$RELEASE" "${vers[@]}")"
        RELEASE="$r"
    fi

    mapfile -t sts < <(jira_api /status 2>/dev/null | jq -r 'unique_by(.name)[].name' 2>/dev/null)
    s="$(menu "Status" "$STATUS" "${sts[@]}")"
    STATUS="$s"

    if [[ -t 0 ]]; then
        read -r -e -p "Only updated within last N days, 0 = all [30]: " days
    else
        read -r -p "Only updated within last N days, 0 = all [30]: " days
    fi
    days="${days:-30}"
    [[ "$days" != "0" ]] && DATE_SPEC="updated:-${days}d"

    sorts=("updated DESC|recently updated first" "created DESC|newest created first"
           "priority|priority" "key|key")
    sort_sel="$(menu "Sort" "${SORT:-updated DESC}" "${sorts[@]}")"
    [[ -n "$sort_sel" ]] && SORT="$sort_sel"

    if [[ -t 0 ]]; then
        read -r -e -p "Max results [$JIRA_MAX]: " m
    else
        read -r -p "Max results [$JIRA_MAX]: " m
    fi
    MAX="${m:-$JIRA_MAX}"

    printf '\n'
    run_query
}

preflight() {
    command -v curl >/dev/null 2>&1 || die "curl is not installed (brew install curl)"
    command -v jq >/dev/null 2>&1 || die "jq is not installed (brew install jq)"
    local missing=""
    [[ -n "$JIRA_SITE" ]] || missing="$missing JIRA_SITE"
    [[ -n "$JIRA_EMAIL" ]] || missing="$missing JIRA_EMAIL"
    [[ -n "$JIRA_TOKEN" ]] || missing="$missing JIRA_TOKEN"
    [[ -z "$missing" ]] || die "missing config:$missing - run 'jira-api.sh --init' or export them"
    if (( ! NO_AUTH_CHECK && ! MYSELF )); then
        local me
        me="$(jira_api /myself)" || exit 1
        printf 'jira-api: login OK (%s)\n' \
            "$(printf '%s' "$me" | jq -r '.displayName // "unknown user"')" >&2
    fi
}

main() {
    parse_args "$@"
    load_config
    if (( DO_INIT )); then do_init; exit 0; fi
    if (( VERBOSE )); then
        export PS4='+${EPOCHREALTIME} ${BASH_SOURCE}:${LINENO}:${FUNCNAME[0]}(): '
        exec 3>>"$JIRA_TRACE"
        BASH_XTRACEFD=3
        printf -- '--- %s ---\n' "$(date '+%F %T')" >&3
        set -x
        printf 'jira-api: trace appended to %s\n' "$JIRA_TRACE" >&2
    fi
    preflight
    if (( VERBOSE )); then
        show_curl_ref
        if [[ -z "$PROJECT$ASSIGNEE$RELEASE$STATUS$REPORTER$TEXT$DATE_SPEC$JQL_RAW$ISSUE_KEY$SYNC_WINDOW" \
            && "$INTERACTIVE$RECENT$MYSELF" == 000 ]]; then
            exit 0
        fi
        printf '\n'
    fi
    if (( MYSELF )); then jira_api /myself | jq .; exit 0; fi
    if (( RELEASES )); then releases "$PROJECT"; exit 0; fi
    if [[ -n "$SYNC_WINDOW" ]]; then sync "$SYNC_WINDOW"; exit 0; fi
    if [[ -n "$ISSUE_KEY" ]]; then issue "$ISSUE_KEY"; exit 0; fi
    if (( INTERACTIVE )) || [[ $# -eq 0 ]]; then interactive; exit 0; fi
    run_query
}

main "$@"