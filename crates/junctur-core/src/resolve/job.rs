//! What a send asks Resolve to do (5g), worked out here so the script in Resolve only carries it
//! out: one Text+ clip per speaker per caption, on that speaker's own video track, at integer
//! timeline frames, with every Text+ input it sets already computed, and the tag that lets the next
//! send find it again.
//!
//! With the word highlight or an animation on (5l) the clips are copies of auto-subs' animated
//! caption (a Fusion macro round a Text+, in the same bundled bin), and each carries its words'
//! character ranges and frames in the shape that macro reads (`WordTiming`).
//!
//! With a template of the user's picked and its look kept (5m), the look is left out: the script
//! sets only the text, the timing and the speaker's colour.

use sha2::{Digest, Sha256};

use super::lua::Value;
use super::reply::Timeline;
use crate::edit::{parse_color, title_name, Captions, EditList};
use crate::export::layout::{Placed, WordSpan};

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
    /// The speaker has a colour of their own (5m): without one the fill is white, which a
    /// template whose look is kept should not get, so it keeps its own.
    pub colored: bool,
    /// The animated caption's text and lit word colours (5l, `Captions::highlight_colors`): white
    /// and the speaker's under the highlight in the speaker's colour; both the fill without it.
    pub base: [f64; 3],
    pub highlight: [f64; 3],
    /// `Center`: fractions of the frame, y up from the bottom.
    pub x: f64,
    pub y: f64,
    /// When each word of `text` (as the macro splits it, at whitespace) is lit (5l), with the
    /// highlight on.
    pub words: Vec<WordTiming>,
}

/// One entry of the animated caption's `WordTiming` (5l): `from..=to` are 0-based character
/// indices into the clip's text, a word after the first starting just after the one before (its
/// space or line break included), as the macro's own `UpdateTextContent` counts them; `start` and
/// `end` are frames from the clip's start, in the template's frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordTiming {
    pub from: usize,
    pub to: usize,
    pub start: i64,
    pub end: i64,
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
    /// The word highlight and the animation (5l): `animation` is its word on the captions line
    /// (`none`, `fade`, `pop`, `slide`), its length in seconds.
    pub highlight: bool,
    pub animation: String,
    pub animation_length: f64,
    /// The template's own look is kept (5m, [`Captions::keeps_look`]): the script sets none of
    /// the inputs above but the highlight and the animation.
    pub keep: bool,
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
    /// The clips are the animated caption (5l): the highlight or an animation is on.
    pub animated: bool,
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
    title_name(&c.template)
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
    // Timeline frames from a clip's start to the template's own.
    let in_template = |frames: i64| frames as f64 / timeline.fps * timeline.template_fps;
    let animated = c.animated();
    // A send with the other kind of clip is a different caption: the key says which, so turning
    // the highlight or an animation on or off replaces every clip instead of restyling a plain
    // Text+ as the macro (or back). The highlight is a kind of its own: the macro's helper only
    // writes the highlight's keyframes when it is on, never clears them (its RemoveHighlight runs
    // from an input's change, which a script setting it may not trigger), so a clip that had it
    // is replaced rather than restyled without it.
    let mut kind = match (c.highlight, animated) {
        (true, _) => "|highlight",
        (false, true) => "|animated",
        _ => "",
    }
    .to_string();
    // A clip says which template it is a copy of and whether its look was kept (5m): picking
    // another template, or switching the look on or off, then replaces the clips. Restyled
    // instead, the old template's clip would stay, and one restyled with the look kept would keep
    // the font and size Junctur gave it before (with the look kept they are not set). Junctur's
    // own template adds nothing, so its keys are 5g's and 5l's.
    let named = template(c);
    let keep = c.keeps_look();
    if !named.is_empty() {
        kind.push_str(&format!("|template={named}"));
    }
    if keep {
        kind.push_str("|keep");
    }
    let mut clips = Vec::new();
    for (t, &id) in speakers.iter().enumerate() {
        let own = edit.speakers.color(id).filter(|_| id > 0);
        let color = own.unwrap_or("#FFFFFF");
        let fill = rgb(color);
        let (base, highlight) = if c.highlight {
            let (base, word) = c.highlight_colors(color);
            (rgb(&base), rgb(&word))
        } else {
            (fill, fill)
        };
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
            let length = in_template(end - start).round().max(1.0) as i64;
            let at = |s: f64| in_template(frame(s) - start).floor() as i64;
            let words = word_timing(&part.words, length, at);
            clips.push(Clip {
                track: t + 1,
                key: format!("{}|{start}|{end}{kind}{}", tracks[t], timing_tag(&words)),
                start,
                end,
                length,
                text: part.lines.join("\n"),
                fill,
                colored: own.is_some(),
                base,
                highlight,
                x: part.x,
                y: part.y,
                words,
            });
        }
    }
    Job {
        recording: recording_key(edit),
        template: template(c).to_string(),
        timeline: timeline.id.clone(),
        tracks,
        animated,
        look: Look {
            font: c.font.trim().to_string(),
            style: style(c.bold, c.italic).to_string(),
            size: size(c, timeline),
            outline: c
                .outline
                .then(|| (rgb(&c.outline_color), c.outline_width as f64 * THICKNESS)),
            underline: c.underline,
            highlight: c.highlight,
            animation: c.animation.word().to_string(),
            animation_length: c.animation_length,
            keep,
        },
        clips,
    }
}

