//! The state hash: FNV-1a over a canonical JSON text of the whole state. Equal hashes mean equal games, which is
//! what determinism, replay and (later) lockstep checks compare.
//!
//! The text is never built; it is streamed straight into the hash. Object keys must be written in sorted order
//! (the order a JavaScript string sort gives, which is byte order for the ASCII keys used here) and a missing
//! optional field is left out, so the text, and the hash, are exactly those of the Classic engine's first, TypeScript version.
//! Rust's integer types already rule out the fractional numbers the TypeScript version had to check for.

/// Something that can write itself into the canonical text.
pub trait Canon {
    fn canon(&self, w: &mut CanonHasher);
}

/// A streaming FNV-1a hasher over canonical JSON.
pub struct CanonHasher {
    h: u32,
}

impl Default for CanonHasher {
    fn default() -> Self {
        Self { h: 0x811c_9dc5 }
    }
}

impl CanonHasher {
    pub fn new() -> Self {
        Self::default()
    }

    /// The hash as eight lowercase hex digits.
    pub fn hex(&self) -> String {
        format!("{:08x}", self.h)
    }

    pub fn value(&self) -> u32 {
        self.h
    }

    /// Feed one UTF-16 code unit, as the TypeScript engine's `charCodeAt` did.
    fn unit(&mut self, c: u16) {
        self.h = (self.h ^ c as u32).wrapping_mul(0x0100_0193);
    }

    /// Feed raw text, unquoted.
    pub fn raw(&mut self, text: &str) {
        for c in text.encode_utf16() {
            self.unit(c);
        }
    }

    pub fn int(&mut self, v: i64) {
        let mut buf = [0u8; 20];
        let mut i = buf.len();
        let mut n = v.unsigned_abs();
        loop {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        if v < 0 {
            self.unit(b'-' as u16);
        }
        for &b in &buf[i..] {
            self.unit(b as u16);
        }
    }

    /// A JSON string, quoted and escaped as `JSON.stringify` does.
    pub fn string(&mut self, s: &str) {
        self.unit(b'"' as u16);
        for c in s.encode_utf16() {
            match c {
                0x22 => self.raw("\\\""),
                0x5c => self.raw("\\\\"),
                0x08 => self.raw("\\b"),
                0x0c => self.raw("\\f"),
                0x0a => self.raw("\\n"),
                0x0d => self.raw("\\r"),
                0x09 => self.raw("\\t"),
                c if c < 0x20 => {
                    const HEX: &[u8; 16] = b"0123456789abcdef";
                    self.raw("\\u00");
                    self.unit(HEX[(c >> 4) as usize] as u16);
                    self.unit(HEX[(c & 15) as usize] as u16);
                }
                c => self.unit(c),
            }
        }
        self.unit(b'"' as u16);
    }

    /// Start an object; write its fields in sorted key order, then call `end`.
    pub fn object(&mut self) -> Object<'_> {
        self.unit(b'{' as u16);
        Object { w: self, last: None }
    }

    /// An array of items.
    pub fn array<'a, T: Canon + 'a>(&mut self, items: impl IntoIterator<Item = &'a T>) {
        self.unit(b'[' as u16);
        for (i, item) in items.into_iter().enumerate() {
            if i > 0 {
                self.unit(b',' as u16);
            }
            item.canon(self);
        }
        self.unit(b']' as u16);
    }
}

/// An object being written. Keys must arrive in strictly increasing order; a debug build checks it.
pub struct Object<'a> {
    w: &'a mut CanonHasher,
    last: Option<&'static str>,
}

impl Object<'_> {
    fn key(&mut self, k: &'static str) {
        debug_assert!(self.last.is_none_or(|l| l < k), "keys out of order: {:?} then {k:?}", self.last);
        if self.last.is_some() {
            self.w.unit(b',' as u16);
        }
        self.last = Some(k);
        self.w.string(k);
        self.w.unit(b':' as u16);
    }

    pub fn field<T: Canon + ?Sized>(&mut self, k: &'static str, v: &T) -> &mut Self {
        self.key(k);
        v.canon(self.w);
        self
    }

    /// A field that is left out entirely when absent, like an `undefined` property in the TypeScript engine.
    pub fn opt<T: Canon>(&mut self, k: &'static str, v: Option<&T>) -> &mut Self {
        if let Some(v) = v {
            self.field(k, v);
        }
        self
    }

    pub fn array<'b, T: Canon + 'b>(&mut self, k: &'static str, items: impl IntoIterator<Item = &'b T>) -> &mut Self {
        self.key(k);
        self.w.array(items);
        self
    }

    pub fn end(&mut self) {
        self.w.unit(b'}' as u16);
    }
}

macro_rules! canon_int {
    ($($t:ty),*) => {$(
        impl Canon for $t {
            fn canon(&self, w: &mut CanonHasher) { w.int(*self as i64); }
        }
    )*};
}
canon_int!(i8, i16, i32, i64, u8, u16, u32);

impl Canon for str {
    fn canon(&self, w: &mut CanonHasher) {
        w.string(self);
    }
}

impl Canon for bool {
    fn canon(&self, w: &mut CanonHasher) {
        w.raw(if *self { "true" } else { "false" });
    }
}

impl<T: Canon> Canon for [T] {
    fn canon(&self, w: &mut CanonHasher) {
        w.array(self);
    }
}

impl<T: Canon> Canon for Vec<T> {
    fn canon(&self, w: &mut CanonHasher) {
        w.array(self);
    }
}

/// Hash any value.
pub fn hash_of<T: Canon + ?Sized>(v: &T) -> CanonHasher {
    let mut w = CanonHasher::new();
    v.canon(&mut w);
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv_text(text: &str) -> String {
        let mut w = CanonHasher::new();
        w.raw(text);
        w.hex()
    }

    struct Small;
    impl Canon for Small {
        fn canon(&self, w: &mut CanonHasher) {
            let empty: [i64; 0] = [];
            w.object()
                .array("entities", &empty)
                .field("nextId", &2)
                .array("players", &empty)
                .field("resource", &vec![0i64, 300])
                .field("rng", &-123)
                .field("tick", &5)
                .end();
        }
    }

    #[test]
    fn streams_exactly_the_canonical_text() {
        let text = r#"{"entities":[],"nextId":2,"players":[],"resource":[0,300],"rng":-123,"tick":5}"#;
        assert_eq!(hash_of(&Small).hex(), fnv_text(text));
    }

    #[test]
    fn strings_are_escaped_as_json_stringify_does() {
        let mut w = CanonHasher::new();
        w.string("a\"b\\c\nd\u{1}é");
        assert_eq!(w.hex(), fnv_text(r#""a\"b\\c\nd\u0001é""#));
    }

    #[test]
    #[should_panic(expected = "keys out of order")]
    fn keys_out_of_order_are_caught() {
        let mut w = CanonHasher::new();
        w.object().field("b", &1).field("a", &2).end();
    }
}
