//! The value side of `-c key=value`: TOML when it parses (strings, booleans, numbers, arrays,
//! inline tables), else the raw text as a string, which is what the CLI does.

use serde_json::{Map, Number, Value};

pub fn parse(raw: &str) -> Value {
    match parse_prefix(raw.trim()) {
        Some((v, used)) if raw.trim().chars().skip(used).all(char::is_whitespace) => v,
        _ => Value::String(raw.trim().to_string()),
    }
}

/// A TOML value at the start of `raw`, and how many characters it took (for `toml_doc`, where
/// a comment or the end of the line follows it).
pub fn parse_prefix(raw: &str) -> Option<(Value, usize)> {
    let chars: Vec<char> = raw.chars().collect();
    let mut p = P { c: &chars, i: 0 };
    let v = p.value()?;
    Some((v, p.i))
}

struct P<'a> {
    c: &'a [char],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.c.get(self.i).is_some_and(|c| c.is_whitespace()) {
            self.i += 1;
        }
    }
    fn eat(&mut self, ch: char) -> bool {
        self.ws();
        if self.c.get(self.i) == Some(&ch) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Option<Value> {
        self.ws();
        match *self.c.get(self.i)? {
            '"' | '\'' => self.string().map(Value::String),
            '[' => {
                self.i += 1;
                let mut out = vec![];
                while !self.eat(']') {
                    out.push(self.value()?);
                    self.eat(',');
                }
                Some(Value::Array(out))
            }
            '{' => {
                self.i += 1;
                let mut m = Map::new();
                while !self.eat('}') {
                    self.ws();
                    let k = match self.c.get(self.i)? {
                        '"' | '\'' => self.string()?,
                        _ => self.bare()?,
                    };
                    if !self.eat('=') {
                        return None;
                    }
                    let v = self.value()?;
                    m.insert(k, v);
                    self.eat(',');
                }
                Some(Value::Object(m))
            }
            _ => {
                let w = self.bare()?;
                match w.as_str() {
                    "true" => Some(Value::Bool(true)),
                    "false" => Some(Value::Bool(false)),
                    n => n
                        .replace('_', "")
                        .parse::<i64>()
                        .map(Value::from)
                        .ok()
                        .or_else(|| {
                            n.parse::<f64>()
                                .ok()
                                .and_then(Number::from_f64)
                                .map(Value::Number)
                        }),
                }
            }
        }
    }

    fn bare(&mut self) -> Option<String> {
        let s = self.i;
        while self
            .c
            .get(self.i)
            .is_some_and(|c| c.is_alphanumeric() || "_-.+".contains(*c))
        {
            self.i += 1;
        }
        (self.i > s).then(|| self.c[s..self.i].iter().collect())
    }

    fn string(&mut self) -> Option<String> {
        let q = self.c[self.i];
        self.i += 1;
        let mut out = String::new();
        loop {
            let ch = *self.c.get(self.i)?;
            self.i += 1;
            if ch == q {
                return Some(out);
            }
            if ch == '\\' && q == '"' {
                let e = *self.c.get(self.i)?;
                self.i += 1;
                match e {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'u' => {
                        let h: String = self.c.get(self.i..self.i + 4)?.iter().collect();
                        self.i += 4;
                        out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?)?);
                    }
                    other => out.push(other),
                }
            } else {
                out.push(ch);
            }
        }
    }
}
