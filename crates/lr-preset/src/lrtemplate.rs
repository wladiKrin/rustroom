//! An `.lrtemplate` develop preset — the format Lightroom Classic wrote before 7.3 (2018): a Lua table,
//! `s = { title = "…", value = { settings = { Exposure2012 = 0.3, … } } }`.

use crate::{Error, Raw, Settings, Value};

/// Read an `.lrtemplate`'s bytes — UTF-8 (a byte-order mark dropped), or Latin-1 when they are not.
pub fn read_bytes(bytes: &[u8]) -> Result<Raw, Error> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    };
    let toks = tokens(&text)?;
    let mut p = Parser { toks: &toks, at: 0 };
    // `s = { … }` — the name on the left is Lightroom's own and means nothing.
    if !matches!(p.next(), Some(Tok::Ident(_))) || p.next() != Some(&Tok::Eq) {
        return Err(Error::Malformed("not a Lightroom template".into()));
    }
    let Lua::Table(top) = p.value()? else {
        return Err(Error::Malformed("not a Lightroom template".into()));
    };
    let text_of = |t: &Table, k: &str| match t.get(k) {
        Some(Lua::Str(s)) if !s.trim().is_empty() => Some(s.clone()),
        _ => None,
    };
    let name = text_of(&top, "title").or_else(|| text_of(&top, "internalName"));
    let settings = match top.get("value") {
        Some(Lua::Table(v)) => match v.get("settings") {
            Some(Lua::Table(s)) => to_settings(s),
            _ => Settings::new(),
        },
        _ => Settings::new(),
    };
    if settings.is_empty() {
        return Err(Error::NotAPreset("it holds no develop settings"));
    }
    Ok(Raw {
        name,
        group: None,
        settings,
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Num(f64),
    Open,
    Close,
    LBracket,
    RBracket,
    Eq,
    Sep,
}

fn bad(what: &str) -> Error {
    Error::Malformed(what.to_string())
}

/// The tokens of a template: names, strings, numbers, `{ } [ ] = , ;` — comments dropped.
fn tokens(text: &str) -> Result<Vec<Tok>, Error> {
    let c: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        match ch {
            _ if ch.is_whitespace() => i += 1,
            '-' if c.get(i + 1) == Some(&'-') => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '{' | '}' | '[' | ']' | '=' | ',' | ';' => {
                out.push(match ch {
                    '{' => Tok::Open,
                    '}' => Tok::Close,
                    '[' => Tok::LBracket,
                    ']' => Tok::RBracket,
                    '=' => Tok::Eq,
                    _ => Tok::Sep,
                });
                i += 1;
            }
            '"' | '\'' => {
                let quote = ch;
                let mut s = String::new();
                i += 1;
                loop {
                    let Some(&x) = c.get(i) else {
                        return Err(bad("a string never ends"));
                    };
                    i += 1;
                    match x {
                        _ if x == quote => break,
                        '\\' => {
                            let Some(&e) = c.get(i) else {
                                return Err(bad("a string never ends"));
                            };
                            i += 1;
                            s.push(match e {
                                'n' => '\n',
                                't' => '\t',
                                'r' => '\r',
                                other => other,
                            });
                        }
                        _ => s.push(x),
                    }
                }
                out.push(Tok::Str(s));
            }
            _ if ch.is_ascii_digit() || ch == '-' || ch == '+' || ch == '.' => {
                let start = i;
                i += 1;
                while i < c.len()
                    && (c[i].is_ascii_alphanumeric() || matches!(c[i], '.' | '-' | '+'))
                {
                    // A sign continues the number only straight after an exponent.
                    if matches!(c[i], '-' | '+') && !matches!(c[i - 1], 'e' | 'E') {
                        break;
                    }
                    i += 1;
                }
                let lit: String = c[start..i].iter().collect();
                let n = lit
                    .parse::<f64>()
                    .map_err(|_| Error::Malformed(format!("not a number: {lit}")))?;
                out.push(Tok::Num(n));
            }
            _ if ch.is_alphabetic() || ch == '_' => {
                let start = i;
                while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                    i += 1;
                }
                out.push(Tok::Ident(c[start..i].iter().collect()));
            }
            other => return Err(Error::Malformed(format!("unexpected {other:?}"))),
        }
    }
    Ok(out)
}

