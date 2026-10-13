//! What a send asks Resolve to do (5g), worked out here so the script in Resolve only carries it
//! out: one Text+ clip per speaker per caption, on that speaker's own video track, at integer
//! timeline frames, with every Text+ input it sets already computed, and the tag that lets the next
//! send find it again.

use sha2::{Digest, Sha256};

use super::lua::Value;
use super::reply::Timeline;
use crate::edit::{parse_color, Captions, EditList};
use crate::export::layout::Placed;

/// Text+ outline thickness per step of `outline_width`: the bundled template's is 0.05, so the
/// default 2 steps is close to it.
const THICKNESS: f64 = 0.02;

/// The clip a caption becomes.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    /// 1-based, into [`Job::tracks`].
    pub track: usize,
    /// The tag: which caption of this recording the clip is, as `<track name>|<start>|<end>`, so a
    /// clip found with it on the next send is the same caption on the same frames.
    pub key: String,
    /// Timeline frames, the end exclusive.
    pub start: i64,
    pub end: i64,
    /// Its length in the template's own frames (the clip info's `endFrame`).
    pub length: i64,
    pub text: String,
    /// The fill, the speaker's colour, 0 to 1 per channel.
    pub fill: [f64; 3],
    /// `Center`: fractions of the frame, y up from the bottom.
    pub x: f64,
    pub y: f64,
}

/// The Text+ inputs every clip shares.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub font: String,
    /// `Style`: the font's own style names for bold and italic.
    pub style: String,
    /// `Size`, which Fusion measures against the frame width (see [`size`]).
    pub size: f64,
    pub outline: Option<([f64; 3], f64)>,
    pub underline: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// Which recording the clips are from (the tag's other half).
    pub recording: String,
    /// The template's clip name in the media pool; empty for Junctur's own.
    pub template: String,
    /// The timeline the `info` request saw: `place` refuses another (the user switched meanwhile).
    pub timeline: String,
    /// Track names, one per speaker; the script finds each by name or adds it.
    pub tracks: Vec<String>,
    pub look: Look,
    pub clips: Vec<Clip>,
}

