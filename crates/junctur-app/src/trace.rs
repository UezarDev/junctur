//! Test mode's record of skipping: every position mpv reports while only the kept ranges play,
//! and, at report time, what happened at each range boundary crossed: which source frames were on
//! screen either side (they must be one range's last kept frame and the next range's first) and
//! how long playback stalled there. Off outside test mode.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{json, Value};

use crate::cuts::{Form, Timeline};
use crate::lock::lock;

struct Run {
    timeline: Arc<Timeline>,
    /// (wall seconds since the trace began, output seconds).
    points: Vec<(f64, f64)>,
}

pub struct Trace {
    began: Instant,
    runs: Mutex<Runs>,
}

/// About an hour of positions at 30 fps; a test run is seconds long, so this only bounds a
/// forgotten one.
const MAX_POINTS: usize = 108_000;

#[derive(Default)]
struct Runs {
    all: Vec<Run>,
    /// The playing load is kept ranges (positions are recorded).
    active: bool,
    points: usize,
    dropped: usize,
}

impl Trace {
    pub fn new() -> Self {
        Self {
            began: Instant::now(),
            runs: Mutex::new(Runs::default()),
        }
    }

    /// A load began: a new run when it plays kept ranges.
    pub fn start(&self, form: &Form) {
        let mut runs = lock(&self.runs);
        runs.active = matches!(form, Form::Cuts(_));
        if let Form::Cuts(t) = form {
            runs.all.push(Run {
                timeline: t.clone(),
                points: Vec::new(),
            });
        }
    }

    pub fn point(&self, output: f64) {
        let wall = self.began.elapsed().as_secs_f64();
        let mut runs = lock(&self.runs);
        if !runs.active {
            return;
        }
        if runs.points >= MAX_POINTS {
            runs.dropped += 1;
            return;
        }
        runs.points += 1;
        if let Some(run) = runs.all.last_mut() {
            run.points.push((wall, output));
        }
    }

    pub fn report(&self) -> Value {
        let runs = lock(&self.runs);
        let crossings: Vec<Value> = runs.all.iter().flat_map(crossings).collect();
        let jumps: Vec<Value> = runs.all.iter().flat_map(jumps).collect();
        let frames_ok = crossings.iter().filter(|c| c["framesOk"] == true).count();
        let max_stall = crossings
            .iter()
            .filter_map(|c| c["stallMs"].as_f64())
            .fold(0.0, f64::max);
        json!({
            "runs": runs.all.len(),
            "droppedPoints": runs.dropped,
            "crossed": crossings.len(),
            "framesOk": frames_ok,
            "maxStallMs": max_stall,
            "crossings": crossings,
            "jumps": jumps,
        })
    }
}

/// Two consecutive samples of normal playback: output moved forward by less than a seek would.
fn steady(a: (f64, f64), b: (f64, f64)) -> bool {
    (0.0..0.2).contains(&(b.1 - a.1))
}

/// Every seek within a run (output jumping other than forward by a little) with the positions
/// mpv reported just after it. A seek while skipping must not show a frame of a cut: in the EDL
/// such a frame would sit before its segment's start, so `inSegment` checks every position after
/// the jump against the segment where playback settles.
fn jumps(run: &Run) -> Vec<Value> {
    let t = &run.timeline;
    let p = &run.points;
    (1..p.len())
        .filter(|&k| !steady(p[k - 1], p[k]))
        .map(|k| {
            let window: Vec<f64> = p[k..p.len().min(k + 6)].iter().map(|q| q.1).collect();
            let settled = window.last().copied().unwrap_or(p[k].1);
            let start = t.segment_start(settled);
            json!({
                "positions": window,
                "segmentStart": start,
                "inSegment": window.iter().all(|&o| o >= start),
            })
        })
        .collect()
}

/// Boundaries crossed by normal playback within one run (a seek's jump is not a crossing), in one
/// pass over the points with a binary search per crossing.
fn crossings(run: &Run) -> Vec<Value> {
    let t = &run.timeline;
    let p = &run.points;
    let bounds = t.boundaries();
    let frame = |o: f64| t.fps.frame_at(t.to_source(o));
    let mut out = Vec::new();
    for k in 0..p.len().saturating_sub(1) {
        let (x, y) = (p[k], p[k + 1]);
        let j = bounds.partition_point(|&b| b <= x.1);
        if !steady(x, y) || j == bounds.len() || bounds[j] > y.1 {
            continue;
        }
        let b = bounds[j];
        // Stall: how much (wall - output) grew from just before the boundary to well after it.
        // It is constant while playback keeps real time. Both samples come from the same stretch
        // of playback (no seek in between), else there is none.
        let back = (1..=k)
            .rev()
            .take_while(|&i| steady(p[i - 1], p[i]))
            .map(|i| p[i - 1]);
        let before = std::iter::once(x).chain(back).find(|q| q.1 < b - 0.1);
        let ahead = (k + 2..p.len())
            .take_while(|&i| steady(p[i - 1], p[i]))
            .map(|i| p[i]);
        let after = std::iter::once(y).chain(ahead).find(|q| q.1 > b + 0.5);
        let stall = before
            .zip(after)
            .map(|(u, v)| (((v.0 - v.1) - (u.0 - u.1)) * 1000.0).round());
        let (from, to) = (frame(x.1), frame(y.1));
        let expect = (t.ranges[j].1 - 1, t.ranges[j + 1].0);
        out.push(json!({
            "output": b,
            "from": from,
            "to": to,
            "expect": [expect.0, expect.1],
            "framesOk": (from, to) == expect,
            "stallMs": stall,
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cuts::Fps;
    use junctur_core::edit::Range;

    #[test]
    fn reports_frames_and_stall_at_a_boundary() {
        let keep = [
            Range {
                start: 1.0,
                end: 2.0,
            },
            Range {
                start: 5.0,
                end: 7.0,
            },
        ];
        let fps = Fps { num: 30, den: 1 };
        let t = Arc::new(Timeline::new(&keep, 10.0, fps));
        let trace = Trace::new();
        trace.start(&Form::Cuts(t));
        // Output frames 0..90 at real time, with a 50 ms stall at the boundary (frame 30).
        let mut runs = lock(&trace.runs);
        let run = runs.all.last_mut().unwrap();
        for f in 0..90u64 {
            let o = fps.seconds(f) + 1e-4;
            let stall = if f >= 30 { 0.05 } else { 0.0 };
            run.points.push((fps.seconds(f) + stall, o));
        }
        drop(runs);
        let r = trace.report();
        assert_eq!(r["crossed"], 1);
        assert_eq!(r["framesOk"], 1);
        let c = &r["crossings"][0];
        assert_eq!(
            (c["from"].as_u64(), c["to"].as_u64()),
            (Some(59), Some(150))
        );
        assert_eq!(c["stallMs"].as_f64(), Some(50.0));
    }

    #[test]
    fn a_seek_right_after_a_boundary_gives_no_stall() {
        let keep = [
            Range {
                start: 1.0,
                end: 2.0,
            },
            Range {
                start: 5.0,
                end: 7.0,
            },
        ];
        let fps = Fps { num: 30, den: 1 };
        let trace = Trace::new();
        trace.start(&Form::Cuts(Arc::new(Timeline::new(&keep, 10.0, fps))));
        // Plays across the boundary (frame 30), then 0.2 s later seeks to the end.
        for f in 0..36u64 {
            trace.point(fps.seconds(f) + 1e-4);
        }
        trace.point(2.9);
        let r = trace.report();
        assert_eq!(r["crossed"], 1);
        assert!(r["crossings"][0]["stallMs"].is_null());
    }
}