/// A Lua value, as far as a template uses one.
#[derive(Debug, Clone, PartialEq)]
enum Lua {
    Num(f64),
    Str(String),
    Bool(bool),
    Nil,
    Table(Table),
}

/// A Lua table: its keyed entries and its positional ones.
#[derive(Debug, Clone, PartialEq, Default)]
struct Table {
    keyed: Vec<(String, Lua)>,
    list: Vec<Lua>,
}

impl Table {
    fn get(&self, k: &str) -> Option<&Lua> {
        self.keyed.iter().find(|(n, _)| n == k).map(|(_, v)| v)
    }
}

struct Parser<'a> {
    toks: &'a [Tok],
    at: usize,
}

impl Parser<'_> {
    fn next(&mut self) -> Option<&Tok> {
        let t = self.toks.get(self.at);
        self.at += 1;
        t
    }

    fn peek(&self, ahead: usize) -> Option<&Tok> {
        self.toks.get(self.at + ahead)
    }

    fn value(&mut self) -> Result<Lua, Error> {
        match self.next().cloned() {
            Some(Tok::Num(n)) => Ok(Lua::Num(n)),
            Some(Tok::Str(s)) => Ok(Lua::Str(s)),
            Some(Tok::Ident(w)) if w == "true" => Ok(Lua::Bool(true)),
            Some(Tok::Ident(w)) if w == "false" => Ok(Lua::Bool(false)),
            Some(Tok::Ident(w)) if w == "nil" => Ok(Lua::Nil),
            Some(Tok::Open) => self.table(),
            other => Err(Error::Malformed(format!("unexpected {other:?}"))),
        }
    }

    /// After `{`: entries up to the matching `}`.
    fn table(&mut self) -> Result<Lua, Error> {
        let mut t = Table::default();
        loop {
            match self.peek(0) {
                None => return Err(bad("a table never ends")),
                Some(Tok::Close) => {
                    self.at += 1;
                    return Ok(Lua::Table(t));
                }
                Some(Tok::Sep) => self.at += 1,
                Some(Tok::Ident(k)) if self.peek(1) == Some(&Tok::Eq) => {
                    let k = k.clone();
                    self.at += 2;
                    let v = self.value()?;
                    t.keyed.push((k, v));
                }
                Some(Tok::LBracket) => {
                    self.at += 1;
                    let key = match self.next().cloned() {
                        Some(Tok::Str(s)) => s,
                        Some(Tok::Num(n)) => n.to_string(),
                        other => return Err(Error::Malformed(format!("unexpected {other:?}"))),
                    };
                    if self.next() != Some(&Tok::RBracket) || self.next() != Some(&Tok::Eq) {
                        return Err(bad("a bracketed key without `] =`"));
                    }
                    let v = self.value()?;
                    t.keyed.push((key, v));
                }
                Some(_) => {
                    let v = self.value()?;
                    t.list.push(v);
                }
            }
        }
    }
}

/// A template's `value.settings` as settings: numbers, text, booleans, and a curve's list of numbers.
fn to_settings(t: &Table) -> Settings {
    let mut out = Settings::new();
    for (k, v) in &t.keyed {
        let v = match v {
            Lua::Num(n) => Value::Num(*n),
            Lua::Str(s) => Value::Text(s.clone()),
            Lua::Bool(b) => Value::Bool(*b),
            Lua::Nil => continue,
            Lua::Table(inner) if inner.keyed.is_empty() => Value::List(
                inner
                    .list
                    .iter()
                    .filter_map(|x| match x {
                        Lua::Num(n) => Some(Value::Num(*n)),
                        _ => None,
                    })
                    .collect(),
            ),
            // Structure — a mask, a correction — is present, not a value.
            Lua::Table(_) => Value::Bool(true),
        };
        out.insert(k.clone(), v);
    }
    out
}