/// The animated caption's `WordTiming` for a clip: one entry per span (a word, as layout cuts them),
/// its character range counted exactly as the macro's own `UpdateTextContent` re-counts it each
/// time the text is set (the first word from 0, each later one from just after the one before,
/// one separator ahead of it, characters by `utf8len`), so the entries survive that re-count
/// unchanged; its seconds made clip frames by `at`, never going back (the macro lights words in
/// order), the start inside the clip and the end before its last frame, since the macro's
/// `ScaleWordTiming` squeezes every word when the last one ends past the comp's last frame.
fn word_timing(spans: &[WordSpan], length: i64, at: impl Fn(f64) -> i64) -> Vec<WordTiming> {
    let last = (length - 1).max(1);
    let mut out: Vec<WordTiming> = Vec::with_capacity(spans.len());
    let mut index = 0;
    for s in spans {
        let chars = s.to.saturating_sub(s.from).max(1) + usize::from(!out.is_empty());
        let prev = out.last().map_or(0, |w| w.start);
        let start = at(s.start).clamp(0, last - 1).max(prev);
        out.push(WordTiming {
            from: index,
            to: index + chars - 1,
            start,
            end: at(s.end).clamp(start + 1, last),
        });
        index += chars;
    }
    out
}

/// A short tag of a clip's word timing for its key (5l repair): a resend whose words moved but
/// whose frames and text did not would otherwise restyle the clip, and the macro only rebuilds its
/// word keyframes when its text changes. Empty without words, so other keys stay as they were.
fn timing_tag(words: &[WordTiming]) -> String {
    if words.is_empty() {
        return String::new();
    }
    // FNV-1a: stable across builds and platforms, unlike the standard library's hasher.
    let mut h: u32 = 0x811c_9dc5;
    for w in words {
        for v in [w.from as i64, w.to as i64, w.start, w.end] {
            for b in v.to_le_bytes() {
                h = (h ^ b as u32).wrapping_mul(0x0100_0193);
            }
        }
    }
    format!("|w{h:08x}")
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
            ("highlight", self.look.highlight.into()),
            ("animation", self.look.animation.as_str().into()),
            ("animationLength", self.look.animation_length.into()),
            ("keep", self.look.keep.into()),
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
            ("animated", self.animated.into()),
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
                                ("colored", c.colored.into()),
                                ("base", rgb(c.base)),
                                ("highlight", rgb(c.highlight)),
                                ("x", c.x.into()),
                                ("y", c.y.into()),
                                // The macro's own field names, so the script hands it on as is.
                                (
                                    "words",
                                    Value::List(
                                        c.words
                                            .iter()
                                            .map(|w| {
                                                Value::Table(vec![
                                                    ("startIndex", Value::Int(w.from as i64)),
                                                    ("endIndex", Value::Int(w.to as i64)),
                                                    ("startFrame", Value::Int(w.start)),
                                                    ("endFrame", Value::Int(w.end)),
                                                ])
                                            })
                                            .collect(),
                                    ),
                                ),
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
            fixes: Vec::new(),
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
                    words: Vec::new(),
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
    #[test]
    fn the_highlight_gives_each_clip_its_words_in_the_macros_shape() {
        use crate::edit::Animation;
        use crate::export::layout::WordSpan;
        let span = |word: usize, line: usize, from: usize, to: usize, start: f64| WordSpan {
            word,
            line,
            from,
            to,
            start,
            end: start + 0.5,
        };
        // "¿qué tal" / "todos" from 1 s, a word each as layout cuts them; "¿" and "é" are one
        // character each, as the macro's `utf8len` counts them.
        let mut cue = placed(1.0, 3.0, &[(1, "¿qué tal/todos")]);
        cue.parts[0].words = vec![
            span(0, 0, 0, 4, 1.0),
            span(1, 0, 5, 8, 1.25),
            span(2, 1, 0, 5, 1.5),
        ];
        let c = Captions {
            highlight: true,
            animation: Animation::Pop,
            animation_length: 0.3,
            ..Default::default()
        };
        let t = Timeline {
            template_fps: 48.0,
            ..timeline(24.0)
        };
        let j = build(&[cue.clone()], &edit(), &c, &t);
        assert!(j.animated);
        let clip = &j.clips[0];
        // Ana's colour lights the word; the text is white. The key says the clip is highlighted,
        // and carries its word timing's tag.
        assert_eq!(clip.base, [1.0, 1.0, 1.0]);
        assert_eq!(clip.highlight, [1.0, 0.0, 0.0]);
        // A plain Text+ (a user's own template) keeps the speaker's colour.
        assert_eq!(clip.fill, [1.0, 0.0, 0.0]);
        assert!(
            clip.key.contains("|86424|86472|highlight|w"),
            "{}",
            clip.key
        );
        // 2 s of a 48 fps template. Each later word's range starts at the separator before it (a
        // space, then a line break), and the last ends on the text's last character.
        assert_eq!(clip.length, 96);
        let got: Vec<_> = clip
            .words
            .iter()
            .map(|w| (w.from, w.to, w.start, w.end))
            .collect();
        assert_eq!(got, [(0, 3, 0, 24), (4, 7, 12, 36), (8, 13, 24, 48)]);
        assert_eq!(clip.text.chars().count(), 14);
        assert_eq!(
            (
                j.look.highlight,
                j.look.animation.as_str(),
                j.look.animation_length
            ),
            (true, "pop", 0.3)
        );
        let text = super::super::lua::chunk(&Value::Table(j.to_lua()));
        assert!(text.contains("[\"animated\"] = true"), "{text}");
        assert!(
            text.contains("{[\"startIndex\"] = 4, [\"endIndex\"] = 7, [\"startFrame\"] = 12, [\"endFrame\"] = 36}"),
            "{text}"
        );
        // The same words at other times are another key (the macro only re-keys its highlight
        // when its text changes); the same times, the same key.
        let mut moved = cue.clone();
        moved.parts[0].words[1].start = 1.3;
        assert_ne!(build(&[moved], &edit(), &c, &t).clips[0].key, clip.key);
        assert_eq!(
            build(&[cue.clone()], &edit(), &c, &t).clips[0].key,
            clip.key
        );
        // A word said before the clip (a cue pushed later) starts at its first frame; one past its
        // end is held inside it, ending before the clip's last frame; frames never go back.
        let mut late = cue.clone();
        late.parts[0].words = vec![span(0, 0, 0, 4, 0.2), span(1, 0, 5, 8, 9.0)];
        let w = &build(&[late], &edit(), &c, &t).clips[0].words;
        assert_eq!((w[0].start, w[0].end, w[1].start, w[1].end), (0, 1, 94, 95));
        // An animation alone makes the clips animated, with no word timing and the speaker's
        // colour; neither, and the key is 5g's.
        let anim = Captions {
            animation: Animation::Fade,
            ..Default::default()
        };
        let mut plain = cue.clone();
        plain.parts[0].words.clear();
        let j = build(&[plain.clone()], &edit(), &anim, &t);
        assert!(j.animated && j.clips[0].words.is_empty());
        assert_eq!(j.clips[0].base, [1.0, 0.0, 0.0]);
        // Its own kind of key: a clip that had the highlight is replaced, not restyled without it.
        assert!(
            j.clips[0].key.ends_with("|86472|animated"),
            "{}",
            j.clips[0].key
        );
        let j = build(&[plain], &edit(), &Captions::default(), &t);
        assert!(!j.animated && j.clips[0].key.ends_with("|86472"));
        assert_eq!(j.look.animation, "none");
    }

    #[test]
    fn a_picked_template_and_its_kept_look_are_in_the_key() {
        let cues = [placed(0.0, 1.0, &[(1, "a"), (0, "b")])];
        let t = timeline(24.0);
        // Junctur's own: 5g's key, the switch ignored, nothing kept.
        let own = Captions {
            keep_look: true,
            ..Default::default()
        };
        let j = build(&cues, &edit(), &own, &t);
        assert!(!j.look.keep);
        assert_eq!(j.clips[1].key, "Junctur 1 Ana|86400|86424");
        // A template of the user's: its name in the key, so picking another replaces the clips.
        let mine = Captions {
            template: " My Title ".into(),
            ..Default::default()
        };
        let j = build(&cues, &edit(), &mine, &t);
        assert!(!j.look.keep);
        assert_eq!(
            j.clips[1].key,
            "Junctur 1 Ana|86400|86424|template=My Title"
        );
        // Its look kept: said in the key too, so switching it on or off replaces them as well.
        let kept = Captions {
            keep_look: true,
            highlight: true,
            ..mine
        };
        let j = build(&cues, &edit(), &kept, &t);
        assert!(j.look.keep);
        assert_eq!(
            j.clips[1].key,
            "Junctur 1 Ana|86400|86424|highlight|template=My Title|keep"
        );
        // Ana has a colour; words with no speaker have none, so the template keeps its own fill.
        assert!(j.clips[1].colored && !j.clips[0].colored);
        let text = super::super::lua::chunk(&Value::Table(j.to_lua()));
        assert!(text.contains("[\"keep\"] = true"), "{text}");
        assert!(text.contains("[\"colored\"] = false"), "{text}");
    }
}
