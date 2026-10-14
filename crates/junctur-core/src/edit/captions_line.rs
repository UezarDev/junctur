//! The edit list's `captions` line: `captions <on|off> chars=.. lines=.. seconds=..` then `key=value`
//! settings, each written only when not the default, so a list that does not use a newer setting
//! still reads in older builds, which refuse unknown keys. Names with spaces are double-quoted;
//! [`Captions::check`] refuses a quote inside one.

use super::caption_text::{ENTRY_SEP, PAIR_SEP};
use super::{Animation, Captions, Case, CensorStyle, ExistingClips, Layout, Replace};

/// The comment line written above it.
pub const HELP: &str =
    "# captions <on|off> chars=<per line> lines=<per speaker> seconds=<longest> \
     [preview=on] [bold=on] [italic=on] [underline=on] [font=\"..\"] [size=<% of height>] \
     [position=<% from bottom>] [outline=off] [outline_width=<1-10>] [outline_color=#RRGGBB] \
     [layout=stacked|side|topBottom] [protagonist=\"<speaker>\"] [template=\"<Resolve title>\"] [keep_look=on] \
     [existing_clips=replace|skip|newTrack] \
     [words=<per speaker>] [case=upper|lower|title] [punctuation=off] [censor=\"word|..\"] \
     [censor_style=whole] [replace=\"from>to|..\"] [translate=<language code>] [highlight=on] \
     [highlight_color=#RRGGBB] [animation=fade|pop|slide] [animation_length=<seconds>]";

pub fn write(c: &Captions) -> String {
    let d = Captions::default();
    let mut out = format!(
        "captions {} chars={} lines={} seconds={}",
        if c.on { "on" } else { "off" },
        c.chars,
        c.lines,
        c.seconds
    );
    // The word limit only when set.
    if c.words != 0 {
        out.push_str(&format!(" words={}", c.words));
    }
    // The switches and the underline only when on.
    for (k, on) in [
        ("preview", c.preview),
        ("bold", c.bold),
        ("italic", c.italic),
        ("underline", c.underline),
    ] {
        if on {
            out.push_str(&format!(" {k}=on"));
        }
    }
    // The look only when not the default.
    let mut put = |differs: bool, kv: String| {
        if differs {
            out.push(' ');
            out.push_str(&kv);
        }
    };
    put(c.font != d.font, format!("font=\"{}\"", c.font));
    put(c.size != d.size, format!("size={}", c.size));
    put(c.position != d.position, format!("position={}", c.position));
    put(
        c.outline != d.outline,
        format!("outline={}", if c.outline { "on" } else { "off" }),
    );
    put(
        c.outline_width != d.outline_width,
        format!("outline_width={}", c.outline_width),
    );
    put(
        c.outline_color != d.outline_color,
        format!("outline_color={}", c.outline_color),
    );
    put(c.layout != d.layout, format!("layout={}", c.layout.word()));
    put(
        c.protagonist != d.protagonist,
        format!("protagonist=\"{}\"", c.protagonist),
    );
    put(
        c.template != d.template,
        format!("template=\"{}\"", c.template),
    );
    // The keep-look switch only when on.
    put(c.keep_look, "keep_look=on".to_string());
    // The remembered choice for clips in the way only when there is one.
    put(
        c.existing_clips != d.existing_clips,
        format!("existing_clips={}", c.existing_clips.word()),
    );
    // The text options likewise; entries joined by `|`, a pair's halves by `>`.
    put(c.case != d.case, format!("case={}", c.case.word()));
    put(
        c.punctuation != d.punctuation,
        format!("punctuation={}", if c.punctuation { "on" } else { "off" }),
    );
    put(
        !c.censor.is_empty(),
        format!("censor=\"{}\"", c.censor.join(&ENTRY_SEP.to_string())),
    );
    put(
        c.censor_style != d.censor_style,
        format!("censor_style={}", c.censor_style.word()),
    );
    let pairs: Vec<String> = c
        .replace
        .iter()
        .map(|r| format!("{}{PAIR_SEP}{}", r.from, r.to))
        .collect();
    put(
        !pairs.is_empty(),
        format!("replace=\"{}\"", pairs.join(&ENTRY_SEP.to_string())),
    );
    // The translation only when set.
    put(
        !c.translate.is_empty(),
        format!("translate={}", c.translate),
    );
    // The highlight and animation likewise.
    put(
        c.highlight,
        format!("highlight={}", if c.highlight { "on" } else { "off" }),
    );
    put(
        !c.highlight_color.is_empty(),
        format!("highlight_color={}", c.highlight_color),
    );
    put(
        c.animation != d.animation,
        format!("animation={}", c.animation.word()),
    );
    put(
        c.animation_length != d.animation_length,
        format!("animation_length={}", c.animation_length),
    );
    out
}

