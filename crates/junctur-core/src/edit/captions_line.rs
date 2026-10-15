//! The edit list's `captions` line: `captions <on|off> chars=.. lines=.. seconds=..` then `key=value`
//! settings, each written only when not the default, so a list that does not use a newer setting
//! still reads in older builds, which refuse unknown keys. Names with spaces are double-quoted;
//! [`Captions::check`] refuses a quote inside one.

use std::sync::LazyLock;

use super::caption_text::{ENTRY_SEP, PAIR_SEP};
use super::{Animation, Captions, Case, CensorStyle, ExistingClips, Layout, Replace};

/// One setting of the line: its name, its part of [`help`], how it is written (`None`: left out,
/// against `d`, the default) and how it is read. Every key is here only, so the comment, the
/// writer and the reader cannot disagree.
struct Key {
    name: &'static str,
    help: &'static str,
    /// The limits lead the line, in table order, before every other key.
    head: bool,
    write: fn(c: &Captions, d: &Captions) -> Option<String>,
    read: fn(c: &mut Captions, v: &str) -> Result<(), String>,
}

/// In the order [`help`] lists them.
const KEYS: &[Key] = &[
    Key {
        name: "chars",
        help: "chars=<per line>",
        head: true,
        write: |c, _| Some(c.chars.to_string()),
        read: |c, v| set(&mut c.chars, whole(v)),
    },
    Key {
        name: "lines",
        help: "lines=<per speaker>",
        head: true,
        write: |c, _| Some(c.lines.to_string()),
        read: |c, v| set(&mut c.lines, whole(v)),
    },
    Key {
        name: "seconds",
        help: "seconds=<longest>",
        head: true,
        write: |c, _| Some(c.seconds.to_string()),
        read: |c, v| set(&mut c.seconds, number(v)),
    },
    Key {
        name: "preview",
        help: "[preview=on]",
        head: false,
        write: |c, _| when_on(c.preview),
        read: |c, v| set(&mut c.preview, flag(v)),
    },
    Key {
        name: "bold",
        help: "[bold=on]",
        head: false,
        write: |c, _| when_on(c.bold),
        read: |c, v| set(&mut c.bold, flag(v)),
    },
    Key {
        name: "italic",
        help: "[italic=on]",
        head: false,
        write: |c, _| when_on(c.italic),
        read: |c, v| set(&mut c.italic, flag(v)),
    },
    Key {
        name: "underline",
        help: "[underline=on]",
        head: false,
        write: |c, _| when_on(c.underline),
        read: |c, v| set(&mut c.underline, flag(v)),
    },
    Key {
        name: "font",
        help: "[font=\"..\"]",
        head: false,
        write: |c, d| (c.font != d.font).then(|| quoted(&c.font)),
        read: |c, v| set(&mut c.font, text(v)),
    },
    Key {
        name: "size",
        help: "[size=<% of height>]",
        head: false,
        write: |c, d| (c.size != d.size).then(|| c.size.to_string()),
        read: |c, v| set(&mut c.size, number(v)),
    },
    Key {
        name: "position",
        help: "[position=<% from bottom>]",
        head: false,
        write: |c, d| (c.position != d.position).then(|| c.position.to_string()),
        read: |c, v| set(&mut c.position, number(v)),
    },
    Key {
        name: "outline",
        help: "[outline=off]",
        head: false,
        write: |c, d| (c.outline != d.outline).then(|| on_off(c.outline)),
        read: |c, v| set(&mut c.outline, flag(v)),
    },
    Key {
        name: "outline_width",
        help: "[outline_width=<1-10>]",
        head: false,
        write: |c, d| (c.outline_width != d.outline_width).then(|| c.outline_width.to_string()),
        read: |c, v| set(&mut c.outline_width, whole(v)),
    },
    Key {
        name: "outline_color",
        help: "[outline_color=#RRGGBB]",
        head: false,
        write: |c, d| (c.outline_color != d.outline_color).then(|| c.outline_color.clone()),
        read: |c, v| {
            c.outline_color = super::parse_color(v).ok_or("expected a colour like #000000")?;
            Ok(())
        },
    },
    Key {
        name: "layout",
        help: "[layout=stacked|side|topBottom]",
        head: false,
        write: |c, d| (c.layout != d.layout).then(|| c.layout.word().into()),
        read: |c, v| {
            c.layout = Layout::from_word(v).ok_or("expected stacked, side or topBottom")?;
            Ok(())
        },
    },
    Key {
        name: "protagonist",
        help: "[protagonist=\"<speaker>\"]",
        head: false,
        write: |c, d| (c.protagonist != d.protagonist).then(|| quoted(&c.protagonist)),
        read: |c, v| set(&mut c.protagonist, text(v)),
    },
    Key {
        name: "template",
        help: "[template=\"<Resolve title>\"]",
        head: false,
        write: |c, d| (c.template != d.template).then(|| quoted(&c.template)),
        read: |c, v| set(&mut c.template, text(v)),
    },
    Key {
        name: "keep_look",
        help: "[keep_look=on]",
        head: false,
        write: |c, _| when_on(c.keep_look),
        read: |c, v| set(&mut c.keep_look, flag(v)),
    },
    Key {
        name: "existing_clips",
        help: "[existing_clips=replace|skip|newTrack]",
        head: false,
        write: |c, d| {
            (c.existing_clips != d.existing_clips).then(|| c.existing_clips.word().into())
        },
        read: |c, v| {
            c.existing_clips =
                ExistingClips::from_word(v).ok_or("expected ask, replace, skip or newTrack")?;
            Ok(())
        },
    },
    // Listed with the text options, written with the limits it is one of.
    Key {
        name: "words",
        help: "[words=<per speaker>]",
        head: true,
        write: |c, _| (c.words != 0).then(|| c.words.to_string()),
        read: |c, v| set(&mut c.words, whole(v)),
    },
    Key {
        name: "case",
        help: "[case=upper|lower|title]",
        head: false,
        write: |c, d| (c.case != d.case).then(|| c.case.word().into()),
        read: |c, v| {
            c.case = Case::from_word(v).ok_or("expected upper, lower or title")?;
            Ok(())
        },
    },
    Key {
        name: "punctuation",
        help: "[punctuation=off]",
        head: false,
        write: |c, d| (c.punctuation != d.punctuation).then(|| on_off(c.punctuation)),
        read: |c, v| set(&mut c.punctuation, flag(v)),
    },
    // Entries joined by `|`, a pair's halves by `>`.
    Key {
        name: "censor",
        help: "[censor=\"word|..\"]",
        head: false,
        write: |c, _| {
            (!c.censor.is_empty()).then(|| quoted(&c.censor.join(&ENTRY_SEP.to_string())))
        },
        read: |c, v| set(&mut c.censor, Ok(entries(v).map(str::to_string).collect())),
    },
    Key {
        name: "censor_style",
        help: "[censor_style=whole]",
        head: false,
        write: |c, d| (c.censor_style != d.censor_style).then(|| c.censor_style.word().into()),
        read: |c, v| {
            c.censor_style = CensorStyle::from_word(v).ok_or("expected middle or whole")?;
            Ok(())
        },
    },
    Key {
        name: "replace",
        help: "[replace=\"from>to|..\"]",
        head: false,
        write: |c, _| {
            let pairs: Vec<String> = c
                .replace
                .iter()
                .map(|r| format!("{}{PAIR_SEP}{}", r.from, r.to))
                .collect();
            (!pairs.is_empty()).then(|| quoted(&pairs.join(&ENTRY_SEP.to_string())))
        },
        read: |c, v| {
            c.replace = entries(v)
                .map(|e| {
                    let (from, to) = e.split_once(PAIR_SEP).ok_or("expected from>to")?;
                    Ok(Replace {
                        from: from.to_string(),
                        to: to.to_string(),
                    })
                })
                .collect::<Result<_, String>>()?;
            Ok(())
        },
    },
    Key {
        name: "translate",
        help: "[translate=<language code>]",
        head: false,
        write: |c, _| (!c.translate.is_empty()).then(|| c.translate.clone()),
        read: |c, v| set(&mut c.translate, text(v)),
    },
    Key {
        name: "highlight",
        help: "[highlight=on]",
        head: false,
        write: |c, _| when_on(c.highlight),
        read: |c, v| set(&mut c.highlight, flag(v)),
    },
    Key {
        name: "highlight_color",
        help: "[highlight_color=#RRGGBB]",
        head: false,
        write: |c, _| (!c.highlight_color.is_empty()).then(|| c.highlight_color.clone()),
        read: |c, v| {
            c.highlight_color = if v.is_empty() {
                String::new()
            } else {
                super::parse_color(v).ok_or("expected a colour like #FFD400")?
            };
            Ok(())
        },
    },
    Key {
        name: "animation",
        help: "[animation=fade|pop|slide]",
        head: false,
        write: |c, d| (c.animation != d.animation).then(|| c.animation.word().into()),
        read: |c, v| {
            c.animation = Animation::from_word(v).ok_or("expected none, fade, pop or slide")?;
            Ok(())
        },
    },
    Key {
        name: "animation_length",
        help: "[animation_length=<seconds>]",
        head: false,
        write: |c, d| {
            (c.animation_length != d.animation_length).then(|| c.animation_length.to_string())
        },
        read: |c, v| set(&mut c.animation_length, number(v)),
    },
];

