//! Port of `VimSearch.swift`: case + diacritic insensitive matching, row and
//! text navigation, and Ctrl+W word dropping.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NsRange {
    pub location: usize,
    pub length: usize,
}

impl NsRange {
    pub fn new(location: usize, length: usize) -> NsRange {
        NsRange { location, length }
    }
}

fn is_combining(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x20D0..=0x20FF
        | 0xFE20..=0xFE2F)
}

fn strip_diacritic(c: char) -> Option<char> {
    let base = match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'a',
        'Ç' => 'c',
        'È' | 'É' | 'Ê' | 'Ë' => 'e',
        'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
        'Ñ' => 'n',
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'o',
        'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
        'Ý' => 'y',
        '\u{0101}' | '\u{0103}' | '\u{0105}' => 'a',
        '\u{0107}' | '\u{0109}' | '\u{010B}' | '\u{010D}' => 'c',
        '\u{010F}' => 'd',
        '\u{0113}' | '\u{0115}' | '\u{0117}' | '\u{0119}' | '\u{011B}' => 'e',
        '\u{011D}' | '\u{011F}' | '\u{0121}' | '\u{0123}' => 'g',
        '\u{0125}' => 'h',
        '\u{0129}' | '\u{012B}' | '\u{012D}' | '\u{012F}' | '\u{0131}' => 'i',
        '\u{0135}' => 'j',
        '\u{0137}' => 'k',
        '\u{013A}' | '\u{013C}' | '\u{013E}' => 'l',
        '\u{0144}' | '\u{0146}' | '\u{0148}' => 'n',
        '\u{014D}' | '\u{014F}' | '\u{0151}' => 'o',
        '\u{0155}' | '\u{0157}' | '\u{0159}' => 'r',
        '\u{015B}' | '\u{015D}' | '\u{015F}' | '\u{0161}' => 's',
        '\u{0163}' | '\u{0165}' => 't',
        '\u{0169}' | '\u{016B}' | '\u{016D}' | '\u{016F}' | '\u{0171}' | '\u{0173}' => 'u',
        '\u{0175}' => 'w',
        '\u{0177}' => 'y',
        '\u{017A}' | '\u{017C}' | '\u{017E}' => 'z',
        _ => return None,
    };
    Some(base)
}

fn fold_char(c: char) -> Vec<char> {
    let mut out = Vec::new();
    for lc in c.to_lowercase() {
        if is_combining(lc) {
            continue;
        }
        out.push(strip_diacritic(lc).unwrap_or(lc));
    }
    out
}

struct FoldText {
    folded: Vec<char>,
    folded_to_orig: Vec<usize>,
    orig_start_u16: Vec<usize>,
    orig_folded_start: Vec<usize>,
}

fn fold_text(s: &str) -> FoldText {
    let mut folded: Vec<char> = Vec::new();
    let mut folded_to_orig: Vec<usize> = Vec::new();
    let mut orig_start_u16: Vec<usize> = Vec::new();
    let mut orig_folded_start: Vec<usize> = Vec::new();
    let mut off = 0usize;
    for (ci, c) in s.chars().enumerate() {
        orig_start_u16.push(off);
        off += c.len_utf16();
        orig_folded_start.push(folded.len());
        for fc in fold_char(c) {
            folded.push(fc);
            folded_to_orig.push(ci);
        }
    }
    orig_start_u16.push(off);
    orig_folded_start.push(folded.len());
    FoldText { folded, folded_to_orig, orig_start_u16, orig_folded_start }
}

fn fold_query(query: &str) -> Vec<char> {
    let mut out = Vec::new();
    for c in query.chars() {
        out.extend(fold_char(c));
    }
    out
}