/// A `|`-joined list, its empty entries dropped (so `censor=""` is none).
fn entries(v: &str) -> impl Iterator<Item = &str> {
    v.split(ENTRY_SEP).filter(|e| !e.trim().is_empty())
}

/// The line after `captions `.
pub fn parse(rest: &str) -> Result<Captions, String> {
    let mut c = Captions::default();
    let mut parts = tokens(rest)?.into_iter();
    c.on = match parts.next().as_deref() {
        Some("on") => true,
        Some("off") => false,
        _ => return Err("expected `captions <on|off> ...`".into()),
    };
    for kv in parts {
        let (k, v) = kv.split_once('=').ok_or("expected key=value")?;
        let whole = || {
            v.parse::<u32>()
                .map_err(|_| "not a whole number".to_string())
        };
        let number = || v.parse::<f64>().map_err(|_| "not a number".to_string());
        let flag = || match v {
            "on" => Ok(true),
            "off" => Ok(false),
            _ => Err("expected on or off".to_string()),
        };
        match k {
            "chars" => c.chars = whole()?,
            "lines" => c.lines = whole()?,
            "words" => c.words = whole()?,
            "seconds" => c.seconds = number()?,
            "preview" => c.preview = flag()?,
            "bold" => c.bold = flag()?,
            "italic" => c.italic = flag()?,
            "underline" => c.underline = flag()?,
            "font" => c.font = v.to_string(),
            "size" => c.size = number()?,
            "position" => c.position = number()?,
            "outline" => c.outline = flag()?,
            "outline_width" => c.outline_width = whole()?,
            "outline_color" => {
                c.outline_color = super::parse_color(v).ok_or("expected a colour like #000000")?;
            }
            "layout" => {
                c.layout = Layout::from_word(v).ok_or("expected stacked, side or topBottom")?;
            }
            "protagonist" => c.protagonist = v.to_string(),
            "template" => c.template = v.to_string(),
            "keep_look" => c.keep_look = flag()?,
            "existing_clips" => {
                c.existing_clips =
                    ExistingClips::from_word(v).ok_or("expected ask, replace, skip or newTrack")?;
            }
            "case" => c.case = Case::from_word(v).ok_or("expected upper, lower or title")?,
            "punctuation" => c.punctuation = flag()?,
            "censor" => c.censor = entries(v).map(str::to_string).collect(),
            "censor_style" => {
                c.censor_style = CensorStyle::from_word(v).ok_or("expected middle or whole")?;
            }
            "replace" => {
                c.replace = entries(v)
                    .map(|e| {
                        let (from, to) = e.split_once(PAIR_SEP).ok_or("expected from>to")?;
                        Ok(Replace {
                            from: from.to_string(),
                            to: to.to_string(),
                        })
                    })
                    .collect::<Result<_, String>>()?;
            }
            "translate" => c.translate = v.to_string(),
            "highlight" => c.highlight = flag()?,
            "highlight_color" => {
                c.highlight_color = if v.is_empty() {
                    String::new()
                } else {
                    super::parse_color(v).ok_or("expected a colour like #FFD400")?
                };
            }
            "animation" => {
                c.animation = Animation::from_word(v).ok_or("expected none, fade, pop or slide")?;
            }
            "animation_length" => c.animation_length = number()?,
            _ => return Err("unknown caption setting".into()),
        }
    }
    c.check()?;
    Ok(c)
}

/// Splits on spaces outside double quotes, dropping the quotes: `font="Open Sans"` is one token,
/// `font=Open Sans`.
fn tokens(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for ch in s.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if quoted {
        return Err("a quote is not closed".into());
    }
    if any {
        out.push(cur);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