/// The tag for a recording: its path hashed, short, so the tag is the same on every send and
/// differs between recordings.
pub fn recording_key(edit: &EditList) -> String {
    let digest = Sha256::digest(edit.source.to_string_lossy().as_bytes());
    digest[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// The template the settings name, as both requests send it: empty for Junctur's own.
pub fn template(c: &Captions) -> &str {
    c.template.trim()
}

/// Each speaker's track name: their id keeps two speakers with one name (or none) apart, their
/// name keeps it readable in Resolve. Words with no speaker go on `Junctur`.
fn track_name(edit: &EditList, id: u32) -> String {
    match (id, edit.speakers.name(id).map(str::trim)) {
        (0, _) => "Junctur".to_string(),
        (id, Some(n)) if !n.is_empty() => format!("Junctur {id} {n}"),
        (id, _) => format!("Junctur {id}"),
    }
}

/// Every placed caption as clips on `timeline`.
pub fn build(placed: &[Placed], edit: &EditList, c: &Captions, timeline: &Timeline) -> Job {
    let mut speakers: Vec<u32> = placed
        .iter()
        .flat_map(|p| p.parts.iter().map(|q| q.speaker))
        .collect();
    speakers.sort_unstable();
    speakers.dedup();
    let tracks: Vec<String> = speakers.iter().map(|&id| track_name(edit, id)).collect();
    let frame = |s: f64| timeline.start + (s * timeline.fps).round() as i64;
    let mut clips = Vec::new();
    for (t, &id) in speakers.iter().enumerate() {
        let fill = rgb(edit
            .speakers
            .color(id)
            .filter(|_| id > 0)
            .unwrap_or("#FFFFFF"));
        // Each track's clips in time order, never overlapping: a clip that rounds to nothing gets one
        // frame, and one that would start inside the previous clip starts at its end instead (shifted,
        // not dropped; only a clip past the timeline's end is left out).
        let mut last_end = i64::MIN;
        for p in placed {
            let Some(part) = p.parts.iter().find(|q| q.speaker == id) else {
                continue;
            };
            let start = frame(p.start).max(last_end);
            let mut end = frame(p.end);
            if end <= start {
                end = start + 1;
            }
            if timeline.end > timeline.start && start >= timeline.end {
                continue;
            }
            last_end = end;
            clips.push(Clip {
                track: t + 1,
                key: format!("{}|{start}|{end}", tracks[t]),
                start,
                end,
                length: (((end - start) as f64 / timeline.fps) * timeline.template_fps)
                    .round()
                    .max(1.0) as i64,
                text: part.lines.join("\n"),
                fill,
                x: part.x,
                y: part.y,
            });
        }
    }
    Job {
        recording: recording_key(edit),
        template: template(c).to_string(),
        timeline: timeline.id.clone(),
        tracks,
        look: Look {
            font: c.font.trim().to_string(),
            style: style(c.bold, c.italic).to_string(),
            size: size(c, timeline),
            outline: c
                .outline
                .then(|| (rgb(&c.outline_color), c.outline_width as f64 * THICKNESS)),
            underline: c.underline,
        },
        clips,
    }
}

/// Text+ `Size` for the caption size (a share of the picture height): Fusion sizes text against the
/// frame width, so a vertical frame's captions keep the height share the preview shows.
pub fn size(c: &Captions, t: &Timeline) -> f64 {
    let aspect = if t.width > 0 && t.height > 0 {
        t.height as f64 / t.width as f64
    } else {
        9.0 / 16.0
    };
    c.size / 100.0 * aspect
}

/// The style names most fonts use for these (Open Sans, Arial, Noto among them).
fn style(bold: bool, italic: bool) -> &'static str {
    match (bold, italic) {
        (false, false) => "Regular",
        (true, false) => "Bold",
        (false, true) => "Italic",
        (true, true) => "Bold Italic",
    }
}

fn rgb(hex: &str) -> [f64; 3] {
    let hex = parse_color(hex).unwrap_or_else(|| "#FFFFFF".into());
    let ch = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(255) as f64 / 255.0;
    [ch(1), ch(3), ch(5)]
}

impl Job {
    /// The `place` request's fields.
    pub fn to_lua(&self) -> Vec<(&'static str, Value)> {
        let rgb = |c: [f64; 3]| Value::List(c.iter().map(|&v| Value::Num(v)).collect());
        let mut look = vec![
            ("font", Value::from(self.look.font.as_str())),
            ("style", self.look.style.as_str().into()),
            ("size", self.look.size.into()),
            ("underline", self.look.underline.into()),
            ("outline", self.look.outline.is_some().into()),
        ];
        if let Some((color, thickness)) = self.look.outline {
            look.push(("outlineColor", rgb(color)));
            look.push(("thickness", thickness.into()));
        }
        vec![
            ("recording", self.recording.as_str().into()),
            ("template", self.template.as_str().into()),
            ("timeline", self.timeline.as_str().into()),
            (
                "tracks",
                Value::List(self.tracks.iter().map(|t| t.as_str().into()).collect()),
            ),
            ("look", Value::Table(look)),
            (
                "clips",
                Value::List(
                    self.clips
                        .iter()
                        .map(|c| {
                            Value::Table(vec![
                                ("track", Value::Int(c.track as i64)),
                                ("key", c.key.as_str().into()),
                                ("start", Value::Int(c.start)),
                                ("end", Value::Int(c.end)),
                                ("length", Value::Int(c.length)),
                                ("text", c.text.as_str().into()),
                                ("fill", rgb(c.fill)),
                                ("x", c.x.into()),
                                ("y", c.y.into()),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{SpeakerName, Speakers};
    use crate::export::layout::PlacedPart;

    fn edit() -> EditList {
        let name = |id: u32, n: &str, color: &str| SpeakerName {
            id,
            name: n.into(),
            track: None,
            color: color.into(),
        };
        EditList {
            source: "C:\\Videos\\stream.mp4".into(),
            duration: 100.0,
            settings: Default::default(),
            keep: Vec::new(),
            speakers: Speakers {
                names: vec![name(1, "Ana", "#FF0000"), name(2, "Bo", "#00FF80")],
                turns: Vec::new(),
            },
            captions: None,
            windows: Vec::new(),
        }
    }

    fn timeline(fps: f64) -> Timeline {
        Timeline {
            name: "Main".into(),
            id: "T-1".into(),
            fps,
            start: 86400,
            end: 86400 + 100 * 24,
            width: 1920,
            height: 1080,
            template: "Junctur Caption".into(),
            template_fps: 24.0,
        }
    }

    fn placed(start: f64, end: f64, parts: &[(u32, &str)]) -> Placed {
        Placed {
            start,
            end,
            parts: parts
                .iter()
                .enumerate()
                .map(|(i, &(speaker, text))| PlacedPart {
                    speaker,
                    lines: text.split('/').map(String::from).collect(),
                    x: 0.5,
                    y: 0.1 + i as f64 * 0.1,
                })
                .collect(),
        }
    }

    #[test]
    fn a_track_per_speaker_and_a_clip_per_part_at_integer_frames() {
        let cues = [
            placed(0.0, 1.5, &[(1, "hola/a todos")]),
            // Bo and Ana at once: one clip each, on their own tracks, Bo's above.
            placed(1.5, 3.02, &[(2, "qué tal"), (1, "bien")]),
            placed(3.02, 4.0, &[(0, "…")]),
        ];
        let j = build(&cues, &edit(), &Captions::default(), &timeline(24.0));
        assert_eq!(j.tracks, ["Junctur", "Junctur 1 Ana", "Junctur 2 Bo"]);
        let got: Vec<_> = j
            .clips
            .iter()
            .map(|c| (c.track, c.start, c.end, c.text.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (1, 86400 + 72, 86400 + 96, "…"),
                (2, 86400, 86400 + 36, "hola\na todos"),
                (2, 86400 + 36, 86400 + 72, "bien"),
                (3, 86400 + 36, 86400 + 72, "qué tal"),
            ]
        );
        // 3.02 s is 72.48 frames: rounded, never a fraction.
        let bo = &j.clips[3];
        assert_eq!((bo.fill, bo.y), ([0.0, 1.0, 128.0 / 255.0], 0.1));
        assert_eq!(j.clips[0].fill, [1.0, 1.0, 1.0]);
        assert_eq!(bo.key, "Junctur 2 Bo|86436|86472");
        assert_eq!(bo.length, 36);
        assert_eq!(j.recording, recording_key(&edit()));
        assert_eq!(j.recording.len(), 12);
    }

    #[test]
    fn speakers_sharing_a_name_or_having_none_get_tracks_of_their_own() {
        let mut e = edit();
        e.speakers.names[1].name = "Ana".into();
        e.speakers.names.push(SpeakerName {
            id: 3,
            name: " ".into(),
            track: None,
            color: "#FFFFFF".into(),
        });
        let cues = [placed(0.0, 1.0, &[(1, "a"), (2, "b"), (3, "c"), (4, "d")])];
        let j = build(&cues, &e, &Captions::default(), &timeline(24.0));
        assert_eq!(
            j.tracks,
            ["Junctur 1 Ana", "Junctur 2 Ana", "Junctur 3", "Junctur 4"]
        );
        // The template is trimmed once, for both requests; the timeline `info` saw goes along.
        let c = Captions {
            template: "  My Title ".into(),
            ..Default::default()
        };
        assert_eq!(template(&c), "My Title");
        let j = build(&cues, &e, &c, &timeline(24.0));
        assert_eq!(
            (j.template.as_str(), j.timeline.as_str()),
            ("My Title", "T-1")
        );
    }

    #[test]
    fn clips_on_a_track_never_overlap_and_lengths_are_in_template_frames() {
        // At 29.97 a 0.01 s cue rounds to nothing and a touching one rounds onto it.
        let cues = [
            placed(1.0, 1.01, &[(1, "a")]),
            placed(1.01, 2.0, &[(1, "b")]),
        ];
        let t = Timeline {
            fps: 30000.0 / 1001.0,
            template_fps: 24.0,
            ..timeline(24.0)
        };
        let j = build(&cues, &edit(), &Captions::default(), &t);
        let frames: Vec<_> = j
            .clips
            .iter()
            .map(|c| (c.start - t.start, c.end - t.start))
            .collect();
        assert_eq!(frames, [(30, 31), (31, 60)]);
        // 29 timeline frames at 29.97 are 23 frames of a 24 fps template.
        assert_eq!(j.clips[1].length, 23);
        // Past the timeline's end: not placed.
        let late = [placed(200.0, 201.0, &[(1, "x")])];
        assert!(build(&late, &edit(), &Captions::default(), &timeline(24.0))
            .clips
            .is_empty());
    }

    #[test]
    fn the_look_maps_onto_text_plus_inputs() {
        let c = Captions {
            bold: true,
            italic: true,
            underline: true,
            size: 8.0,
            outline_width: 3,
            outline_color: "#FFFFFF".into(),
            template: "My Title".into(),
            ..Default::default()
        };
        let j = build(&[], &edit(), &c, &timeline(24.0));
        assert_eq!(j.look.style, "Bold Italic");
        assert!(j.look.underline);
        // 8 % of a 1080-high frame, measured against its 1920 width: an assumption about Text+'s
        // `Size` (Fusion sizing text by the frame width) that only a real Resolve can confirm.
        assert!((j.look.size - 0.08 * 1080.0 / 1920.0).abs() < 1e-12);
        let (color, thickness) = j.look.outline.unwrap();
        assert_eq!(color, [1.0, 1.0, 1.0]);
        assert!((thickness - 0.06).abs() < 1e-12);
        assert_eq!(j.template, "My Title");
        let vertical = Timeline {
            width: 1080,
            height: 1920,
            ..timeline(24.0)
        };
        assert!((size(&c, &vertical) - 0.08 * 1920.0 / 1080.0).abs() < 1e-12);
        let plain = build(
            &[],
            &edit(),
            &Captions {
                outline: false,
                ..Default::default()
            },
            &timeline(24.0),
        );
        assert_eq!(plain.look.style, "Regular");
        assert_eq!(plain.look.outline, None);
    }

    #[test]
    fn the_request_holds_integers_for_frames() {
        let j = build(
            &[placed(0.0, 1.0, &[(1, "say \"hi\"")])],
            &edit(),
            &Captions::default(),
            &timeline(24.0),
        );
        let text = super::super::lua::chunk(&Value::Table(j.to_lua()));
        assert!(
            text.contains("[\"start\"] = 86400, [\"end\"] = 86424"),
            "{text}"
        );
        assert!(text.contains("[\"text\"] = \"say \\\"hi\\\"\""), "{text}");
        assert!(text.contains("[\"timeline\"] = \"T-1\""), "{text}");
        assert!(
            text.contains("[\"outline\"] = true, [\"outlineColor\"] = {0.000000"),
            "{text}"
        );
    }
}