/// The comment line written above it.
pub fn help() -> &'static str {
    static HELP: LazyLock<String> = LazyLock::new(|| {
        let keys: Vec<&str> = KEYS.iter().map(|k| k.help).collect();
        format!("# captions <on|off> {}", keys.join(" "))
    });
    &HELP
}

pub fn write(c: &Captions) -> String {
    let d = Captions::default();
    let mut out = format!("captions {}", on_off(c.on));
    let (head, rest): (Vec<&Key>, Vec<&Key>) = KEYS.iter().partition(|k| k.head);
    for k in head.into_iter().chain(rest) {
        if let Some(v) = (k.write)(c, &d) {
            out.push_str(&format!(" {}={v}", k.name));
        }
    }
    out
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
        let key = KEYS
            .iter()
            .find(|key| key.name == k)
            .ok_or("unknown caption setting")?;
        (key.read)(&mut c, v)?;
    }
    c.check()?;
    Ok(c)
}

/// `field` set to `v` once it reads.
fn set<T>(field: &mut T, v: Result<T, String>) -> Result<(), String> {
    *field = v?;
    Ok(())
}

fn text(v: &str) -> Result<String, String> {
    Ok(v.to_string())
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

/// A switch that is off unless set: written only when on.
fn when_on(on: bool) -> Option<String> {
    on.then(|| "on".to_string())
}

fn quoted(v: &str) -> String {
    format!("\"{v}\"")
}

fn whole(v: &str) -> Result<u32, String> {
    v.parse().map_err(|_| "not a whole number".to_string())
}

fn number(v: &str) -> Result<f64, String> {
    v.parse().map_err(|_| "not a number".to_string())
}

fn flag(v: &str) -> Result<bool, String> {
    match v {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err("expected on or off".to_string()),
    }
}

/// A `|`-joined list, its empty entries dropped (so `censor=""` is none).
fn entries(v: &str) -> impl Iterator<Item = &str> {
    v.split(ENTRY_SEP).filter(|e| !e.trim().is_empty())
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
