//! Sampling a few spread-out windows, so cost depends on the window count, not the file length.

/// Spread `n` windows of `len` seconds evenly over `duration`. A file
/// too short to hold them all is read whole as one window.
pub fn window_starts(duration: f64, n: usize, len: f64) -> (Vec<f64>, f64) {
    if n == 0 || duration <= n as f64 * len {
        return (vec![0.0], duration.max(0.0));
    }
    let slice = duration / n as f64;
    let starts = (0..n)
        .map(|i| (slice * (i as f64 + 0.5) - len / 2.0).max(0.0))
        .collect();
    (starts, len)
}

/// Cut every track's copy of each window to the shortest one and concatenate, so sample `i` means
/// the same instant on every track (tracks can end a few ms apart).
pub(super) fn align(per_track: Vec<Vec<Vec<f32>>>, windows: usize) -> Vec<Vec<f32>> {
    let lens: Vec<usize> = (0..windows)
        .map(|w| per_track.iter().map(|t| t[w].len()).min().unwrap_or(0))
        .collect();
    per_track
        .into_iter()
        .map(|t| {
            t.into_iter()
                .zip(&lens)
                .flat_map(|(w, &n)| w.into_iter().take(n))
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_spread_and_short_files_read_whole() {
        let (s, len) = window_starts(800.0, 8, 10.0);
        assert_eq!(s.len(), 8);
        assert_eq!(s[0], 45.0);
        assert_eq!(len, 10.0);
        assert_eq!(window_starts(30.0, 8, 10.0), (vec![0.0], 30.0));
    }
}
