//! What a send asks Resolve to do, worked out here so the script in Resolve only carries it out:
//! one Text+ clip per speaker per caption on that speaker's own track, at integer timeline frames,
//! with every Text+ input computed and a tag that lets the next send find it again. With the
//! highlight or an animation on, clips are copies of auto-subs' animated caption (a Fusion macro
//! round a Text+); with a user template's look kept, only text, timing and colour are set.

use sha2::{Digest, Sha256};

use super::lua::Value;
use super::reply::Timeline;
use crate::edit::{parse_color, title_name, Captions, EditList, ExistingClips, TextStyle};
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
    /// The speaker has a colour of their own: without one the fill is white, which a
    /// template whose look is kept should not get, so it keeps its own.
    pub colored: bool,
    /// The animated caption's text and lit word colours (`Captions::highlight_colors`): white
    /// and the speaker's under the highlight in the speaker's colour; both the fill without it.
    pub base: [f64; 3],
    pub highlight: [f64; 3],
    /// `Center`: fractions of the frame, y up from the bottom.
    pub x: f64,
    pub y: f64,
    /// When each word of `text` (as the macro splits it, at whitespace) is lit, with the
    /// highlight on.
    pub words: Vec<WordTiming>,
}

/// One entry of the animated caption's `WordTiming`: `from..=to` are 0-based character
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
    /// The word highlight and the animation: `animation` is its word on the captions line
    /// (`none`, `fade`, `pop`, `slide`), its length in seconds.
    pub highlight: bool,
    pub animation: String,
    pub animation_length: f64,
    /// The template's own look is kept ([`Captions::keeps_look`]): the script sets none of
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
    /// The clips are the animated caption: the highlight or an animation is on.
    pub animated: bool,
    /// What the script does with clips Junctur did not place that are in the way of new
    /// captions on these tracks: ask (change nothing, count them), replace, skip, or a
    /// new track.
    pub existing: ExistingClips,
    pub look: Look,
    pub clips: Vec<Clip>,
}

/// The tag for a recording: its path hashed, short, so the tag is the same on every send and
/// differs between recordings.
pub fn recording_key(edit: &EditList) -> String {
    let digest = Sha256::digest(edit.source.to_string_lossy().as_bytes());
    crate::hash::hex(&digest[..6])
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
    // Text+ as the macro (or back). The highlight is a kind of its own, since the macro never
    // clears its highlight keyframes, so a clip that had it is replaced rather than restyled.
    let mut kind = match (c.highlight, animated) {
        (true, _) => "|highlight",
        (false, true) => "|animated",
        _ => "",
    }
    .to_string();
    // A clip says which template it is a copy of and whether its look was kept: picking another
    // template, or switching the look on or off, then replaces the clips rather than leaving the
    // old template's clip or the font Junctur set. Junctur's own template adds nothing to the key.
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
        // Each track's clips in time order, never overlapping: a clip that rounds to nothing gets
        // one frame, and one that would start inside the previous clip starts at its end instead
        // (only a clip past the timeline's end is left out).
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
        existing: c.existing_clips,
        look: Look {
            font: c.font.trim().to_string(),
            style: font_style(c.style()).to_string(),
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

/// The animated caption's `WordTiming` for a clip: one entry per span, its character range counted
/// as the macro's own `UpdateTextContent` re-counts it each time the text is set, so the entries
/// survive that re-count unchanged. Frames never go back (the macro lights words in order) and
/// end before the clip's last frame, since `ScaleWordTiming` squeezes every word otherwise.
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

/// A short tag of a clip's word timing for its key: a resend whose words moved but
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
fn font_style(s: TextStyle) -> &'static str {
    match (s.bold, s.italic) {
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
            ("existing", self.existing.word().into()),
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
mod tests;
