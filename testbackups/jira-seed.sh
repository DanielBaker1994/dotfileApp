#!/usr/bin/env bash
#
# jira-seed.sh - dump fake Jira issues onto a project for testing
#
# Usage: jira-seed.sh [OPTIONS]
#
#   -n, --count N       number of issues to create (default 20)
#   -p, --project KEY   project key (default: JIRA_DEFAULT_PROJECT or SAM1)
#   --min-desc SIZE     minimum description size: empty|tiny|short|medium|long|huge
#   --max-desc SIZE     maximum description size (default huge)
#   -c, --comments      add comments to ~1/3 of the issues
#   --versions          create versions (12.0, 12.1, 13.0) and attach some issues
#   --cleanup           delete every issue key recorded in the seed-keys file
#   --dry-run           print what would be created, make no API calls
#   --debug             print each request URL
#
# Description sizes: empty, tiny (5-9 words), short (1-2 sentences),
# medium (3-5 sentences), long (8-12 sentences, multi-paragraph),
# huge (25-40 sentences, 4-6 paragraphs).
#
# Credentials come from ~/.config/jira/config (same as jira-api.sh).
# Created keys are recorded in ~/.cache/jira/seed_keys.txt for --cleanup.
#

set -o pipefail

CONFIG_FILE="${JIRA_CONFIG_FILE:-$HOME/.config/jira/config}"
KEYS_FILE="$HOME/.cache/jira/seed_keys.txt"

COUNT=20 PROJECT="" MIN_DESC="" MAX_DESC="huge" COMMENTS=0 VERSIONS=0 CLEANUP=0 DRY_RUN=0 DEBUG=0

SUBJ=(Refactor Fix Add Remove Document Optimize Migrate Investigate Update Improve
      Redesign Automate Deprecate Restructure Harden Extend Simplify Standardize
      Instrument Audit Rebalance Centralize Decouple Parallelize Vendor)
OBJ=("the login flow" "the notification service" "the billing engine" "the dashboard" "the API gateway"
     "the search index" "the webhook handler" "the data pipeline" "the payment processor" "user onboarding"
     "auth token refresh" "the rate limiter" "the logging stack" "the cache layer" "the report generator"
     "the export job" "the sync service" "the audit trail" "the feature flags" "the alert routing"
     "the metrics exporter" "the config loader" "the retry logic" "the circuit breaker" "the backup job")
SUFFIX=("before the next release window" "after the incident review" "across all environments"
        "for the Q3 roadmap" "per the compliance audit" "following the load test review"
        "ahead of the holiday freeze" "after the schema migration" "to unblock the mobile team"
        "per the security review")

LOREM=(lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor
       incididunt ut labore et dolore magna aliqua enim ad minim veniam quis nostrud
       exercitation ullamco laboris nisi aliquip ex ea commodo consequat duis aute
       irure in reprehenderit voluptate velit esse cillum fugiat nulla pariatur
       excepteur sint occaecat cupidatat non proident sunt culpa officia deserunt
       mollit anim id est laborum veritatis et quasi architecto beatae vitae dicta
       sunt explicabo nemo enim ipsam voluptatem quia voluptas aspernatur odit aut
       fugit sed quia consequuntur magni dolores eos qui ratione voluptatem sequi)

STATUSES=(In Progress Done)

die() { printf 'jira-seed: %s\n' "$*" >&2; exit 1; }
debug() { (( DEBUG )) && printf 'jira-seed: %s\n' "$*" >&2; }

pick() { local -n arr="$1"; printf '%s' "${arr[$((RANDOM % ${#arr[@]}))]}"; }

nextval() { [[ $# -ge 2 ]] || die "option $1 requires a value"; printf '%s' "$2"; }

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help)
                sed -n '2,/^$/p' "$0" | sed 's/^# //; /^#$/d'
                exit 0 ;;
            -n|--count) COUNT="$(nextval "$@")"; shift ;;
            --count=*) COUNT="${1#*=}" ;;
            -p|--project) PROJECT="$(nextval "$@")"; shift ;;
            --project=*) PROJECT="${1#*=}" ;;
            --min-desc) MIN_DESC="$(nextval "$@")"; shift ;;
            --min-desc=*) MIN_DESC="${1#*=}" ;;
            --max-desc) MAX_DESC="$(nextval "$@")"; shift ;;
            --max-desc=*) MAX_DESC="${1#*=}" ;;
            -c|--comments) COMMENTS=1 ;;
            --versions) VERSIONS=1 ;;
            --cleanup) CLEANUP=1 ;;
            --dry-run) DRY_RUN=1 ;;
            --debug) DEBUG=1 ;;
            *) die "unknown option: $1 (see --help)" ;;
        esac
        shift
    done
    [[ "$COUNT" =~ ^[0-9]+$ ]] || die "--count must be a number"
}

