//! Port of `ListFilter.swift` — `PopupFuzzy` (character-based matcher),
//! `FuzzyIndex` (byte-optimized matcher + narrowing ranker) and `SortRank`.
//! Foundation/std only.

use std::cmp::Ordering;

/// `FuzzyIndex` is the byte-oriented fast path over `PopupFuzzy`'s character
/// matcher (Swift keeps a `memchr`/`memcmp` variant for the per-keystroke list
/// scan). Both share the same scoring constants.
const SCORE_MATCH: i64 = 16;
const BONUS_BOUNDARY_WHITE: i64 = 10; // bonusBoundary (8) + 2

pub struct FuzzyIndex {
    keys: Vec<Vec<u8>>,
    last_tokens: Vec<Vec<u8>>,
    last_matches: Vec<usize>,
}

impl FuzzyIndex {
    pub fn new(texts: &[String]) -> Self {
        FuzzyIndex {
            keys: texts.iter().map(|t| t.to_lowercase().into_bytes()).collect(),
            last_tokens: Vec::new(),
            last_matches: Vec::new(),
        }
    }

    pub fn count(&self) -> usize {
        self.keys.len()
    }

    pub fn tokens(query: &str) -> Vec<Vec<u8>> {
        query
            .to_lowercase()
            .split_whitespace()
            .map(|s| s.as_bytes().to_vec())
            .collect()
    }

    pub fn ranked(&mut self, query: &str) -> Vec<usize> {
        let ts = Self::tokens(query);
        if ts.is_empty() {
            self.last_tokens = Vec::new();
            self.last_matches = Vec::new();
            return (0..self.keys.len()).collect();
        }
        let narrowing = !self.last_tokens.is_empty()
            && self
                .last_tokens
                .iter()
                .all(|old| ts.iter().any(|t| Self::contains(t, old)));
        let candidates: Vec<usize> = if narrowing {
            self.last_matches.clone()
        } else {
            (0..self.keys.len()).collect()
        };
        let mut scores = vec![0i64; candidates.len()];
        let mut hit = vec![false; candidates.len()];
        for (j, &row) in candidates.iter().enumerate() {
            if let Some(s) = Self::score(&ts, &self.keys[row]) {
                scores[j] = s;
                hit[j] = true;
            }
        }
        let mut matches: Vec<(usize, i64)> = Vec::with_capacity(candidates.len());
        for j in 0..candidates.len() {
            if hit[j] {
                matches.push((candidates[j], scores[j]));
            }
        }
        self.last_tokens = ts;
        self.last_matches = matches.iter().map(|m| m.0).collect();
        matches.sort_by(|a, b| {
            if a.1 != b.1 {
                b.1.cmp(&a.1)
            } else {
                a.0.cmp(&b.0)
            }
        });
        matches.into_iter().map(|m| m.0).collect()
    }

    fn score(ts: &[Vec<u8>], text: &[u8]) -> Option<i64> {
        let mut total = 0i64;
        for t in ts {
            total += Self::match_token(t, text)?;
        }
        Some(total)
    }

    fn match_token(tok: &[u8], text: &[u8]) -> Option<i64> {
        let len = tok.len();
        let n = text.len();
        if len == 0 || len > n {
            return None;
        }
        let mut best = i64::MIN;
        let first = tok[0];
        let mut i = 0usize;
        let mut c_at = 0usize;
        let mut c_idx = 0usize;
        while i + len <= n {
            let end = n - len + 1;
            let j = match text[i..end].iter().position(|&b| b == first) {
                Some(p) => i + p,
                None => break,
            };
            while c_at < j {
                if text[c_at] & 0xC0 != 0x80 {
                    c_idx += 1;
                }
                c_at += 1;
            }
            if BONUS_BOUNDARY_WHITE - c_idx as i64 / 8 <= best {
                break;
            }
            if &text[j..j + len] == tok {
                let ws = j == 0 || Self::is_boundary(text, j);
                let sc = (if ws { BONUS_BOUNDARY_WHITE } else { 0 }) - c_idx as i64 / 8;
                if sc > best {
                    best = sc;
                }
                if ws {
                    break;
                }
            }
            i = j + 1;
        }
        if best == i64::MIN {
            None
        } else {
            Some(best + SCORE_MATCH * len as i64)
        }
    }

