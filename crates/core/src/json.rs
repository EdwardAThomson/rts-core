//! A small JSON reader for the data files both engines load (rules data, setting packs), so neither needs a
//! third-party crate. It started as the Classic engine's reader (`classic-data`) and moved here for the 3D engine's
//! setting packs.
//!
//! Numbers must be whole: a fraction or exponent is an error, because every number the simulation reads is an
//! integer. Objects keep their keys in file order and refuse a key given twice.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Object(fields) => Some(fields),
            _ => None,
        }
    }

    /// What kind of value this is, for error messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "a true/false value",
            Value::Int(_) => "a number",
            Value::Str(_) => "a string",
            Value::Array(_) => "a list",
            Value::Object(_) => "an object",
        }
    }
}

/// Where and why a text is not valid JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "line {}, column {}: {}", self.line, self.column, self.message)
    }
}

pub fn parse(text: &str) -> Result<Value, ParseError> {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    p.space();
    let v = p.value(0)?;
    p.space();
    if p.i < p.s.len() {
        return Err(p.error("unexpected text after the end"));
    }
    Ok(v)
}

/// Deep enough for any data file; stops a malformed one from overflowing the stack.
const MAX_DEPTH: usize = 64;

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> ParseError {
        let before = &self.s[..self.i.min(self.s.len())];
        let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
        let column = before.iter().rev().take_while(|&&b| b != b'\n').count() + 1;
        ParseError { line, column, message: message.into() }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), ParseError> {
        if self.peek() == Some(b) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected '{}'", b as char)))
        }
    }

    fn word(&mut self, w: &str, v: Value) -> Result<Value, ParseError> {
        if self.s[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            Ok(v)
        } else {
            Err(self.error("unexpected text"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.error("nested too deeply"));
        }
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') => self.word("true", Value::Bool(true)),
            Some(b'f') => self.word("false", Value::Bool(false)),
            Some(b'n') => self.word("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("unexpected character")),
            None => Err(self.error("unexpected end of text")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.expect(b'{')?;
        let mut fields: Vec<(String, Value)> = Vec::new();
        self.space();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Value::Object(fields));
        }
        loop {
            self.space();
            if self.peek() != Some(b'"') {
                return Err(self.error("expected a key in double quotes"));
            }
            let key = self.string()?;
            if fields.iter().any(|(k, _)| *k == key) {
                return Err(self.error(&format!("key \"{key}\" given twice")));
            }
            self.space();
            self.expect(b':')?;
            self.space();
            let v = self.value(depth + 1)?;
            fields.push((key, v));
            self.space();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Object(fields));
                }
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, ParseError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.space();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.space();
            items.push(self.value(depth + 1)?);
            self.space();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn number(&mut self) -> Result<Value, ParseError> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        let digits = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        if self.i == digits {
            return Err(self.error("expected a digit"));
        }
        if self.s[digits] == b'0' && self.i - digits > 1 {
            return Err(self.error("a number may not start with 0"));
        }
        if matches!(self.peek(), Some(b'.' | b'e' | b'E')) {
            return Err(self.error("numbers must be whole: the simulation uses integers only"));
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).expect("ASCII digits");
        text.parse().map(Value::Int).map_err(|_| self.error("number out of range"))
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let mut n = 0;
        for _ in 0..4 {
            let d = match self.peek() {
                Some(c @ b'0'..=b'9') => c - b'0',
                Some(c @ b'a'..=b'f') => c - b'a' + 10,
                Some(c @ b'A'..=b'F') => c - b'A' + 10,
                _ => return Err(self.error("expected four hex digits")),
            };
            n = n * 16 + d as u32;
            self.i += 1;
        }
        Ok(n)
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            match self.peek() {
                None => return Err(self.error("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return String::from_utf8(out).map_err(|_| self.error("invalid UTF-8"));
                }
                Some(c) if c < 0x20 => return Err(self.error("control character in string")),
                Some(b'\\') => {
                    self.i += 1;
                    let Some(e) = self.peek() else { return Err(self.error("unterminated string")) };
                    self.i += 1;
                    let c = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&hi) {
                                if !self.s[self.i..].starts_with(b"\\u") {
                                    return Err(self.error("unpaired surrogate"));
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return Err(self.error("unpaired surrogate"));
                                }
                                0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                            } else {
                                hi
                            };
                            char::from_u32(code).ok_or_else(|| self.error("invalid escape"))?
                        }
                        _ => return Err(self.error("invalid escape")),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_shapes_data_files_use() {
        let v = parse(r#" { "a": [1, -2, true, null], "b": { "c": "x\"é😀" } } "#).unwrap();
        assert_eq!(v.get("a").unwrap().as_array().unwrap()[1], Value::Int(-2));
        assert_eq!(v.get("b").unwrap().get("c").unwrap().as_str(), Some("x\"é😀"));
    }

    #[test]
    fn refuses_fractions_duplicates_and_junk() {
        assert!(parse("1.5").unwrap_err().message.contains("whole"));
        assert!(parse("2e3").is_err());
        assert!(parse(r#"{"a":1,"a":2}"#).unwrap_err().message.contains("twice"));
        assert!(parse("[1,]").is_err());
        assert!(parse("{} x").is_err());
        assert!(parse("012").is_err());
        let e = parse("{\n  \"a\": ?\n}").unwrap_err();
        assert_eq!((e.line, e.column), (2, 8));
    }
}
