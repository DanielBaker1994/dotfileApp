#!/usr/bin/env bash
#
# jira-sync-test.sh - end-to-end cache sync test
#
# Sequence under test:
#   1. populate the cache            jira-api --sync 30m
#   2. dump the cached items to JSON (baseline snapshot + key list)
#   3. seed 100 fake issues          jira-seed -n 100 --versions --min-desc long --comments
#   4. mutate existing issues        add comments + toggle assignees on 10 baseline issues
#   5. poll the last 5 minutes       jira-api --sync 5m
#   6. assert the cache now = baseline issues + the 100 new ones (keyed by
#      Jira issue number, e.g. SAM1-170)
#   7. assert the comment changes were captured in the cache
#   8. assert the assignee changes were captured in the cache
#
# The seeded issues are left on the board for further testing; remove them
# afterwards with: jira-seed --cleanup
#
# Usage: jira-sync-test.sh
#

set -o pipefail

JIRA_API="$(cd "$(dirname "${BASH_SOURCE[0]}")/../jira" && pwd)/jira-api.sh"
JIRA_SEED="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/jira-seed.sh"
CACHE="$HOME/.cache/jira/jiras.json"
SEED_KEYS="$HOME/.cache/jira/seed_keys.txt"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASS=0 FAIL=0
pass() { printf 'PASS: %s\n' "$*"; PASS=$((PASS + 1)); }
fail() { printf 'FAIL: %s\n' "$*" >&2; FAIL=$((FAIL + 1)); }

echo "== step 1: populate the cache (--sync 30m)"
"$JIRA_API" --sync 30m >/dev/null 2>&1 || { echo "initial sync failed"; exit 1; }
[[ -f "$CACHE" ]] || { echo "no cache file after sync"; exit 1; }

echo "== step 2: dump the cached items as JSON"
jq . "$CACHE" > "$WORK/baseline.json"
jq -r 'keys[]' "$CACHE" | sort > "$WORK/baseline_keys.txt"
BASELINE=$(wc -l < "$WORK/baseline_keys.txt" | tr -d ' ')
echo "   baseline issues: $BASELINE"
echo "   baseline dump:   $WORK/baseline.json ($(wc -c < "$WORK/baseline.json" | tr -d ' ') bytes)"

echo "== step 3: seed 100 issues"
[[ -f "$SEED_KEYS" ]] && sort -u "$SEED_KEYS" > "$WORK/keys_before.txt" || : > "$WORK/keys_before.txt"
"$JIRA_SEED" -n 100 --versions --min-desc long --comments 2>&1 | tail -1
sort -u "$SEED_KEYS" > "$WORK/keys_after.txt"
comm -13 "$WORK/keys_before.txt" "$WORK/keys_after.txt" > "$WORK/new_keys.txt"
NEW=$(wc -l < "$WORK/new_keys.txt" | tr -d ' ')
echo "   new seeded keys: $NEW"

echo "== step 4: mutate existing issues (comments + assignee changes)"
SITE="$(grep JIRA_SITE  ~/.config/jira/config | cut -d\' -f2)"
EMAIL="$(grep JIRA_EMAIL ~/.config/jira/config | cut -d\' -f2)"
TOKEN="$(grep JIRA_TOKEN ~/.config/jira/config | cut -d\' -f2)"
AUTH="$EMAIL:$TOKEN"
MY_ID="$(curl -s -m 30 -u "$AUTH" "$SITE/rest/api/2/myself" | jq -r '.accountId')"
COMMENTED=()
while IFS= read -r k; do COMMENTED+=("$k"); done < <(sort -R "$WORK/baseline_keys.txt" | head -5)
for k in "${COMMENTED[@]}"; do
    curl -s -o /dev/null -m 30 -u "$AUTH" -H 'Content-Type: application/json' \
        -d "{\"body\":\"sync-test comment on $k $(date -u '+%H:%M:%SZ')\"}" \
        "$SITE/rest/api/2/issue/$k/comment"
    printf '%s\n' "$k" >> "$WORK/commented.txt"
done
echo "   comments added to: ${COMMENTED[*]}"
ASSIGNED=()
while IFS= read -r k; do ASSIGNED+=("$k"); done < <(sort -R "$WORK/baseline_keys.txt" | head -5)
for k in "${ASSIGNED[@]}"; do
    cur="$(curl -s -m 30 -u "$AUTH" "$SITE/rest/api/2/issue/$k?fields=assignee" | jq -r '.fields.assignee.accountId // "null"')"
    if [[ "$cur" == "null" ]]; then
        curl -s -o /dev/null -m 30 -u "$AUTH" -H 'Content-Type: application/json' -X PUT \
            -d "{\"fields\":{\"assignee\":{\"accountId\":\"$MY_ID\"}}}" "$SITE/rest/api/2/issue/$k"
        expected="Atlassin Lover"
    else
        curl -s -o /dev/null -m 30 -u "$AUTH" -H 'Content-Type: application/json' -X PUT \
            -d '{"fields":{"assignee":null}}' "$SITE/rest/api/2/issue/$k"
        expected=""
    fi
    printf '%s\t%s\n' "$k" "$expected" >> "$WORK/assignee_expected.txt"
