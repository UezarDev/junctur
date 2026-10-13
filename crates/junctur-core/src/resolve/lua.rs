//! Lua literals for the request file (5g). Resolve 21's script state has no `io` and no JSON, so a
//! request is a Lua chunk, `return { ... }`, that the resident script reads with `loadfile`.
//! Keys are always written `["key"] =`, so a key that is a Lua keyword (`end`) needs no care.

/// A Lua value as the request needs them.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    /// Frames: Resolve 21 drops a whole `AppendToTimeline` batch over one non-integer frame, so
    /// they are written as integers, never `12.0`.
    Int(i64),
    Num(f64),
    Bool(bool),
    List(Vec<Value>),
    Table(Vec<(&'static str, Value)>),
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Int(n)
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Num(n)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

/// `return <value>`.
pub fn chunk(v: &Value) -> String {
    let mut out = String::from("return ");
    write(v, &mut out);
    out.push('\n');
    out
}

fn write(v: &Value, out: &mut String) {
    match v {
        Value::Str(s) => string(s, out),
        Value::Int(n) => out.push_str(&n.to_string()),
        // Six places is a millionth of the picture: finer than a pixel at 8K.
        Value::Num(n) if n.is_finite() => out.push_str(&format!("{n:.6}")),
        Value::Num(_) => out.push('0'),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::List(items) => {
            out.push('{');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write(item, out);
            }
            out.push('}');
        }
        Value::Table(fields) => {
            out.push('{');
            for (i, (k, item)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str("[\"");
                out.push_str(k);
                out.push_str("\"] = ");
                write(item, out);
            }
            out.push('}');
        }
    }
}

/// A Lua 5.1 string: quotes, backslashes and control bytes escaped (decimal `\ddd`, the only
/// numeric escape LuaJIT and 5.1 share), anything else as it is, UTF-8 included.
fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 || c as u32 == 0x7F => {
                out.push_str(&format!("\\{:03}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_escaped_and_utf8_kept() {
        let mut s = String::new();
        string("a \"b\" \\ c\nd\re\u{1}ñ", &mut s);
        assert_eq!(s, r#""a \"b\" \\ c\nd\013e\001ñ""#);
    }

    #[test]
    fn tables_lists_and_numbers() {
        let v = Value::Table(vec![
            ("id", "17".into()),
            ("end", Value::Int(120)),
            ("x", Value::Num(0.5)),
            ("nan", Value::Num(f64::NAN)),
            ("on", true.into()),
            ("list", Value::List(vec![Value::Int(1), "two".into()])),
        ]);
        assert_eq!(
            chunk(&v),
            "return {[\"id\"] = \"17\", [\"end\"] = 120, [\"x\"] = 0.500000, [\"nan\"] = 0, \
             [\"on\"] = true, [\"list\"] = {1, \"two\"}}\n"
        );
    }
}