    fn is_boundary(t: &[u8], j: usize) -> bool {
        let b = t[j - 1];
        if b < 0x80 {
            return !((0x61..=0x7A).contains(&b)
                || (0x30..=0x39).contains(&b)
                || (0x41..=0x5A).contains(&b));
        }
        let mut k = j - 1;
        while k > 0 && t[k] & 0xC0 == 0x80 {
            k -= 1;
        }
        match std::str::from_utf8(&t[k..j]).ok().and_then(|s| s.chars().next()) {
            Some(c) => !(c.is_alphabetic() || c.is_numeric()),
            None => true,
        }
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() {
            return true;
        }
        if needle.len() > hay.len() {
            return false;
        }
        hay.windows(needle.len()).any(|w| w == needle)
    }
}

/// Approximation of Foundation's `localizedStandardCompare` (Finder-style,
/// case-insensitive, number-aware, lowercase before uppercase on a tie).
pub fn localized_standard_compare(a: &str, b: &str) -> Ordering {
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    let primary = natural_ci(&ac, &bc);
    if primary != Ordering::Equal {
        return primary;
    }
    case_tiebreak(&ac, &bc)
}

fn natural_ci(a: &[char], b: &[char]) -> Ordering {
    let mut i = 0usize;
    let mut j = 0usize;
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let si = i;
            let sj = j;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let c = cmp_numeric(&a[si..i], &b[sj..j]);
            if c != Ordering::Equal {
                return c;
            }
            continue;
        }
        let la = a[i].to_lowercase().next().unwrap_or(a[i]);
        let lb = b[j].to_lowercase().next().unwrap_or(b[j]);
        let c = la.cmp(&lb);
        if c != Ordering::Equal {
            return c;
        }
        i += 1;
        j += 1;
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn cmp_numeric(a: &[char], b: &[char]) -> Ordering {
    let ta: Vec<char> = a.iter().copied().skip_while(|c| *c == '0').collect();
    let tb: Vec<char> = b.iter().copied().skip_while(|c| *c == '0').collect();
    if ta.len() != tb.len() {
        return ta.len().cmp(&tb.len());
    }
    ta.cmp(&tb)
}

