//! What the renderer needs to know about the source: the video stream's encoding (to match it),
//! where the usable keyframes really are, and the audio tracks' rate, channels and bitrate.
//!
//! Keyframes are read per file, never assumed: OBS versions differ (2 s GOPs in one, irregular
//! ~8.3 s GOPs plus scene-cut keyframes in another).

use std::path::Path;

use serde_json::Value;

use crate::{ffmpeg, Error, Result};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoInfo {
    pub codec: String,
    pub profile: String,
    /// ffprobe's form: 40 = level 4.0.
    pub level: i64,
    pub pix_fmt: String,
    pub field_order: String,
    pub color_range: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub fps_num: u64,
    pub fps_den: u64,
    /// Average rate; differs from `fps_*` (the stream's base rate) for variable frame rate.
    pub avg_num: u64,
    pub avg_den: u64,
    pub bit_rate: Option<u64>,
    pub b_frames: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioTrack {
    pub rate: u32,
    pub channels: u32,
    pub bit_rate: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Source {
    pub video: VideoInfo,
    /// Frame count; frames sit at `k * fps_den / fps_num` s.
    pub frames: u64,
    /// Keyframes the stream can be cut at by copying (closed GOP), as frame numbers.
    pub keyframes: Vec<u64>,
    /// Why frame times are not on the constant grid, if they are not.
    pub grid_problem: Option<String>,
    pub audio: Vec<AudioTrack>,
}

impl Source {
    /// Why the cut pieces cannot be encoded to splice into this stream, if they cannot.
    pub fn mismatch(&self) -> Option<String> {
        let v = &self.video;
        if v.codec != "h264" {
            return Some(format!("the video is {}, not H.264", v.codec));
        }
        if nvenc_profile(&v.profile).is_none() {
            return Some(format!("H.264 profile {} has no NVENC match", v.profile));
        }
        if v.pix_fmt != "yuv420p" {
            return Some(format!(
                "pixel format {} (NVENC H.264 writes yuv420p)",
                v.pix_fmt
            ));
        }
        if !matches!(v.field_order.as_str(), "progressive" | "unknown" | "") {
            return Some(format!("interlaced video ({})", v.field_order));
        }
        if v.fps_num * v.avg_den != v.avg_num * v.fps_den {
            return Some("variable frame rate".into());
        }
        if let Some(p) = &self.grid_problem {
            return Some(p.clone());
        }
        if self.keyframes.is_empty() {
            return Some("no keyframe the stream can be cut at".into());
        }
        None
    }
}

/// The `-profile:v` value for h264_nvenc.
pub fn nvenc_profile(profile: &str) -> Option<&'static str> {
    match profile {
        "Main" => Some("main"),
        "High" => Some("high"),
        "Baseline" | "Constrained Baseline" => Some("baseline"),
        _ => None,
    }
}

pub fn inspect(file: &Path) -> Result<Source> {
    let path = file.to_string_lossy();
    let streams = ffmpeg::ffprobe(&[
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_streams",
        &path,
    ])?;
    let (video, audio) = parse_streams(&streams)?;
    // One pass over the packet index: pts and flags in decode order (~7 s for 1h45 of 1080p30).
    let packets = ffmpeg::ffprobe(&[
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "packet=pts_time,flags",
        "-of",
        "csv=p=0",
        &path,
    ])?;
    let packets = parse_packets(&packets, video.fps_num, video.fps_den);
    let (frames, grid_problem) = check_grid(&packets);
    Ok(Source {
        keyframes: usable_keyframes(&packets),
        video,
        frames,
        grid_problem,
        audio,
    })
}

fn rate(s: &Value) -> Option<(u64, u64)> {
    let (n, d) = s.as_str()?.split_once('/')?;
    let (n, d) = (n.parse().ok()?, d.parse().ok()?);
    (n > 0 && d > 0).then_some((n, d))
}

fn num(x: &Value) -> Option<u64> {
    x.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| x.as_u64())
}

fn tag(x: &Value) -> Option<String> {
    x.as_str()
        .filter(|s| !s.is_empty() && *s != "unknown")
        .map(str::to_string)
}

fn parse_streams(json: &[u8]) -> Result<(VideoInfo, Vec<AudioTrack>)> {
    let v: Value = serde_json::from_slice(json)?;
    let streams = v["streams"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let s = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .ok_or_else(|| Error::Render("the source has no video stream".into()))?;
    let (fps_num, fps_den) = rate(&s["r_frame_rate"])
        .ok_or_else(|| Error::Probe(format!("frame rate {}", s["r_frame_rate"])))?;
    let (avg_num, avg_den) = rate(&s["avg_frame_rate"]).unwrap_or((fps_num, fps_den));
    let text = |k: &str| s[k].as_str().unwrap_or("").to_string();
    let video = VideoInfo {
        codec: text("codec_name"),
        profile: text("profile"),
        level: s["level"].as_i64().unwrap_or(0),
        pix_fmt: text("pix_fmt"),
        field_order: text("field_order"),
        color_range: tag(&s["color_range"]),
        color_space: tag(&s["color_space"]),
        color_transfer: tag(&s["color_transfer"]),
        color_primaries: tag(&s["color_primaries"]),
        fps_num,
        fps_den,
        avg_num,
        avg_den,
        bit_rate: num(&s["bit_rate"]),
        b_frames: s["has_b_frames"].as_u64().unwrap_or(0) > 0,
    };
    let audio = streams
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .map(|s| AudioTrack {
            rate: num(&s["sample_rate"]).unwrap_or(48_000) as u32,
            channels: s["channels"].as_u64().unwrap_or(2) as u32,
            bit_rate: num(&s["bit_rate"]),
        })
        .collect();
    Ok((video, audio))
}

/// One video packet in decode order: its frame number (pts on the frame grid) and keyframe flag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Packet {
    pub frame: i64,
    pub key: bool,
}