load_config() {
    if [[ -f "$CONFIG_FILE" ]]; then
        # shellcheck disable=SC1090
        source "$CONFIG_FILE"
    fi
    [[ -n "$JIRA_SITE" ]] || die "JIRA_SITE missing - run 'jira-api --init'"
    [[ -n "$JIRA_EMAIL" ]] || die "JIRA_EMAIL missing - run 'jira-api --init'"
    [[ -n "$JIRA_TOKEN" ]] || die "JIRA_TOKEN missing - run 'jira-api --init'"
    PROJECT="${PROJECT:-${JIRA_DEFAULT_PROJECT:-SAM1}}"
    [[ -n "$MIN_DESC" ]] || MIN_DESC="empty"
}

jira_req() {
    local method="$1" path="$2" body="${3:-}" ver="${4:-2}" url outfile code
    url="$JIRA_SITE/rest/api/$ver$path"
    debug "$method $url"
    outfile="$(mktemp)" || die "cannot create temp file"
    if [[ -n "$body" ]]; then
        code="$(curl -sS -m 30 -u "$JIRA_EMAIL:$JIRA_TOKEN" -H 'Content-Type: application/json' \
            -X "$method" -d "$body" -o "$outfile" -w '%{http_code}' "$url")" \
            || { rm -f "$outfile"; die "curl failed: $url"; }
    else
        code="$(curl -sS -m 30 -u "$JIRA_EMAIL:$JIRA_TOKEN" \
            -X "$method" -o "$outfile" -w '%{http_code}' "$url")" \
            || { rm -f "$outfile"; die "curl failed: $url"; }
    fi
    if (( code >= 400 )); then
        local err
        err="$(cat "$outfile")"
        rm -f "$outfile"
        die "$method $path -> HTTP $code: $err"
    fi
    cat "$outfile"
    rm -f "$outfile"
}

DESC_SIZES=(empty tiny short medium long huge)
desc_size() {
    local lo=-1 hi=-1 i
    for i in "${!DESC_SIZES[@]}"; do
        [[ "${DESC_SIZES[$i]}" == "$MIN_DESC" ]] && lo=$i
        [[ "${DESC_SIZES[$i]}" == "$MAX_DESC" ]] && hi=$i
    done
    (( lo >= 0 && hi >= 0 && lo <= hi )) || die "bad desc sizes: min=$MIN_DESC max=$MAX_DESC"
    printf '%s' "${DESC_SIZES[$((RANDOM % (hi - lo + 1) + lo))]}"
}

lorem_sentence() {
    local n=$((RANDOM % 9 + 6)) i w s=""
    for ((i = 0; i < n; i++)); do s+="$(pick LOREM) "; done
    s="${s% }"
    printf '%s' "${s^}"
}

gen_text() {
    local size="$1" n i s=""
    case "$size" in
        empty) return ;;
        tiny)
            for ((i = 0; i < $((RANDOM % 5 + 5)); i++)); do s+="$(pick LOREM) "; done
            s="${s% }"
            printf '%s' "${s^}"; return ;;
        short) n=$((RANDOM % 2 + 1)) ;;
        medium) n=$((RANDOM % 3 + 3)) ;;
        long) n=$((RANDOM % 5 + 8)) ;;
        huge) n=$((RANDOM % 16 + 25)) ;;
    esac
    for ((i = 0; i < n; i++)); do s+="$(lorem_sentence) "; done
    s="${s% }"
    printf '%s' "$s"
}

make_adf() {
    local text="$1" p
    if [[ -z "$text" ]]; then
        printf '%s' '{"type":"doc","version":1,"content":[]}'
        return
    fi
    p="$(printf '%s' "$text" | jq -R -s 'split("\n") | map(select(length>0)) | map({type:"paragraph",content:[{type:"text",text:.}]})')"
    jq -nc --argjson paras "$p" '{type:"doc",version:1,content:$paras}'
}

random_title() {
    local w=$((RANDOM % 3 + 2))
    if (( w >= 3 )); then
        printf '%s %s %s' "$(pick SUBJ)" "$(pick OBJ)" "$(pick SUFFIX)"
    else
        printf '%s %s' "$(pick SUBJ)" "$(pick OBJ)"
    fi
}

make_summary() {
    local t="$1"
    jq -nc --arg v "$t" '$v'
}

record_keys() {
    mkdir -p "$(dirname "$KEYS_FILE")"
    while IFS= read -r k; do
        [[ -n "$k" ]] && printf '%s\n' "$k" >> "$KEYS_FILE"
    done
}

cleanup() {
    [[ -f "$KEYS_FILE" ]] || die "no seed-keys file at $KEYS_FILE"
    local n=0
    while IFS= read -r k; do
        [[ -n "$k" ]] || continue
        jira_req DELETE "/issue/$k" >/dev/null 2>&1 && n=$((n + 1))
    done < "$KEYS_FILE"
    rm -f "$KEYS_FILE"
    printf 'jira-seed: deleted %s seeded issues (keys file cleared)\n' "$n"
}

