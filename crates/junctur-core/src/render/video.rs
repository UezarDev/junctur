//! One ffmpeg run per [`Piece`]: a copy of the source packets between two keyframes, or an NVENC
//! encode of the frames around a cut, matched to the source so the joined stream stays one stream.
//!
//! Every piece is an MPEG-TS file whose timestamps already sit at its place in the output
//! (`-output_ts_offset`, no mux delay), so the pieces are joined by plain byte concatenation.
//! TS also repeats SPS/PPS before every IDR, which is what lets an NVENC piece follow an OBS piece:
//! their parameter sets differ, and MP4 pieces would carry them only out of band.

use std::path::Path;

use super::plan::Piece;
use super::source::{nvenc_profile, VideoInfo};

/// Output timestamps start here, so the first piece's B-frame lead-in (dts before pts) stays
/// positive; the final mux shifts the file back to zero.
const BASE: f64 = 1.0;

/// NVENC quality target, capped at 1.5x the source bitrate so cut edges do not bloat the file.
const CQ: &str = "19";
const FALLBACK_BITRATE: u64 = 8_000_000;

fn secs(frame: u64, v: &VideoInfo) -> f64 {
    frame as f64 * v.fps_den as f64 / v.fps_num as f64
}

/// The ffmpeg arguments that write `batch` of `src` to `out`: one piece, or several encode pieces
/// back to back in the output (see [`super::plan::batches`]). `matched` = the encode must match
/// the source's profile and level (smart render); without it (full re-encode) NVENC picks.
pub fn args(src: &Path, batch: &[Piece], v: &VideoInfo, matched: bool, out: &Path) -> Vec<String> {
    let p = &batch[0];
    let mut a: Vec<String> = vec!["-v".into(), "error".into(), "-nostdin".into(), "-y".into()];
    // Copy: seek straight to the keyframe. Encode: an accurate seek keeps the frame *showing* at
    // the seek time, so seeking a quarter frame into the first frame starts on it; at its exact
    // start, rounding could keep the frame before (half a frame early did, measured).
    let ss = if p.copy {
        secs(p.start, v)
    } else {
        secs(p.start, v) + 0.25 * secs(1, v)
    };
    if !p.copy && matched {
        // NVDEC: decoding on the GPU leaves the CPU free for the other pieces and the audio
        // (decoding on the CPU made a render CPU-bound); frames go straight to NVENC. Only for a
        // matched source: 8-bit 4:2:0 H.264, which NVDEC always takes. A full re-encode may be
        // fed anything, so it decodes on the CPU and converts.
        a.extend(["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"].map(String::from));
    }
    a.extend(["-ss".into(), format!("{ss:.6}"), "-i".into()]);
    a.push(src.to_string_lossy().into_owned());
    a.extend(["-map", "0:v:0", "-an", "-sn", "-dn"].map(String::from));
    if batch.len() > 1 {
        // Frame `n` after the seek is source frame `p.start + n`: keep each piece's frames, then
        // number them back to back. `select` and `setpts` only read timestamps, so CUDA frames
        // pass through untouched.
        let keep: Vec<String> = batch
            .iter()
            .map(|q| format!("between(n,{},{})", q.start - p.start, q.end - p.start - 1))
            .collect();
        a.push("-vf".into());
        a.push(format!(
            "select='{}',setpts=N*{}/{}/TB",
            keep.join("+"),
            v.fps_den,
            v.fps_num
        ));
    }
    a.push("-frames:v".into());
    a.push(batch.iter().map(Piece::frames).sum::<u64>().to_string());
    if p.copy {
        a.extend(["-c:v", "copy"].map(String::from));
    } else {
        encode_args(&mut a, v, matched);
    }
    a.extend(
        [
            "-muxdelay",
            "0",
            "-muxpreload",
            "0",
            "-avoid_negative_ts",
            "disabled",
            "-output_ts_offset",
        ]
        .map(String::from),
    );
    a.push(format!("{:.6}", BASE + secs(p.at, v)));
    a.extend(["-f", "mpegts"].map(String::from));
    a.push(out.to_string_lossy().into_owned());
    a
}