/// Read an `.lrtemplate`'s text.
pub fn read(text: &str) -> Result<Raw, Error> {
    read_bytes(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    fn fixture() -> String {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/yuzukumo/minimal.lrtemplate");
        std::fs::read_to_string(p).unwrap()
    }

    /// The third party's sample: its title, numbers, text, and the curve's nested list.
    #[test]
    fn a_template_gives_its_title_and_settings() {
        let r = read(&fixture()).unwrap();
        assert_eq!(r.name.as_deref(), Some("Minimal Demo"));
        assert_eq!(r.group, None, "a template keeps no group");
        assert_eq!(r.settings["Exposure2012"], Value::Num(0.25));
        assert_eq!(r.settings["Contrast2012"], Value::Num(-10.0));
        assert_eq!(r.settings["Temperature"], Value::Num(5600.0));
        assert_eq!(r.settings["WhiteBalance"], Value::Text("Custom".into()));
        assert_eq!(
            r.settings["ToneCurvePV2012"].numbers(),
            [0.0, 0.0, 128.0, 140.0, 255.0, 255.0]
        );
        assert!(
            !r.settings.contains_key("id"),
            "only value.settings are settings"
        );
    }

    /// A byte-order mark is dropped; a Latin-1 title is read, not refused.
    #[test]
    fn a_bom_and_latin_1_are_read() {
        let text = fixture();
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(text.as_bytes());
        assert_eq!(read_bytes(&bom).unwrap(), read(&text).unwrap());
        let latin1: Vec<u8> = text
            .replace("Minimal Demo", "Caf\u{e9}")
            .chars()
            .map(|c| c as u32 as u8)
            .collect();
        assert_eq!(read_bytes(&latin1).unwrap().name.as_deref(), Some("Café"));
    }

    /// Lightroom's own escapes, booleans, a comment and a `["key"]`.
    #[test]
    fn strings_booleans_comments_and_bracket_keys() {
        let text = fixture().replace(
            "type = \"Develop\",",
            "type = \"Develop\", -- a comment\n    [\"odd key\"] = \"say \\\"hi\\\"\",",
        );
        let text = text.replace("Tint = 5,", "Tint = 5,\n            AutoTone = true,");
        let r = read(&text).unwrap();
        assert_eq!(r.settings["AutoTone"], Value::Bool(true));
        assert_eq!(r.name.as_deref(), Some("Minimal Demo"));
    }

    /// A truncated file is an error, never a panic.
    #[test]
    fn a_truncated_template_is_an_error() {
        let text = fixture();
        for cut in [3, text.len() / 2, text.len() - 3] {
            assert!(read(&text[..cut]).is_err(), "cut at {cut}");
        }
        assert!(read("not lua at all").is_err());
    }

    /// ⚠ Lightroom-written templates that cannot be committed (no licence file — see the fixtures'
    /// README): every one under `$LR_PRESET_SAMPLES` reads, with a name and at least one setting.
    /// Run by hand: `LR_PRESET_SAMPLES=<dir> cargo test --workspace -- --ignored real_lightroom_templates`.
    #[test]
    #[ignore]
    fn real_lightroom_templates() {
        let dir = std::env::var("LR_PRESET_SAMPLES").expect("LR_PRESET_SAMPLES names a folder");
        let mut stack = vec![std::path::PathBuf::from(dir)];
        let mut n = 0;
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "lrtemplate") {
                    let r = read_bytes(&std::fs::read(&p).unwrap())
                        .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                    assert!(r.name.is_some(), "{} has no name", p.display());
                    assert!(!r.settings.is_empty(), "{} has no settings", p.display());
                    n += 1;
                }
            }
        }
        assert!(n > 0, "the sample: no templates under LR_PRESET_SAMPLES");
        eprintln!("read {n} Lightroom-written templates");
    }
}
