//! The edit list's `captions` line (5a; moved out of `format.rs` when 5g's look made it a parser of
//! its own). `captions <on|off> chars=.. lines=.. seconds=..` then `key=value` settings, each written
//! only when it is not the default (5d's rule), so a list that does not use a newer setting still
//! reads in the builds before it, which refuse keys they do not know. Names with spaces (a font, a
//! speaker) are written in double quotes; [`Captions::check`] refuses a quote inside one.

use super::{Captions, Layout};

/// The comment line written above it.
pub const HELP: &str =
    "# captions <on|off> chars=<per line> lines=<per speaker> seconds=<longest> \
     [preview=on] [bold=on] [italic=on] [underline=on] [font=\"..\"] [size=<% of height>] \
     [position=<% from bottom>] [outline=off] [outline_width=<1-10>] [outline_color=#RRGGBB] \
     [layout=stacked|side|topBottom] [protagonist=\"<speaker>\"] [template=\"<Resolve title>\"]";

pub fn write(c: &Captions) -> String {
    let d = Captions::default();
    let mut out = format!(
        "captions {} chars={} lines={} seconds={}",
        if c.on { "on" } else { "off" },
        c.chars,
        c.lines,
        c.seconds
    );
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