fn encode_args(a: &mut Vec<String>, v: &VideoInfo, matched: bool) {
    let rate = v.bit_rate.unwrap_or(FALLBACK_BITRATE);
    let mut push = |k: &str, val: String| {
        a.push(k.into());
        a.push(val);
    };
    push("-c:v", "h264_nvenc".into());
    push("-preset", "p5".into());
    push("-tune", "hq".into());
    push("-rc", "vbr".into());
    push("-cq", CQ.into());
    push("-b:v", rate.to_string());
    push("-maxrate", (rate * 3 / 2).to_string());
    push("-bufsize", (rate * 3).to_string());
    if !matched {
        push("-pix_fmt", "yuv420p".into());
    }
    push("-fps_mode", "passthrough".into());
    push("-bf", if v.b_frames { "2" } else { "0" }.into());
    if let Some(profile) = nvenc_profile(&v.profile) {
        push("-profile:v", profile.into());
    }
    if matched && v.level > 0 {
        push("-level", format!("{}.{}", v.level / 10, v.level % 10));
    }
    let tags = [
        ("-color_range", &v.color_range),
        ("-colorspace", &v.color_space),
        ("-color_trc", &v.color_transfer),
        ("-color_primaries", &v.color_primaries),
    ];
    for (k, val) in tags {
        if let Some(val) = val {
            push(k, val.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> VideoInfo {
        VideoInfo {
            codec: "h264".into(),
            profile: "Main".into(),
            level: 40,
            pix_fmt: "yuv420p".into(),
            color_range: Some("tv".into()),
            color_space: Some("bt709".into()),
            fps_num: 30,
            fps_den: 1,
            avg_num: 30,
            avg_den: 1,
            bit_rate: Some(5_000_000),
            b_frames: true,
            ..Default::default()
        }
    }

    fn after<'a>(a: &'a [String], flag: &str) -> Option<&'a str> {
        let i = a.iter().position(|x| x == flag)?;
        a.get(i + 1).map(String::as_str)
    }

    #[test]
    fn copy_piece_seeks_to_its_keyframe_and_lands_at_its_place() {
        let p = Piece {
            start: 120,
            end: 360,
            at: 20,
            copy: true,
        };
        let a = args(Path::new("in.mp4"), &[p], &info(), true, Path::new("o.ts"));
        assert_eq!(after(&a, "-ss"), Some("4.000000"));
        assert_eq!(after(&a, "-frames:v"), Some("240"));
        assert_eq!(after(&a, "-c:v"), Some("copy"));
        assert_eq!(after(&a, "-output_ts_offset"), Some("1.666667"));
        assert_eq!(a.last().map(String::as_str), Some("o.ts"));
    }

    #[test]
    fn encode_piece_matches_the_source() {
        let p = Piece {
            start: 100,
            end: 120,
            at: 0,
            copy: false,
        };
        let a = args(Path::new("in.mp4"), &[p], &info(), true, Path::new("o.ts"));
        assert_eq!(after(&a, "-ss"), Some("3.341667"));
        assert_eq!(after(&a, "-c:v"), Some("h264_nvenc"));
        assert_eq!(after(&a, "-profile:v"), Some("main"));
        assert_eq!(after(&a, "-level"), Some("4.0"));
        assert_eq!(after(&a, "-colorspace"), Some("bt709"));
        assert_eq!(
            after(&a, "-color_trc"),
            None,
            "unknown tags are not invented"
        );
        let full = args(Path::new("in.mp4"), &[p], &info(), false, Path::new("o.ts"));
        assert_eq!(after(&a, "-hwaccel"), Some("cuda"));
        assert_eq!(after(&full, "-level"), None);
        assert_eq!(after(&full, "-hwaccel"), None);
        assert_eq!(after(&full, "-pix_fmt"), Some("yuv420p"));
    }

    #[test]
    fn a_batch_selects_its_frames_and_numbers_them_back_to_back() {
        let ps = [
            Piece {
                start: 100,
                end: 120,
                at: 50,
                copy: false,
            },
            Piece {
                start: 150,
                end: 160,
                at: 70,
                copy: false,
            },
        ];
        let a = args(Path::new("in.mp4"), &ps, &info(), true, Path::new("o.ts"));
        assert_eq!(after(&a, "-ss"), Some("3.341667"));
        assert_eq!(
            after(&a, "-vf"),
            Some("select='between(n,0,19)+between(n,50,59)',setpts=N*1/30/TB")
        );
        assert_eq!(after(&a, "-frames:v"), Some("30"));
        assert_eq!(after(&a, "-output_ts_offset"), Some("2.666667"));
        // A single piece has no filter, as before batching.
        assert_eq!(
            after(
                &args(Path::new("i"), &ps[..1], &info(), true, Path::new("o")),
                "-vf"
            ),
            None
        );
    }
}
