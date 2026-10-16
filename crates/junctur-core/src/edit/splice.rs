//! Hand edits to the kept ranges: cut a span out, or keep a span. Results sit on the file's
//! millisecond grid, so the list in memory is exactly what a re-read of the file gives.

use super::Range;

/// The file writes seconds with three decimals.
fn ms(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// `keep` without `[a, b]`.
pub fn cut_span(keep: &[Range], a: f64, b: f64, join: f64) -> Vec<Range> {
    let (a, b) = (ms(a), ms(b));
    let mut out = Vec::with_capacity(keep.len() + 1);
    for r in keep {
        if r.end <= a || r.start >= b {
            out.push(*r);
            continue;
        }
        if r.start < a {
            out.push(Range {
                start: r.start,
                end: a,
            });
        }
        if r.end > b {
            out.push(Range {
                start: b,
                end: r.end,
            });
        }
    }
    normalize(out, join)
}

/// `keep` with `[a, b]` added.
pub fn keep_span(keep: &[Range], a: f64, b: f64, join: f64) -> Vec<Range> {
    let mut out = keep.to_vec();
    out.push(Range {
        start: ms(a),
        end: ms(b),
    });
    out.sort_by(|x, y| x.start.total_cmp(&y.start));
    normalize(out, join)
}

/// What sorted `keep` keeps of `[a, b]`, clipped to it: what an accepted suggestion stores so its
/// undo puts back exactly that (7a's moments, 7b's like-this edits).
pub fn kept_in(keep: &[Range], a: f64, b: f64) -> Vec<Range> {
    let from = keep.partition_point(|r| r.end <= a);
    keep[from..]
        .iter()
        .take_while(|r| r.start < b)
        .map(|r| Range {
            start: r.start.max(a),
            end: r.end.min(b),
        })
        .collect()
}

/// `keep` with what it keeps inside `[a, b]` replaced by what `fresh` keeps there (a re-cut of only
/// that stretch): `keep` outside it as it was, `fresh` clipped to it.
pub fn replace_span(keep: &[Range], fresh: &[Range], a: f64, b: f64) -> Vec<Range> {
    let (a, b) = (ms(a), ms(b));
    let mut out = cut_span(keep, a, b, 0.0);
    out.extend(
        fresh
            .iter()
            .map(|r| Range {
                start: r.start.max(a),
                end: r.end.min(b),
            })
            .filter(|r| r.end > r.start),
    );
    out.sort_by(|x, y| x.start.total_cmp(&y.start));
    normalize(out, 0.0)
}

/// Sorted input to the file's form: on the millisecond grid, no empty ranges (the parser rejects
/// `end <= start`), and ranges less than `join` apart merged.
fn normalize(sorted: Vec<Range>, join: f64) -> Vec<Range> {
    let mut out: Vec<Range> = Vec::with_capacity(sorted.len());
    for r in sorted {
        let r = Range {
            start: ms(r.start).max(0.0),
            end: ms(r.end),
        };
        if r.end <= r.start {
            continue;
        }
        crate::spans::push_joined(&mut out, r, crate::spans::Join::Within(join.max(1e-9)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: f64, end: f64) -> Range {
        Range { start, end }
    }

    #[test]
    fn a_stretch_is_cut_again_and_the_rest_left() {
        let keep = [r(1.0, 5.0), r(8.0, 10.0)];
        let fresh = [r(0.5, 3.0), r(4.0, 9.0)];
        // Inside 2..6 the fresh cut's ranges, clipped; outside it the user's, joined where they
        // touch.
        assert_eq!(
            replace_span(&keep, &fresh, 2.0, 6.0),
            [r(1.0, 3.0), r(4.0, 6.0), r(8.0, 10.0)]
        );
        // Nothing fresh in the stretch: it is cut there.
        assert_eq!(
            replace_span(&keep, &[], 2.0, 9.0),
            [r(1.0, 2.0), r(9.0, 10.0)]
        );
        // The whole recording: the fresh cut.
        assert_eq!(replace_span(&keep, &fresh, 0.0, 10.0), fresh);
    }

    #[test]
    fn cuts_split_trim_and_drop() {
        let keep = [r(1.0, 5.0), r(8.0, 10.0)];
        assert_eq!(
            cut_span(&keep, 2.0, 3.0, 0.0),
            [r(1.0, 2.0), r(3.0, 5.0), r(8.0, 10.0)]
        );
        assert_eq!(cut_span(&keep, 4.0, 9.0, 0.0), [r(1.0, 4.0), r(9.0, 10.0)]);
        assert_eq!(cut_span(&keep, 0.0, 20.0, 0.0), []);
        assert_eq!(
            cut_span(&keep, 6.0, 7.0, 0.0),
            keep,
            "a cut of nothing kept"
        );
        // A sliver under a millisecond would be written as `keep x x`, which does not parse.
        assert_eq!(cut_span(&keep, 1.0004, 5.0, 0.0), [r(8.0, 10.0)]);
    }

    #[test]
    fn keeps_merge_with_neighbours() {
        let keep = [r(1.0, 2.0), r(5.0, 6.0)];
        assert_eq!(keep_span(&keep, 2.0, 5.0, 0.0), [r(1.0, 6.0)]);
        assert_eq!(
            keep_span(&keep, 3.0, 4.0, 0.0),
            [r(1.0, 2.0), r(3.0, 4.0), r(5.0, 6.0)]
        );
        // Two ends rounded from one frame boundary are a millisecond apart: joined.
        assert_eq!(
            keep_span(&keep, 2.001, 3.0, 0.02),
            [r(1.0, 3.0), r(5.0, 6.0)]
        );
        assert_eq!(keep_span(&[], 0.12345, 0.5, 0.0), [r(0.123, 0.5)]);
    }

    #[test]
    fn results_survive_the_file() {
        let list = super::super::EditList {
            source: "x.mp4".into(),
            duration: 20.0,
            settings: Default::default(),
            keep: cut_span(&[r(0.0, 20.0)], 1.23456, 7.891011, 0.0),
            speakers: Default::default(),
            captions: Default::default(),
            fixes: Vec::new(),
            times: Vec::new(),
            windows: vec![],
        };
        assert_eq!(super::super::parse(&list.to_text()).unwrap(), list);
    }
}
