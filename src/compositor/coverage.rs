//! Whether windows cover an output completely: the geometry, without a
//! compositor.

/// A rectangle in the compositor's layout coordinates (logical pixels; they
/// may be fractional for a scaled output).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn right(&self) -> f64 {
        self.x + self.width
    }

    fn bottom(&self) -> f64 {
        self.y + self.height
    }

    fn contains_point(&self, x: f64, y: f64) -> bool {
        self.x <= x && x < self.right() && self.y <= y && y < self.bottom()
    }
}

/// Strips narrower than this many logical pixels are not counted as
/// uncovered: a scaled output's logical size can be fractional where window
/// positions are whole numbers.
const SLIVER: f64 = 1.0;

/// Whether the union of `windows` covers all of `area`. Windows outside the
/// area count for the part inside it. An empty area is covered by anything; a
/// rectangle with a size that is not a positive number covers nothing.
pub fn covers(area: Rect, windows: &[Rect]) -> bool {
    if !(area.width > 0.0 && area.height > 0.0) {
        return true;
    }
    let windows: Vec<Rect> = windows
        .iter()
        .filter(|w| w.width > 0.0 && w.height > 0.0 && w.width.is_finite() && w.height.is_finite())
        .copied()
        .collect();

    // The edges of every window inside the area cut it into cells, each of
    // which is either inside some window or not at all.
    let cut = |start: f64, end: f64, edges: &mut dyn Iterator<Item = f64>| {
        let mut cuts: Vec<f64> = edges.filter(|e| start < *e && *e < end).collect();
        cuts.push(start);
        cuts.push(end);
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        cuts
    };
    let xs = cut(
        area.x,
        area.right(),
        &mut windows.iter().flat_map(|w| [w.x, w.right()]),
    );
    let ys = cut(
        area.y,
        area.bottom(),
        &mut windows.iter().flat_map(|w| [w.y, w.bottom()]),
    );

    for column in xs.windows(2) {
        let (left, right) = (column[0], column[1]);
        if right - left < SLIVER {
            continue;
        }
        for row in ys.windows(2) {
            let (top, bottom) = (row[0], row[1]);
            if bottom - top < SLIVER {
                continue;
            }
            let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);
            if !windows.iter().any(|w| w.contains_point(x, y)) {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const HD: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    #[test]
    fn nothing_covers_nothing_and_one_full_window_covers_all() {
        assert!(!covers(HD, &[]));
        assert!(covers(HD, &[HD]));
        // A window larger than the output, and one on another output.
        assert!(covers(HD, &[Rect::new(-10.0, -10.0, 2000.0, 1200.0)]));
        assert!(!covers(HD, &[Rect::new(1920.0, 0.0, 1920.0, 1080.0)]));
    }

    #[test]
    fn tiled_windows_without_gaps_cover_and_with_gaps_do_not() {
        let halves = [
            Rect::new(0.0, 0.0, 960.0, 1080.0),
            Rect::new(960.0, 0.0, 960.0, 1080.0),
        ];
        assert!(covers(HD, &halves));
        // Hyprland's default: 5 px between windows, 20 px around them.
        let gapped = [
            Rect::new(20.0, 20.0, 935.0, 1040.0),
            Rect::new(965.0, 20.0, 935.0, 1040.0),
        ];
        assert!(!covers(HD, &gapped));
        // A bar reserves the top: a maximised window leaves it uncovered.
        assert!(!covers(HD, &[Rect::new(0.0, 24.0, 1920.0, 1056.0)]));
    }

    #[test]
    fn overlapping_windows_and_holes_between_them() {
        // Four quadrants that overlap: covered.
        let quadrants = [
            Rect::new(0.0, 0.0, 1000.0, 600.0),
            Rect::new(900.0, 0.0, 1020.0, 600.0),
            Rect::new(0.0, 500.0, 1000.0, 580.0),
            Rect::new(900.0, 500.0, 1020.0, 580.0),
        ];
        assert!(covers(HD, &quadrants));
        // A ring of four windows with a hole in the middle.
        let ring = [
            Rect::new(0.0, 0.0, 1920.0, 400.0),
            Rect::new(0.0, 680.0, 1920.0, 400.0),
            Rect::new(0.0, 400.0, 800.0, 280.0),
            Rect::new(1120.0, 400.0, 800.0, 280.0),
        ];
        assert!(!covers(HD, &ring));
    }

    #[test]
    fn a_fractional_output_size_is_covered_by_whole_pixels() {
        // 2560x1600 at scale 1.5 is 1706.67 x 1066.67 logical pixels.
        let area = Rect::new(1920.0, 0.0, 2560.0 / 1.5, 1600.0 / 1.5);
        assert!(covers(area, &[Rect::new(1920.0, 0.0, 1706.0, 1066.0)]));
        // A real strip is not a sliver.
        assert!(!covers(area, &[Rect::new(1920.0, 0.0, 1700.0, 1066.0)]));
    }

    #[test]
    fn rectangles_without_a_size_cover_nothing() {
        let broken = [
            Rect::new(0.0, 0.0, 0.0, 1080.0),
            Rect::new(0.0, 0.0, -1920.0, 1080.0),
            Rect::new(0.0, 0.0, f64::NAN, 1080.0),
            Rect::new(0.0, 0.0, f64::INFINITY, 1080.0),
        ];
        assert!(!covers(HD, &broken));
        assert!(covers(Rect::new(0.0, 0.0, 0.0, 0.0), &[]));
    }
}
