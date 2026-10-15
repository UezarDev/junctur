//! What a file holds, from ffprobe: audio streams, the first video stream, and the queries the
//! renderer needs (every stream's fields, the video packet index), each parsed here so no caller
//! spells out ffprobe arguments.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::run;
use crate::{Error, Result};

/// One audio stream as ffprobe reports it. `index` is the audio ordinal (0 = first audio track),
/// which is what `-map 0:a:N` takes; the report shows it 1-based as OBS does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioStream {
    pub index: usize,
    pub codec: String,
    pub channels: u32,
    pub sample_rate: u32,
    pub duration: Option<f64>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    pub duration: f64,
    pub audio: Vec<AudioStream>,
}

/// The first video stream: size and frame rate (as a fraction, e.g. 30000/1001).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VideoStream {
    pub width: u32,
    pub height: u32,
    pub fps_num: u64,
    pub fps_den: u64,
}

fn ffprobe(file: &Path, query: &[&str]) -> Result<Vec<u8>> {
    let path = file.to_string_lossy();
    let mut args = vec!["-v", "error"];
    args.extend_from_slice(query);
    args.push(&path);
    run("ffprobe", &args)
}

pub fn probe(file: &Path) -> Result<Probe> {
    let out = ffprobe(
        file,
        &[
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-select_streams",
            "a",
        ],
    )?;
    parse_probe(&out)
}

/// None for an audio-only file.
pub fn probe_video(file: &Path) -> Result<Option<VideoStream>> {
    let out = ffprobe(
        file,
        &[
            "-print_format",
            "json",
            "-show_streams",
            "-select_streams",
            "v:0",
        ],
    )?;
    parse_video(&out)
}

/// Every stream's fields as ffprobe gives them, for a caller that reads more than [`probe`] keeps
/// (the renderer matches profile, level, colour tags).
pub fn streams(file: &Path) -> Result<Vec<Value>> {
    let out = ffprobe(file, &["-print_format", "json", "-show_streams"])?;
    let v: Value = serde_json::from_slice(&out)?;
    Ok(v["streams"].as_array().cloned().unwrap_or_default())
}

/// One packet of the first video stream: its presentation time and whether it is a keyframe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PacketTime {
    pub pts: f64,
    pub key: bool,
}

/// The first video stream's packets in decode order, in one pass over the packet index; packets
/// without a time are skipped.
pub fn video_packets(file: &Path) -> Result<Vec<PacketTime>> {
    let out = ffprobe(
        file,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time,flags",
            "-of",
            "csv=p=0",
        ],
    )?;
    Ok(parse_packets(&out))
}

fn parse_packets(csv: &[u8]) -> Vec<PacketTime> {
    String::from_utf8_lossy(csv)
        .lines()
        .filter_map(|l| {
            let (pts, flags) = l.trim().split_once(',')?;
            Some(PacketTime {
                pts: pts.parse().ok()?,
                key: flags.starts_with('K'),
            })
        })
        .collect()
}

/// A rate as ffprobe writes it (`"30000/1001"`), both parts positive.
pub fn rate(x: &Value) -> Option<(u64, u64)> {
    let (n, d) = x.as_str()?.split_once('/')?;
    let (n, d) = (n.parse().ok()?, d.parse().ok()?);
    (n > 0 && d > 0).then_some((n, d))
}

/// A number ffprobe may write as a string (`"48000"`) or as a number.
pub fn num<T: std::str::FromStr + TryFrom<u64>>(x: &Value) -> Option<T> {
    x.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| x.as_u64().and_then(|n| T::try_from(n).ok()))
}

fn seconds(x: &Value) -> Option<f64> {
    x.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| x.as_f64())
}

fn parse_probe(json: &[u8]) -> Result<Probe> {
    let v: Value = serde_json::from_slice(json)?;
    let duration = seconds(&v["format"]["duration"])
        .ok_or_else(|| Error::Probe("no format duration".into()))?;
    let audio = v["streams"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, s)| AudioStream {
            index: i,
            codec: s["codec_name"].as_str().unwrap_or("unknown").to_string(),
            channels: s["channels"].as_u64().unwrap_or(0) as u32,
            sample_rate: seconds(&s["sample_rate"]).unwrap_or(0.0) as u32,
            duration: seconds(&s["duration"]),
            title: s["tags"]["title"].as_str().map(str::to_string),
        })
        .collect();
    Ok(Probe { duration, audio })
}

fn parse_video(json: &[u8]) -> Result<Option<VideoStream>> {
    let v: Value = serde_json::from_slice(json)?;
    let Some(s) = v["streams"].as_array().and_then(|a| a.first()) else {
        return Ok(None);
    };
    let (n, d) = rate(&s["r_frame_rate"])
        .ok_or_else(|| Error::Probe(format!("frame rate {}", s["r_frame_rate"])))?;
    Ok(Some(VideoStream {
        width: s["width"].as_u64().unwrap_or(0) as u32,
        height: s["height"].as_u64().unwrap_or(0) as u32,
        fps_num: n,
        fps_den: d,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_video_stream() {
        let j = br#"{"streams":[{"width":1920,"height":1080,"r_frame_rate":"30000/1001"}]}"#;
        let v = parse_video(j).unwrap().unwrap();
        assert_eq!((v.width, v.fps_num, v.fps_den), (1920, 30000, 1001));
        assert_eq!(parse_video(br#"{"streams":[]}"#).unwrap(), None);
        assert!(parse_video(br#"{"streams":[{"r_frame_rate":"0/0"}]}"#).is_err());
    }

    #[test]
    fn parses_ffprobe_json_without_titles() {
        let json = br#"{"streams":[
            {"codec_name":"aac","channels":2,"sample_rate":"48000","duration":"10.5"},
            {"codec_name":"aac","channels":2,"sample_rate":"48000","tags":{"title":"Mic"}}],
            "format":{"duration":"10.6"}}"#;
        let p = parse_probe(json).unwrap();
        assert_eq!(p.duration, 10.6);
        assert_eq!(p.audio.len(), 2);
        assert_eq!(p.audio[0].title, None);
        assert_eq!(p.audio[1].title.as_deref(), Some("Mic"));
        assert_eq!(p.audio[1].index, 1);
    }

    #[test]
    fn file_without_audio_has_no_streams() {
        let p = parse_probe(br#"{"streams":[],"format":{"duration":"3"}}"#).unwrap();
        assert!(p.audio.is_empty());
    }

    #[test]
    fn packets_rates_and_numbers() {
        let p = parse_packets(b"0.000000,K__\n0.100000,___\nN/A,___\n");
        assert_eq!(
            p,
            vec![
                PacketTime {
                    pts: 0.0,
                    key: true
                },
                PacketTime {
                    pts: 0.1,
                    key: false
                }
            ]
        );
        assert_eq!(rate(&Value::from("30000/1001")), Some((30000, 1001)));
        assert_eq!(rate(&Value::from("0/0")), None);
        assert_eq!(num::<u64>(&Value::from("160000")), Some(160_000));
        assert_eq!(num::<u32>(&Value::from(48_000)), Some(48_000));
        assert_eq!(num::<u64>(&Value::Null), None);
    }
}