fn find_from(ft: &FoldText, fq: &[char], start_u16: usize) -> Option<NsRange> {
    let n = ft.orig_start_u16.len().saturating_sub(1);
    let mut ci = 0;
    while ci < n && ft.orig_start_u16[ci] < start_u16 {
        ci += 1;
    }
    let qlen = fq.len();
    let fcount = ft.folded.len();
    for c in ci..n {
        let fpos = ft.orig_folded_start[c];
        if fpos + qlen <= fcount && &ft.folded[fpos..fpos + qlen] == &fq[..] {
            let end_folded = fpos + qlen;
            let end_orig = ft.folded_to_orig[end_folded - 1] + 1;
            let start = ft.orig_start_u16[c];
            let end = ft.orig_start_u16[end_orig];
            return Some(NsRange::new(start, end - start));
        }
    }
    None
}

pub fn rows(
    texts: &[String],
    query: &str,
    from: i64,
    back: bool,
    skip_current: bool,
) -> (Option<usize>, usize, usize) {
    let n = texts.len();
    if n == 0 || query.is_empty() {
        return (None, 0, 0);
    }
    let hits: Vec<usize> = (0..n).filter(|&i| matches(&texts[i], query)).collect();
    if hits.is_empty() {
        return (None, 0, 0);
    }
    let start = (n as i64 - 1).min(from).max(0);
    let row = if back {
        hits.iter()
            .rev()
            .copied()
            .find(|&i| if skip_current { (i as i64) < start } else { (i as i64) <= start })
            .unwrap_or(*hits.last().unwrap())
    } else {
        hits.iter()
            .copied()
            .find(|&i| if skip_current { (i as i64) > start } else { (i as i64) >= start })
            .unwrap_or(*hits.first().unwrap())
    };
    let index = hits.iter().position(|&i| i == row).map(|p| p + 1).unwrap_or(1);
    (Some(row), index, hits.len())
}

pub fn matches(text: &str, query: &str) -> bool {
    if query.is_empty() {
        return false;
    }
    let ft = fold_text(text);
    let fq = fold_query(query);
    if fq.is_empty() {
        return false;
    }
    find_from(&ft, &fq, 0).is_some()
}

pub fn ranges(s: &str, query: &str, limit: usize) -> Vec<NsRange> {
    let ft = fold_text(s);
    let fq = fold_query(query);
    let total = *ft.orig_start_u16.last().unwrap_or(&0);
    if total == 0 || fq.is_empty() {
        return Vec::new();
    }
    let mut all: Vec<NsRange> = Vec::new();
    let mut at: usize = 0;
    while at < total && all.len() < limit {
        match find_from(&ft, &fq, at) {
            Some(r) => {
                all.push(r);
                at = r.location + r.length.max(1);
            }
            None => break,
        }
    }
    all
}

pub fn text(
    s: &str,
    query: &str,
    from: i64,
    back: bool,
    skip_current: bool,
) -> (Option<NsRange>, usize, usize) {
    let all = ranges(s, query, 5000);
    if all.is_empty() {
        return (None, 0, 0);
    }
    let total = s.encode_utf16().count() as i64;
    let start = total.min(from).max(0);
    let hit = if back {
        all.iter()
            .rev()
            .copied()
            .find(|r| if skip_current { (r.location as i64) < start } else { (r.location as i64) <= start })
            .unwrap_or(*all.last().unwrap())
    } else {
        all.iter()
            .copied()
            .find(|r| if skip_current { (r.location as i64) > start } else { (r.location as i64) >= start })
            .unwrap_or(*all.first().unwrap())
    };
    let index = all.iter().position(|r| r.location == hit.location).map(|p| p + 1).unwrap_or(1);
    (Some(hit), index, all.len())
}