done
echo "   assignee toggled on: ${ASSIGNED[*]}"

mutations_ok() {
    local k ok=1 cur
    while IFS= read -r k; do
        [[ -n "$k" ]] || continue
        jq -r --arg k "$k" '.[$k].comments[]?.body // empty' "$CACHE" | grep -q "sync-test comment on $k" || ok=0
    done < "$WORK/commented.txt"
    while IFS=$'\t' read -r k expected; do
        [[ -n "$k" ]] || continue
        cur="$(jq -r --arg k "$k" '.[$k].assignee // ""' "$CACHE")"
        [[ "$cur" == "$expected" ]] || ok=0
    done < "$WORK/assignee_expected.txt"
    return $((1 - ok))
}

echo "== step 5: poll the last 5 minutes (--sync 5m)"
RETRY=0
while :; do
    "$JIRA_API" --sync 5m >/dev/null 2>&1 || { echo "poll sync failed"; exit 1; }
    mutations_ok && break
    RETRY=$((RETRY + 1))
    if (( RETRY > 3 )); then
        echo "   WARNING: mutations still not visible after 4 polls"
        break
    fi
    echo "   search index lag - waiting 30s and re-polling ($RETRY/3)"
    sleep 30
done
jq -r 'keys[]' "$CACHE" | sort > "$WORK/post_keys.txt"

echo "== step 6: assert cache = baseline + 100 new"
MISSING_BASE=0
while IFS= read -r k; do
    [[ -n "$k" ]] || continue
    grep -qx "$k" "$WORK/post_keys.txt" || { MISSING_BASE=$((MISSING_BASE + 1)); echo "   missing baseline: $k"; }
done < "$WORK/baseline_keys.txt"
[[ "$MISSING_BASE" -eq 0 ]] && pass "all $BASELINE baseline issues still in cache" \
    || fail "$MISSING_BASE baseline issues missing"

MISSING_NEW=0
while IFS= read -r k; do
    [[ -n "$k" ]] || continue
    grep -qx "$k" "$WORK/post_keys.txt" || { MISSING_NEW=$((MISSING_NEW + 1)); echo "   missing new: $k"; }
done < "$WORK/new_keys.txt"
[[ "$MISSING_NEW" -eq 0 ]] && pass "all $NEW seeded issues in cache" \
    || fail "$MISSING_NEW seeded issues missing"

POST=$(wc -l < "$WORK/post_keys.txt" | tr -d ' ')
if [[ "$POST" -eq $((BASELINE + NEW)) ]]; then
    pass "cache total $POST = baseline $BASELINE + new $NEW"
else
    fail "cache total $POST != $((BASELINE + NEW))"
fi

LATEST_SNAP=$(ls -t "$HOME/.cache/jira/dumps/2026_"*.json 2>/dev/null | head -1)
[[ -s "$LATEST_SNAP" ]] && pass "post-sync JSON snapshot: $LATEST_SNAP"

echo "== step 7: assert comment changes captured in cache"
MISSING_CMT=0
while IFS= read -r k; do
    [[ -n "$k" ]] || continue
    jq -r --arg k "$k" '.[$k].comments[]?.body // empty' "$CACHE" | grep -q "sync-test comment on $k" \
        && pass "comment captured on $k" || { MISSING_CMT=$((MISSING_CMT + 1)); fail "comment MISSING on $k"; }
done < "$WORK/commented.txt"
[[ "$MISSING_CMT" -eq 0 ]] && echo "   all comment changes captured" || echo "   $MISSING_CMT comment changes missed"

echo "== step 8: assert assignee changes captured in cache"
MISSING_ASN=0
while IFS=$'\t' read -r k expected; do
    [[ -n "$k" ]] || continue
    cur="$(jq -r --arg k "$k" '.[$k].assignee // ""' "$CACHE")"
    if [[ "$cur" == "$expected" ]]; then
        pass "assignee change captured on $k (${expected:-unassigned})"
    else
        MISSING_ASN=$((MISSING_ASN + 1))
        fail "assignee on $k is '$cur', expected '${expected}'"
    fi
done < "$WORK/assignee_expected.txt"
[[ "$MISSING_ASN" -eq 0 ]] && echo "   all assignee changes captured" || echo "   $MISSING_ASN assignee changes missed"

echo
echo "== results: $PASS passed, $FAIL failed"
if [[ "$FAIL" -eq 0 ]]; then
    echo "ALL GOOD - cache holds baseline + seeded issues, keyed by Jira number"
    exit 0
else
    echo "FAILURES - see above"
    exit 1
fi