fn case_tiebreak(a: &[char], b: &[char]) -> Ordering {
    let n = a.len().min(b.len());
    for k in 0..n {
        let ca = a[k];
        let cb = b[k];
        if ca == cb {
            continue;
        }
        let ua = ca.is_uppercase();
        let ub = cb.is_uppercase();
        if ua != ub {
            return if ua { Ordering::Greater } else { Ordering::Less };
        }
        let c = ca.cmp(&cb);
        if c != Ordering::Equal {
            return c;
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    // Reference: the Swift test's `OldFuzzy` (character matcher, length
    // tie-break) used as the parity oracle in `Tests/test_list_filter.swift`.
    fn old_tokens(query: &str) -> Vec<Vec<char>> {
        query
            .to_lowercase()
            .split_whitespace()
            .map(|s| s.chars().collect())
            .collect()
    }

    fn old_match_token(
        token: &[char],
        text: &[char],
    ) -> Option<(i64, usize, Vec<usize>)> {
        let len = token.len();
        let n = text.len();
        if len == 0 || len > n {
            return None;
        }
        let mut best_start: isize = -1;
        let mut best_score = i64::MIN;
        let mut i = 0usize;
        while i + len <= n {
            let equal = (0..len).all(|k| text[i + k] == token[k]);
            if equal {
                let is_word_start = i == 0 || {
                    let prev = text[i - 1];
                    !(prev.is_alphabetic() || prev.is_numeric())
                };
                let sc = (if is_word_start { BONUS_BOUNDARY_WHITE } else { 0 }) - i as i64 / 8;
                if sc > best_score {
                    best_score = sc;
                    best_start = i as isize;
                }
            }
            i += 1;
        }
        if best_start < 0 {
            return None;
        }
        let positions = (0..len).map(|k| best_start as usize + k).collect();
        Some((best_score + SCORE_MATCH * len as i64, len, positions))
    }

    fn old_fuzzy_filter(rows: &[usize], query: &str, texts: &[String]) -> Vec<usize> {
        let q = query.trim();
        if q.is_empty() {
            return rows.to_vec();
        }
        let ts = old_tokens(q);
        let cache: Vec<Vec<char>> = texts
            .iter()
            .map(|t| t.to_lowercase().chars().collect())
            .collect();
        let mut scored: Vec<(usize, usize, i64, i64)> = Vec::new();
        for &row in rows {
            let mut total = 0i64;
            let mut length = 0i64;
            let mut ok = true;
            for tok in &ts {
                match old_match_token(tok, &cache[row]) {
                    Some(m) => {
                        total += m.0;
                        length += m.1 as i64;
                    }
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                scored.push((scored.len(), row, total, length));
            }
        }
        scored.sort_by(|a, b| {
            if a.2 != b.2 {
                return b.2.cmp(&a.2);
            }
            if a.3 != b.3 {
                return a.3.cmp(&b.3);
            }
            a.0.cmp(&b.0)
        });
        scored.into_iter().map(|x| x.1).collect()
    }

    // ---- big-list parity: syntheticRows + keystroke queries ----

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) % n as u64) as usize
        }
    }

    const SEARCH_FIELDS: &[&str] = &[
        "key",
        "title",
        "status",
        "assignee",
        "reporter",
        "description",
        "labels",
        "priority",
        "releaseLabel",
        "project",
    ];

    fn search_text(d: &std::collections::HashMap<String, String>) -> String {
        SEARCH_FIELDS
            .iter()
            .filter_map(|f| {
                d.get(*f)
                    .map(|v| v.chars().take(150).collect::<String>())
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn synthetic_rows(n: usize) -> Vec<std::collections::HashMap<String, String>> {
        let words: Vec<&str> = ("lorem ipsum dolor sit amet payment processor invoice retry checkout service \
            gateway timeout login session token refresh cache layer report export import \
            dashboard widget mobile crash memory leak search index query slow api migrate \
            schema billing customer webhook queue worker deploy pipeline flaky test café \
            résumé naïve über straße")
            .split(' ')
            .collect();
        let people = ["Ana Lopez", "Bo Chen", "Cara Diaz", "Dev Patel", "Eli Novak", "Fay Ober", ""];
        let statuses = ["To Do", "In Progress", "In Review", "Done", "Blocked"];
        let projects = ["PAY", "WEB", "OPS", "MOB"];
        let mut r = Lcg(42);
        let mut rows = Vec::with_capacity(n);
        for i in 0..n {
            let sentence = |k: usize, r: &mut Lcg| {
                (0..k)
                    .map(|_| words[r.next(words.len())])
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let p = projects[r.next(4)];
            let title = sentence(4 + r.next(6), &mut r);
            let title = title
                .split(' ')
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            let status = statuses[r.next(statuses.len())];
            let assignee = people[r.next(people.len())];
            let reporter = people[r.next(people.len() - 1)];
            let description = sentence(r.next(200), &mut r);
            let labels = if r.next(3) == 0 {
                String::new()
            } else {
                words[r.next(words.len())].to_string()
            };
            let priority = format!("P{}", r.next(5));
            let release_label = if r.next(2) == 0 {
                String::new()
            } else {
                format!("{}.{}", r.next(20), r.next(9))
            };
            let updated = format!(
                "2026-0{}-1{}T10:0{}:00.000-0400",
                1 + r.next(9),
                r.next(10),
                r.next(10)
            );
            let mut m = std::collections::HashMap::new();
            m.insert("key".into(), format!("{}-{}", p, i + 1));
            m.insert("title".into(), title);
            m.insert("status".into(), status.into());
            m.insert("assignee".into(), assignee.into());
            m.insert("reporter".into(), reporter.into());
            m.insert("description".into(), description);
            m.insert("labels".into(), labels);
            m.insert("priority".into(), priority);
            m.insert("releaseLabel".into(), release_label);
            m.insert("project".into(), p.into());
            m.insert("updated".into(), updated);
            rows.push(m);
        }
        rows
    }

    fn reverse_for(c: Ordering, ascending: bool) -> Ordering {
        if ascending {
            c
        } else {
            c.reverse()
        }
    }
}
