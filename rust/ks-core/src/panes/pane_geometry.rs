//! Port of `PaneGeometry.swift`: directional pane focus math.

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PaneDir {
    Left,
    Down,
    Up,
    Right,
}

impl PaneDir {
    pub const ALL: [PaneDir; 4] = [PaneDir::Left, PaneDir::Down, PaneDir::Up, PaneDir::Right];

    pub fn raw_value(self) -> char {
        match self {
            PaneDir::Left => 'h',
            PaneDir::Down => 'j',
            PaneDir::Up => 'k',
            PaneDir::Right => 'l',
        }
    }

    pub fn opposite(self) -> PaneDir {
        match self {
            PaneDir::Left => PaneDir::Right,
            PaneDir::Right => PaneDir::Left,
            PaneDir::Up => PaneDir::Down,
            PaneDir::Down => PaneDir::Up,
        }
    }

    pub fn vertical(self) -> bool {
        matches!(self, PaneDir::Up | PaneDir::Down)
    }

    pub fn from_raw(c: char) -> Option<PaneDir> {
        match c {
            'h' => Some(PaneDir::Left),
            'j' => Some(PaneDir::Down),
            'k' => Some(PaneDir::Up),
            'l' => Some(PaneDir::Right),
            _ => None,
        }
    }

    pub fn from_key_code(key_code: u16) -> Option<PaneDir> {
        match key_code {
            4 => Some(PaneDir::Left),
            38 => Some(PaneDir::Down),
            40 => Some(PaneDir::Up),
            37 => Some(PaneDir::Right),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect { x, y, width, height }
    }

    pub fn min_x(self) -> f64 {
        self.x.min(self.x + self.width)
    }
    pub fn max_x(self) -> f64 {
        self.x.max(self.x + self.width)
    }
    pub fn mid_x(self) -> f64 {
        self.x + self.width / 2.0
    }
    pub fn min_y(self) -> f64 {
        self.y.min(self.y + self.height)
    }
    pub fn max_y(self) -> f64 {
        self.y.max(self.y + self.height)
    }
    pub fn mid_y(self) -> f64 {
        self.y + self.height / 2.0
    }
}

#[derive(Clone, Debug)]
pub struct PaneRect {
    pub id: String,
    pub rect: Rect,
}

impl PaneRect {
    pub fn new(id: &str, rect: Rect) -> PaneRect {
        PaneRect { id: id.to_string(), rect }
    }
}

pub type Came = HashMap<String, HashMap<PaneDir, String>>;

pub const MIN_OVERLAP: f64 = 2.0;
pub const GAP_SLACK: f64 = 16.0;

struct Cand {
    id: String,
    gap: f64,
    overlap: f64,
    dist: f64,
}

pub fn next(from: &str, dir: PaneDir, panes: &[PaneRect], came: Option<&Came>) -> Option<String> {
    let cur = panes.iter().find(|p| p.id == from)?.rect;
    let mut cands: Vec<Cand> = Vec::new();
    for p in panes.iter().filter(|p| p.id != from) {
        let r = p.rect;
        let ahead = match dir {
            PaneDir::Right => r.mid_x() > cur.mid_x() && r.min_x() >= cur.mid_x(),
            PaneDir::Left => r.mid_x() < cur.mid_x() && r.max_x() <= cur.mid_x(),
            PaneDir::Down => r.mid_y() > cur.mid_y() && r.min_y() >= cur.mid_y(),
            PaneDir::Up => r.mid_y() < cur.mid_y() && r.max_y() <= cur.mid_y(),
        };
        if !ahead {
            continue;
        }
        let gap = match dir {
            PaneDir::Right => (r.min_x() - cur.max_x()).max(0.0),
            PaneDir::Left => (cur.min_x() - r.max_x()).max(0.0),
            PaneDir::Down => (r.min_y() - cur.max_y()).max(0.0),
            PaneDir::Up => (cur.min_y() - r.max_y()).max(0.0),
        };
        let overlap = if dir.vertical() {
            r.max_x().min(cur.max_x()) - r.min_x().max(cur.min_x())
        } else {
            r.max_y().min(cur.max_y()) - r.min_y().max(cur.min_y())
        };
        let dist = if dir.vertical() {
            (r.mid_x() - cur.mid_x()).abs()
        } else {
            (r.mid_y() - cur.mid_y()).abs()
        };
        cands.push(Cand { id: p.id.clone(), gap, overlap, dist });
    }

    let hits: Vec<&Cand> = cands.iter().filter(|c| c.overlap > MIN_OVERLAP).collect();

    if let Some(back) = came.and_then(|c| c.get(from)).and_then(|m| m.get(&dir)) {
        if hits.iter().any(|c| &c.id == back) {
            return Some(back.clone());
        }
    }

    fn bucket(g: f64) -> i64 {
        (g / GAP_SLACK).floor() as i64
    }
    fn less(a: &Cand, b: &Cand) -> bool {
        if bucket(a.gap) != bucket(b.gap) {
            return a.gap < b.gap;
        }
        if a.overlap != b.overlap {
            return a.overlap > b.overlap;
        }
        a.dist < b.dist
    }

    let mut best: Option<&Cand> = None;
    for c in hits {
        match best {
            None => best = Some(c),
            Some(b) => {
                if less(c, b) {
                    best = Some(c);
                }
            }
        }
    }
    best.map(|c| c.id.clone())
}

pub fn remember(came: &mut Came, from: &str, to: &str, dir: PaneDir) {
    came.entry(to.to_string())
        .or_default()
        .insert(dir.opposite(), from.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, x: f64, y: f64, w: f64, h: f64) -> PaneRect {
        PaneRect::new(id, Rect::new(x * 16.0, y * 10.0, w * 16.0, h * 10.0))
    }

    fn walk(start: &str, keys: &str, panes: &[PaneRect]) -> Vec<String> {
        let mut came: Came = HashMap::new();
        let mut at = start.to_string();
        let mut trail: Vec<String> = Vec::new();
        for k in keys.chars() {
            let d = match PaneDir::from_raw(k) {
                Some(d) => d,
                None => continue,
            };
            if let Some(to) = next(&at, d, panes, Some(&came)) {
                remember(&mut came, &at, &to, d);
                at = to;
            }
            trail.push(at.clone());
        }
        trail
    }

    fn expect(panes: &[PaneRect], from: &str, d: PaneDir, to: Option<&str>) {
        let got = next(from, d, panes, None);
        assert_eq!(
            got.as_deref(),
            to,
            "{} {} -> {:?}, got {:?}",
            from,
            d.raw_value(),
            to,
            got
        );
    }

    #[test]
    fn matches_swift_cases() {
        let notes = [
            pane("sidebar", 1.0, 9.0, 19.0, 90.0),
            pane("editor", 21.0, 9.0, 78.0, 44.0),
            pane("list", 21.0, 55.0, 41.0, 20.0),
            pane("preview", 63.0, 55.0, 36.0, 20.0),
            pane("terminal", 21.0, 77.0, 78.0, 22.0),
        ];
        expect(&notes, "editor", PaneDir::Left, Some("sidebar"));
        expect(&notes, "editor", PaneDir::Right, None);
        expect(&notes, "editor", PaneDir::Up, None);
        expect(&notes, "editor", PaneDir::Down, Some("list"));
        expect(&notes, "list", PaneDir::Right, Some("preview"));
        expect(&notes, "preview", PaneDir::Left, Some("list"));
        expect(&notes, "list", PaneDir::Down, Some("terminal"));
        expect(&notes, "preview", PaneDir::Down, Some("terminal"));
        expect(&notes, "terminal", PaneDir::Up, Some("list"));
        expect(&notes, "sidebar", PaneDir::Right, Some("editor"));
        expect(&notes, "terminal", PaneDir::Left, Some("sidebar"));
        expect(&notes, "sidebar", PaneDir::Left, None);
        assert_eq!(
            walk("terminal", "hl", &notes),
            vec!["sidebar".to_string(), "terminal".to_string()],
            "terminal h l returns to terminal"
        );
        assert_eq!(
            walk("preview", "kj", &notes),
            vec!["editor".to_string(), "preview".to_string()],
            "preview k j returns to the preview"
        );

        let files = [
            pane("sidebar", 1.0, 9.0, 19.0, 90.0),
            pane("filter", 21.0, 9.0, 78.0, 6.0),
            pane("list", 21.0, 17.0, 45.0, 82.0),
            pane("preview", 67.0, 17.0, 32.0, 82.0),
        ];
        expect(&files, "list", PaneDir::Up, Some("filter"));
        expect(&files, "preview", PaneDir::Up, Some("filter"));
        expect(&files, "filter", PaneDir::Down, Some("list"));
        expect(&files, "list", PaneDir::Right, Some("preview"));
        expect(&files, "list", PaneDir::Left, Some("sidebar"));
        expect(&files, "preview", PaneDir::Right, None);
        assert_eq!(
            walk("preview", "kj", &files),
            vec!["filter".to_string(), "preview".to_string()],
            "files preview k j retraces"
        );

        let jira = [
            pane("sidebar", 1.0, 9.0, 17.0, 90.0),
            pane("field", 19.0, 9.0, 80.0, 6.0),
            pane("rows", 19.0, 17.0, 52.0, 82.0),
            pane("issue", 72.0, 17.0, 27.0, 82.0),
        ];
        expect(&jira, "rows", PaneDir::Right, Some("issue"));
        expect(&jira, "issue", PaneDir::Left, Some("rows"));
        expect(&jira, "rows", PaneDir::Left, Some("sidebar"));
        expect(&jira, "rows", PaneDir::Up, Some("field"));
        expect(&jira, "field", PaneDir::Left, Some("sidebar"));

        let compare = [
            pane("sidebar", 1.0, 9.0, 17.0, 90.0),
            pane("left", 19.0, 9.0, 40.0, 90.0),
            pane("right", 59.0, 9.0, 40.0, 90.0),
        ];
        expect(&compare, "left", PaneDir::Right, Some("right"));
        expect(&compare, "right", PaneDir::Left, Some("left"));
        expect(&compare, "left", PaneDir::Left, Some("sidebar"));
        expect(&compare, "right", PaneDir::Down, None);
        expect(&compare, "sidebar", PaneDir::Right, Some("left"));

        let ai = [
            pane("rules", 1.0, 9.0, 17.0, 90.0),
            pane("input", 19.0, 9.0, 80.0, 40.0),
            pane("preview", 19.0, 51.0, 80.0, 48.0),
        ];
        expect(&ai, "input", PaneDir::Down, Some("preview"));
        expect(&ai, "preview", PaneDir::Up, Some("input"));
        expect(&ai, "preview", PaneDir::Left, Some("rules"));
        assert_eq!(
            walk("preview", "hl", &ai),
            vec!["rules".to_string(), "preview".to_string()],
            "AI preview h l comes back to the preview"
        );
        assert_eq!(
            walk("input", "hl", &ai),
            vec!["rules".to_string(), "input".to_string()],
            "AI input h l comes back to the input"
        );

        let notes_live = [
            PaneRect::new("sidebar", Rect::new(0.0, 30.0, 50.0, 774.0)),
            PaneRect::new("editor", Rect::new(62.0, 36.0, 1620.0, 764.0)),
            PaneRect::new("terminal", Rect::new(4.0, 808.0, 1682.0, 240.0)),
        ];
        expect(&notes_live, "terminal", PaneDir::Up, Some("editor"));
        expect(&notes_live, "sidebar", PaneDir::Down, Some("terminal"));
        expect(&notes_live, "editor", PaneDir::Left, Some("sidebar"));

        let diag = [pane("a", 0.0, 0.0, 10.0, 10.0), pane("b", 20.0, 20.0, 10.0, 10.0)];
        expect(&diag, "a", PaneDir::Right, None);
        expect(&diag, "a", PaneDir::Down, None);
        expect(&diag, "zz", PaneDir::Right, None);
    }
}
