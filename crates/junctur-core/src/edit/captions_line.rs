//! The edit list's `captions` line (5a; moved out of `format.rs` when 5g's look made it a parser of
//! its own). `captions <on|off> chars=.. lines=.. seconds=..` then `key=value` settings, each written
//! only when it is not the default (5d's rule), so a list that does not use a newer setting still
//! reads in the builds before it, which refuse keys they do not know. Names with spaces (a font, a
//! speaker) are written in double quotes; [`Captions::check`] refuses a quote inside one.

use super::caption_text::{ENTRY_SEP, PAIR_SEP};
use super::{Captions, Case, CensorStyle, Layout, Replace};

/// The comment line written above it.
pub const HELP: &str =
    "# captions <on|off> chars=<per line> lines=<per speaker> seconds=<longest> \
     [preview=on] [bold=on] [italic=on] [underline=on] [font=\"..\"] [size=<% of height>] \
     [position=<% from bottom>] [outline=off] [outline_width=<1-10>] [outline_color=#RRGGBB] \
     [layout=stacked|side|topBottom] [protagonist=\"<speaker>\"] [template=\"<Resolve title>\"] \
     [words=<per speaker>] [case=upper|lower|title] [punctuation=off] [censor=\"word|..\"] \
     [censor_style=whole] [replace=\"from>to|..\"]";

pub fn write(c: &Captions) -> String {
    let d = Captions::default();
    let mut out = format!(
        "captions {} chars={} lines={} seconds={}",
        if c.on { "on" } else { "off" },
        c.chars,
        c.lines,
        c.seconds
    );
    // 5i's word limit only when set.
    if c.words != 0 {
        out.push_str(&format!(" words={}", c.words));
    }
    // 5d's switches and 5f's underline only when on.
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
    // 5g's look only when not the default.
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
    // 5j's text options likewise; entries joined by `|`, a pair's halves by `>`.
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
mod tests {
    use super::*;

    #[test]
    fn quoted_values_keep_their_spaces() {
        assert_eq!(
            tokens(r#"on font="Open Sans"  protagonist="" x=1"#).unwrap(),
            ["on", "font=Open Sans", "protagonist=", "x=1"]
        );
        assert!(tokens(r#"on font="Open"#).is_err());
    }

    #[test]
    fn the_look_round_trips_and_defaults_are_not_written() {
        assert_eq!(
            write(&Captions::default()),
            "captions off chars=42 lines=2 seconds=6"
        );
        let c = Captions {
            on: true,
            font: "Noto Sans".into(),
            size: 6.5,
            position: 10.0,
            outline: false,
            outline_width: 4,
            outline_color: "#202020".into(),
            layout: Layout::TopBottom,
            protagonist: "Ana María".into(),
            template: "My Title".into(),
            ..Default::default()
        };
        let line = write(&c);
        assert_eq!(
            line,
            "captions on chars=42 lines=2 seconds=6 font=\"Noto Sans\" size=6.5 position=10 \
             outline=off outline_width=4 outline_color=#202020 layout=topBottom \
             protagonist=\"Ana María\" template=\"My Title\""
        );
        assert_eq!(parse(line.strip_prefix("captions ").unwrap()).unwrap(), c);
    }

    #[test]
    fn the_word_limit_is_written_only_when_set() {
        let c = Captions {
            on: true,
            words: 1,
            ..Default::default()
        };
        let line = write(&c);
        assert_eq!(line, "captions on chars=42 lines=2 seconds=6 words=1");
        assert_eq!(parse(line.strip_prefix("captions ").unwrap()).unwrap(), c);
        assert_eq!(parse("on words=0").unwrap().words, 0);
        for bad in ["on words=21", "on words=-1", "on words=two"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_text_options_round_trip_and_defaults_are_not_written() {
        let c = Captions {
            on: true,
            case: Case::Title,
            punctuation: false,
            censor: vec!["damn".into(), "what the heck".into()],
            censor_style: CensorStyle::Whole,
            replace: vec![
                Replace {
                    from: "gonna".into(),
                    to: "going to".into(),
                },
                Replace {
                    from: "um".into(),
                    to: String::new(),
                },
            ],
            ..Default::default()
        };
        let line = write(&c);
        assert_eq!(
            line,
            "captions on chars=42 lines=2 seconds=6 case=title punctuation=off \
             censor=\"damn|what the heck\" censor_style=whole replace=\"gonna>going to|um>\""
        );
        assert_eq!(parse(line.strip_prefix("captions ").unwrap()).unwrap(), c);
        assert!(parse("on censor=\"\"").unwrap().censor.is_empty());
        assert_eq!(parse("on case=asSpoken").unwrap().case, Case::AsSpoken);
        for bad in [
            "on case=shout",
            "on punctuation=no",
            "on censor_style=all",
            "on replace=gonna",
            "on replace=\">x\"",
            "on censor=\"...\"",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn bad_look_settings_are_refused() {
        for bad in [
            "on size=1",
            "on size=big",
            "on position=60",
            "on outline=yes",
            "on outline_width=0",
            "on outline_color=black",
            "on layout=diagonal",
            "on font=\"\"",
            "on font=\"Open",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        // A bare one-word font needs no quotes.
        assert_eq!(parse("off font=Arial").unwrap().font, "Arial");
    }
}