create_issue() {
    local type="$1" summary="$2" adf="$3" body fields
    local prio labels version
    fields="$(jq -nc --arg p "$PROJECT" --arg t "$type" --arg s "$summary" --argjson d "$adf" \
        '{project:{key:$p},issuetype:{name:$t},summary:$s,description:$d}')"
    prio="$(pick PRIOS)"
    fields="$(printf '%s' "$fields" | jq -c --arg p "$prio" '. + {priority:{name:$p}}')"
    if (( RANDOM % 4 < 3 )); then
        labels="$(jq -nc --arg l "seed-$(pick LABELS)" '[$l]')"
        fields="$(printf '%s' "$fields" | jq -c --argjson l "$labels" '. + {labels:$l}')"
    fi
    if [[ -n "$CURRENT_VERSION" ]] && (( RANDOM % 3 < 2 )); then
        version="$(jq -nc --arg v "$CURRENT_VERSION" '[{name:$v}]')"
        fields="$(printf '%s' "$fields" | jq -c --argjson v "$version" '. + {fixVersions:$v}')"
    fi
    if (( RANDOM % 2 == 0 )); then
        fields="$(printf '%s' "$fields" | jq -c --arg a "$MY_ACCOUNT" '. + {assignee:{accountId:$a}}')"
    fi
    body="$(jq -nc --argjson f "$fields" '{fields:$f}')"
    if (( DRY_RUN )); then
        printf '%s\t%s\t%s\t%s\n' "$type" "$summary" "$adf" "$body" >&2
        printf 'DRY-%04d' "$((RANDOM % 9000 + 1000))"
        return
    fi
    jira_req POST /issue "$body" 3 | jq -r '.key'
}

transition() {
    local key="$1" name="$2" tid
    tid="$(jira_req GET "/issue/$key/transitions" | jq -r --arg n "$name" '.transitions[] | select(.name==$n) | .id' | head -1)"
    [[ -n "$tid" ]] || return 0
    jira_req POST "/issue/$key/transitions" "{\"transition\":{\"id\":$tid}}" >/dev/null
}

add_comments() {
    local key="$1" n=$((RANDOM % 3 + 1)) i text
    for ((i = 0; i < n; i++)); do
        text="$(lorem_sentence)"
        jira_req POST "/issue/$key/comment" "$(jq -nc --arg t "$text" '{body:$t}')" >/dev/null
    done
}

PRIOS=(Highest High Medium Low Lowest)
LABELS=(billing payments auth perf regression ux tech-debt qa-critical)

main() {
    parse_args "$@"
    load_config
    if (( CLEANUP )); then cleanup; exit 0; fi
    command -v jq >/dev/null 2>&1 || die "jq is not installed (brew install jq)"

    MY_ACCOUNT="$(jira_req GET /myself | jq -r '.accountId')"
    TYPES=(Task Story Epic)
    if (( RANDOM % 4 == 0 )); then TYPES+=("Epic"); fi

    if (( VERSIONS && ! DRY_RUN )); then
        local -a existing
        mapfile -t existing < <(jira_req GET "/project/$PROJECT/versions" | jq -r '.[].name')
        local v rel_date
        for v in 12.0 12.1 13.0; do
            [[ " ${existing[*]} " == *" $v "* ]] && continue
            # a released version with a release date — the window shows the
            # date next to the fix version, so seeded versions carry one too
            rel_date="$(date -v-"$(echo "$v" | tr -d '.')"d '+%Y-%m-%d')"
            jira_req POST /version "$(jq -nc --arg n "$v" --arg p "$PROJECT" \
                --arg d "$rel_date" '{name:$n,project:$p,released:true,releaseDate:$d}')" >/dev/null
        done
        CURRENT_VERSION="13.0"
    fi

    mkdir -p "$(dirname "$KEYS_FILE")"
    : > "$KEYS_FILE"

    printf 'jira-seed: creating %s issues in %s (desc %s..%s, comments=%s, versions=%s)\n' \
        "$COUNT" "$PROJECT" "$MIN_DESC" "$MAX_DESC" "$COMMENTS" "$VERSIONS" >&2

    local i size text adf key status total_status=0 total_comment=0
    local -A by_size
    for ((i = 0; i < COUNT; i++)); do
        size="$(desc_size)"
        text="$(gen_text "$size")"
        adf="$(make_adf "$text")"
        key="$(create_issue "$(pick TYPES)" "$(random_title)" "$adf")" || die "create failed at issue $((i + 1))"
        if (( DRY_RUN )); then
            continue
        fi
        record_keys <<< "$key"
        by_size[$size]=$(( ${by_size[$size]:-0} + 1 ))
        printf '%s\t%s\t%s\n' "$key" "$size" "$(printf '%s' "$text" | wc -w | tr -d ' ')" >&2
        if (( RANDOM % 3 < 2 )); then
            status="$(pick STATUSES)"
            transition "$key" "$status"
            total_status=$((total_status + 1))
        fi
        if (( COMMENTS && RANDOM % 3 == 0 )); then
            add_comments "$key"
            total_comment=$((total_comment + 1))
        fi
    done

    printf 'jira-seed: done - %s issues created, %s transitioned, %s with comments\n' \
        "$COUNT" "$total_status" "$total_comment" >&2
    printf 'jira-seed: sizes: %s\n' "$(for s in "${!by_size[@]}"; do printf '%s=%s ' "$s" "${by_size[$s]}"; done)" >&2
    printf 'jira-seed: keys recorded in %s (use --cleanup to delete)\n' "$KEYS_FILE" >&2
}

main "$@"