fn parse_packets(csv: &[u8], fps_num: u64, fps_den: u64) -> Vec<Packet> {
    String::from_utf8_lossy(csv)
        .lines()
        .filter_map(|l| {
            let (pts, flags) = l.trim().split_once(',')?;
            let t: f64 = pts.parse().ok()?;
            Some(Packet {
                frame: (t * fps_num as f64 / fps_den as f64).round() as i64,
                key: flags.starts_with('K'),
            })
        })
        .collect()
}

/// Frame count, and a problem if the frames are not exactly 0..N on the grid (gaps, duplicates,
/// off-grid times): the plan's frame arithmetic would then not match the stream.
fn check_grid(packets: &[Packet]) -> (u64, Option<String>) {
    let mut frames: Vec<i64> = packets.iter().map(|p| p.frame).collect();
    frames.sort_unstable();
    let n = frames.len() as u64;
    let on_grid = frames.iter().enumerate().all(|(i, &f)| f == i as i64);
    let problem = (!on_grid).then(|| "frame times are not on a constant frame grid".to_string());
    (n, problem)
}

/// Keyframes where copying can start or stop: nothing decoded before it shows after it, and
/// nothing decoded after it shows before it. Open-GOP keyframes fail this and are skipped.
pub fn usable_keyframes(packets: &[Packet]) -> Vec<u64> {
    let mut out = Vec::new();
    let mut max_before = i64::MIN;
    for (i, p) in packets.iter().enumerate() {
        if p.key && max_before < p.frame {
            let gop_end = packets[i + 1..]
                .iter()
                .position(|q| q.key)
                .map_or(packets.len(), |k| i + 1 + k);
            if packets[i..gop_end].iter().all(|q| q.frame >= p.frame) {
                out.push(p.frame as u64);
            }
        }
        max_before = max_before.max(p.frame);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(frames: &[(i64, bool)]) -> Vec<Packet> {
        frames
            .iter()
            .map(|&(frame, key)| Packet { frame, key })
            .collect()
    }

    #[test]
    fn closed_gop_keyframes_are_usable() {
        // OBS order: I0 P3 B1 B2 | I4 P6 B5 (closed: I4 follows every frame before it).
        let p = pk(&[
            (0, true),
            (3, false),
            (1, false),
            (2, false),
            (4, true),
            (6, false),
            (5, false),
        ]);
        assert_eq!(usable_keyframes(&p), vec![0, 4]);
    }

    #[test]
    fn open_gop_keyframes_are_not() {
        // I3 is decoded before B1 B2, which show before it: copying from I3 would drop them.
        let p = pk(&[(0, true), (3, true), (1, false), (2, false), (4, false)]);
        assert_eq!(usable_keyframes(&p), vec![0]);
    }

    #[test]
    fn grid_and_packet_parsing() {
        let p = parse_packets(
            b"0.000000,K__\n0.100000,___\n0.033333,___\nN/A,___\n",
            30,
            1,
        );
        assert_eq!(p.iter().map(|p| p.frame).collect::<Vec<_>>(), vec![0, 3, 1]);
        assert!(p[0].key && !p[1].key);
        assert_eq!(check_grid(&p).0, 3);
        assert!(check_grid(&p).1.is_some(), "frame 2 missing");
        assert!(check_grid(&pk(&[(0, true), (2, false), (1, false)]))
            .1
            .is_none());
    }

    #[test]
    fn mismatch_reasons() {
        let json = br#"{"streams":[
            {"codec_type":"video","codec_name":"h264","profile":"High","level":40,"pix_fmt":"yuv420p",
             "field_order":"progressive","color_range":"tv","color_space":"bt709","r_frame_rate":"30/1",
             "avg_frame_rate":"30/1","bit_rate":"5000000","has_b_frames":1},
            {"codec_type":"audio","sample_rate":"48000","channels":2,"bit_rate":"160000"}]}"#;
        let (video, audio) = parse_streams(json).unwrap();
        assert_eq!(audio[0].bit_rate, Some(160_000));
        assert_eq!(video.color_transfer, None);
        let mut s = Source {
            video,
            frames: 10,
            keyframes: vec![0],
            grid_problem: None,
            audio,
        };
        assert_eq!(s.mismatch(), None);
        s.video.avg_num = 29;
        assert_eq!(s.mismatch().as_deref(), Some("variable frame rate"));
        s.video.avg_num = 30;
        s.video.profile = "High 10".into();
        assert!(s.mismatch().unwrap().contains("High 10"));
        s.video.codec = "hevc".into();
        assert!(s.mismatch().unwrap().contains("hevc"));
        assert!(parse_streams(br#"{"streams":[]}"#).is_err());
    }
}