pub fn drop_word(s: &str) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    while chars.last() == Some(&' ') {
        chars.pop();
    }
    while let Some(&c) = chars.last() {
        if c == ' ' {
            break;
        }
        chars.pop();
    }
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_swift_cases() {
        let rows_in: Vec<String> = ["KAN-3 Subtask", "KAN-2 Task 2", "SAM1-4 Billing", "kan-1 task 1", "Résumé draft"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mut r = rows(&rows_in, "kan", 0, false, false);
        assert!(r.0 == Some(0) && r.1 == 1 && r.2 == 3, "typing 'kan' on row 0 stays on row 0 (1/3), got {:?}", r);
        r = rows(&rows_in, "billing", 0, false, false);
        assert!(r.0 == Some(2) && r.2 == 1, "typing finds a later row, got {:?}", r);
        r = rows(&rows_in, "KAN-1", 0, false, false);
        assert!(r.0 == Some(3), "upper-case query matches lower-case row (no smartcase), got {:?}", r);
        r = rows(&rows_in, "resume", 0, false, false);
        assert!(r.0 == Some(4), "diacritics ignored, got {:?}", r);

        r = rows(&rows_in, "kan", 0, false, true);
        assert!(r.0 == Some(1) && r.1 == 2, "n from row 0 -> row 1 (2/3), got {:?}", r);
        r = rows(&rows_in, "kan", 3, false, true);
        assert!(r.0 == Some(0) && r.1 == 1, "n from the last match wraps to the first, got {:?}", r);
        r = rows(&rows_in, "kan", 3, true, true);
        assert!(r.0 == Some(1), "N from row 3 -> row 1, got {:?}", r);
        r = rows(&rows_in, "kan", 0, true, true);
        assert!(r.0 == Some(3), "N from the first match wraps to the last, got {:?}", r);
        r = rows(&rows_in, "billing", 2, false, true);
        assert!(r.0 == Some(2) && r.2 == 1, "n with one match stays on it, got {:?}", r);
        assert!(rows(&rows_in, "zzz", 0, false, false).0.is_none(), "no match -> nil");
        assert!(rows(&rows_in, "", 0, false, false).0.is_none(), "empty query -> nil");
        assert!(rows(&[], "a", 0, false, false).0.is_none(), "no rows -> nil");
        assert!(rows(&rows_in, "kan", 99, false, true).0 == Some(0), "a cursor past the end is clamped (wraps to the first)");

        let text_in = "alpha beta\nGamma beta\n🙂 beta end";
        let mut t = text(text_in, "BETA", 0, false, false);
        assert!(t.0 == Some(NsRange::new(6, 4)) && t.2 == 3 && t.1 == 1, "text: first 'beta', got {:?}", t);
        t = text(text_in, "beta", 6, false, true);
        assert!(t.0.map(|r| r.location) == Some(17) && t.1 == 2, "text n -> the second, got {:?}", t);
        t = text(text_in, "beta", 17, false, true);
        let third = {
            let all = ranges(text_in, "beta end", 5000);
            all[0].location
        };
        assert!(t.0.map(|r| r.location) == Some(third) && t.1 == 3, "text n past an emoji (UTF-16) -> the third, got {:?}", t);
        t = text(text_in, "beta", third as i64, false, true);
        assert!(t.0.map(|r| r.location) == Some(6), "text n wraps, got {:?}", t);
        t = text(text_in, "beta", 6, true, true);
        assert!(t.0.map(|r| r.location) == Some(third), "text N wraps backward, got {:?}", t);
        assert!(text(text_in, "nope", 0, false, false).0.is_none(), "text: no match");

        assert_eq!(drop_word("sprint board "), "sprint ", "Ctrl+W drops the last word + trailing spaces");
        assert_eq!(drop_word("sprint"), "", "Ctrl+W on one word clears");
        assert_eq!(drop_word(""), "", "Ctrl+W on empty");

        assert!(matches("Résumé draft", "resume"), "row match ignores case + accents");
        assert!(!matches("KAN-2", ""), "an empty query matches nothing");
        let all = ranges("Kan kan KAN", "kan", 5000);
        assert_eq!(
            all.iter().map(|r| r.location).collect::<Vec<_>>(),
            vec![0, 4, 8],
            "every text match in order, got {:?}",
            all
        );
        assert_eq!(
            ranges(&"a".repeat(50), "a", 10).len(),
            10,
            "ranges stop at the limit"
        );
    }
}
