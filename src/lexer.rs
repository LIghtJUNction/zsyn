use crate::{unicode, ErrorKind, Mode, Span, SyntaxKind as K};

/// A zero-copy lexical item, located by UTF-8 byte offsets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: K,
    pub span: Span,
    pub error: Option<ErrorKind>,
}

/// A mode-aware, allocation-free lexer. Nested mode transitions are driven by the parser.
///
/// Inputs with at least `u32::MAX` bytes cannot be represented by [`Span`].
/// Instead of reading them, the iterator emits one empty error token at byte
/// zero with [`ErrorKind::SourceLimit`], then ends.
#[derive(Clone, Debug)]
pub struct Lexer<'s> {
    source: &'s str,
    mode: Mode,
    pos: usize,
}

/// Unicode 18 XID_Start with Typst's underscore extension.
#[inline]
pub fn is_id_start(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphabetic() || c == '_'
    } else {
        unicode::contains(unicode::XID_START, c)
    }
}

/// Unicode 18 XID_Continue with Typst's hyphen extension.
#[inline]
pub fn is_id_continue(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphanumeric() || c == '_' || c == '-'
    } else {
        unicode::contains(unicode::XID_CONTINUE, c)
    }
}

#[inline]
pub(crate) fn is_newline(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{000b}' | '\u{000c}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

#[inline]
fn is_space(c: char, mode: Mode) -> bool {
    if mode == Mode::Markup {
        c == ' ' || c == '\t' || is_newline(c)
    } else {
        c.is_whitespace()
    }
}

impl<'s> Lexer<'s> {
    pub const fn new(source: &'s str, mode: Mode) -> Self {
        Self {
            source,
            mode,
            pos: 0,
        }
    }
    /// Starts at a byte offset, rounding an invalid offset down to a UTF-8 boundary.
    pub fn at(source: &'s str, mode: Mode, offset: usize) -> Self {
        let mut lexer = Self::new(source, mode);
        lexer.set_position(offset);
        lexer
    }
    /// Internal cursors are already checked token boundaries.
    #[inline]
    pub(crate) const fn at_boundary(source: &'s str, mode: Mode, pos: usize) -> Self {
        Self { source, mode, pos }
    }
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }
    pub const fn mode(&self) -> Mode {
        self.mode
    }
    pub const fn position(&self) -> usize {
        self.pos
    }
    /// Clamps to the source and rounds down to the preceding UTF-8 boundary.
    pub fn set_position(&mut self, offset: usize) {
        self.pos = offset.min(self.source.len());
        while !self.source.is_char_boundary(self.pos) {
            self.pos -= 1;
        }
    }
    fn rest(&self) -> &'s str {
        &self.source[self.pos..]
    }
    fn ch(&self) -> Option<char> {
        self.rest().chars().next()
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.ch()?;
        self.pos += c.len_utf8();
        Some(c)
    }
    fn starts(&self, s: &str) -> bool {
        self.rest().starts_with(s)
    }
    fn token(&self, start: usize, kind: K, error: Option<ErrorKind>) -> Token {
        Token {
            kind,
            span: Span::new(start as u32, self.pos as u32),
            error,
        }
    }
    fn space(&mut self) -> K {
        let mut newlines = 0;
        let mut cr = false;
        let bytes = self.source.as_bytes();
        while let Some(&b) = bytes.get(self.pos) {
            let c = if b < 128 {
                if b != b' ' && !(b'\t'..=b'\r').contains(&b) {
                    break;
                }
                self.pos += 1;
                b as char
            } else {
                let c = self.ch().unwrap_or('\0');
                if !is_space(c, self.mode) {
                    break;
                }
                self.pos += c.len_utf8();
                c
            };
            if newlines < 2 && is_newline(c) && !(c == '\n' && cr) {
                newlines += 1;
            }
            cr = c == '\r';
        }
        if self.mode == Mode::Markup && newlines >= 2 {
            K::Parbreak
        } else {
            K::Space
        }
    }
    fn line_comment(&mut self) {
        while let Some(c) = self.ch() {
            if is_newline(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
    }
    fn block_comment(&mut self) -> Option<ErrorKind> {
        self.pos += 2;
        let mut depth = 1usize;
        let b = self.source.as_bytes();
        while self.pos < b.len() {
            if b[self.pos] == b'/' && b.get(self.pos + 1) == Some(&b'*') {
                depth += 1;
                self.pos += 2;
            } else if b[self.pos] == b'*' && b.get(self.pos + 1) == Some(&b'/') {
                depth -= 1;
                self.pos += 2;
                if depth == 0 {
                    return None;
                }
            } else {
                self.pos += 1;
            }
        }
        Some(ErrorKind::Unterminated)
    }
    fn escape(&mut self) -> (K, Option<ErrorKind>) {
        self.pos += 1;
        match self.ch() {
            None => (K::Linebreak, None),
            Some(c) if c.is_whitespace() => (K::Linebreak, None),
            Some('u') if self.starts("u{") => {
                self.pos += 2;
                let start = self.pos;
                let mut value = 0u32;
                let mut valid = true;
                while let Some(c) = self.ch() {
                    if let Some(d) = c.to_digit(16) {
                        value = match value.checked_mul(16).and_then(|v| v.checked_add(d)) {
                            Some(v) => v,
                            None => {
                                valid = false;
                                0
                            }
                        };
                        self.pos += 1;
                    } else if c.is_ascii_alphanumeric() {
                        valid = false;
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                valid &= self.pos > start && char::from_u32(value).is_some();
                if self.starts("}") {
                    self.pos += 1;
                } else {
                    valid = false;
                }
                if valid {
                    (K::Escape, None)
                } else {
                    (K::Error, Some(ErrorKind::InvalidEscape))
                }
            }
            Some(c) => {
                self.pos += c.len_utf8();
                (K::Escape, None)
            }
        }
    }
    fn string(&mut self) -> Option<ErrorKind> {
        self.pos += 1;
        let bytes = self.source.as_bytes();
        while self.pos < bytes.len() {
            match bytes[self.pos] {
                b'"' => {
                    self.pos += 1;
                    return None;
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos < bytes.len() {
                        self.bump();
                    }
                }
                _ => self.pos += 1,
            }
        }
        Some(ErrorKind::Unterminated)
    }
    fn raw(&mut self) -> Option<ErrorKind> {
        let bytes = self.source.as_bytes();
        let start = self.pos;
        while bytes.get(self.pos) == Some(&b'`') {
            self.pos += 1;
        }
        let fence = self.pos - start;
        if fence == 2 {
            return None;
        }
        while self.pos < bytes.len() {
            if bytes[self.pos] == b'`' {
                let begin = self.pos;
                while bytes.get(self.pos) == Some(&b'`') {
                    self.pos += 1;
                }
                if self.pos - begin >= fence {
                    self.pos = begin + fence;
                    return None;
                }
            } else {
                self.pos += 1;
            }
        }
        Some(ErrorKind::Unterminated)
    }
    fn label(&mut self) -> bool {
        let start = self.pos;
        self.pos += 1;
        while let Some(c) = self.ch() {
            if is_id_continue(c) || matches!(c, ':' | '.') {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        if self.pos > start + 1 && self.starts(">") {
            self.pos += 1;
            true
        } else {
            self.pos = start;
            false
        }
    }
    fn reference(&mut self) -> bool {
        let start = self.pos;
        self.pos += 1;
        while let Some(c) = self.ch() {
            if is_id_continue(c) || matches!(c, ':' | '.') {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        // A sentence's final punctuation is not part of a reference target.
        while self.pos > start + 1 && matches!(self.source.as_bytes()[self.pos - 1], b'.' | b':') {
            self.pos -= 1;
        }
        if self.pos > start + 1 {
            true
        } else {
            self.pos = start;
            false
        }
    }
    fn link(&mut self) {
        let mut parens = 0usize;
        let mut brackets = 0usize;
        while let Some(c) = self.ch() {
            if c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`') {
                break;
            }
            match c {
                '(' => parens += 1,
                ')' => {
                    if parens == 0 {
                        break;
                    }
                    parens -= 1;
                }
                '[' => brackets += 1,
                ']' => {
                    if brackets == 0 {
                        break;
                    }
                    brackets -= 1;
                }
                _ => {}
            }
            self.pos += c.len_utf8();
        }
        while self.pos > 0
            && matches!(
                self.source.as_bytes()[self.pos - 1],
                b'.' | b',' | b';' | b':' | b'!' | b'?'
            )
        {
            self.pos -= 1;
        }
    }
    fn number(&mut self) -> (K, Option<ErrorKind>) {
        let start = self.pos;
        let b = self.source.as_bytes();
        let mut float = false;
        let mut valid = true;
        let mut radix = 10;
        if b.get(start) == Some(&b'0') {
            radix = match b.get(start + 1) {
                Some(b'x') => 16,
                Some(b'o') => 8,
                Some(b'b') => 2,
                _ => 10,
            };
        }
        if radix != 10 {
            self.pos += 2;
            let digits = self.pos;
            while b.get(self.pos).is_some_and(u8::is_ascii_alphanumeric) {
                self.pos += 1;
            }
            valid = self.pos > digits
                && self.source[digits..self.pos]
                    .chars()
                    .all(|c| c.is_digit(radix));
        } else {
            while b.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
            if b.get(self.pos) == Some(&b'.')
                && b.get(self.pos + 1) != Some(&b'.')
                && !b
                    .get(self.pos + 1)
                    .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
            {
                float = true;
                self.pos += 1;
                while b.get(self.pos).is_some_and(u8::is_ascii_digit) {
                    self.pos += 1;
                }
            }
            if matches!(b.get(self.pos), Some(b'e' | b'E'))
                && !(b.get(self.pos) == Some(&b'e') && b.get(self.pos + 1) == Some(&b'm'))
            {
                float = true;
                self.pos += 1;
                if matches!(b.get(self.pos), Some(b'+' | b'-')) {
                    self.pos += 1;
                }
                let digits = self.pos;
                while b.get(self.pos).is_some_and(u8::is_ascii_digit) {
                    self.pos += 1;
                }
                valid &= self.pos > digits;
            }
        }
        let suffix = self.pos;
        while b.get(self.pos).is_some_and(u8::is_ascii_alphanumeric) {
            self.pos += 1;
        }
        if b.get(self.pos) == Some(&b'%') {
            self.pos += 1;
        }
        let unit = &self.source[suffix..self.pos];
        if !float
            && radix == 10
            && suffix - start >= 19
            && self.source[start..suffix].parse::<i64>().is_err()
        {
            float = true;
        }
        if radix != 10 && i64::from_str_radix(&self.source[start + 2..suffix], radix).is_err() {
            valid = false;
        }
        let kind = if unit.is_empty() {
            if float {
                K::Float
            } else {
                K::Int
            }
        } else if matches!(
            unit,
            "pt" | "mm" | "cm" | "in" | "em" | "deg" | "rad" | "fr" | "%"
        ) {
            K::Numeric
        } else {
            valid = false;
            K::Error
        };
        if valid {
            (kind, None)
        } else {
            (K::Error, Some(ErrorKind::InvalidNumber))
        }
    }
    #[inline]
    fn identifier(&mut self, math: bool) -> K {
        let start = self.pos;
        let bytes = self.source.as_bytes();
        let mut unicode = bytes[start] >= 128;
        if unicode {
            self.bump();
        } else {
            self.pos += 1;
        }
        while let Some(&b) = bytes.get(self.pos) {
            if b < 128 {
                if b.is_ascii_alphanumeric() || (!math && matches!(b, b'_' | b'-')) {
                    self.pos += 1;
                } else {
                    break;
                }
            } else {
                unicode = true;
                let c = self.ch().unwrap_or('\0');
                if !is_id_continue(c) {
                    break;
                }
                self.pos += c.len_utf8();
            }
        }
        if math {
            if !unicode {
                return if self.pos - start == 1 {
                    K::MathText
                } else {
                    K::MathIdent
                };
            }
            let first = crate::grapheme::first_len(&self.source[start..]);
            if first >= self.pos - start {
                self.pos = start + first;
                K::MathText
            } else {
                K::MathIdent
            }
        } else {
            keyword(&self.source[start..self.pos])
        }
    }
    fn code(&mut self) -> (K, Option<ErrorKind>) {
        let b = self.source.as_bytes()[self.pos];
        if b.is_ascii_digit()
            || (b == b'.'
                && self
                    .source
                    .as_bytes()
                    .get(self.pos + 1)
                    .is_some_and(u8::is_ascii_digit))
        {
            return self.number();
        }
        if b.is_ascii_alphabetic() || b == b'_' || (b >= 128 && self.ch().is_some_and(is_id_start))
        {
            return (self.identifier(false), None);
        }
        if b == b'"' {
            let e = self.string();
            return (if e.is_some() { K::Error } else { K::Str }, e);
        }
        if b == b'<' && self.label() {
            return (K::Label, None);
        }
        let pair = self.source.as_bytes().get(self.pos..self.pos + 2);
        if matches!(pair, Some(b"&&" | b"||" | b"~=")) {
            self.pos += 2;
            return (K::Error, Some(ErrorKind::UnexpectedToken));
        }
        let kind = match pair {
            Some(b"==") => Some(K::EqEq),
            Some(b"!=") => Some(K::ExclEq),
            Some(b"<=") => Some(K::LtEq),
            Some(b">=") => Some(K::GtEq),
            Some(b"+=") => Some(K::PlusEq),
            Some(b"-=") => Some(K::HyphEq),
            Some(b"*=") => Some(K::StarEq),
            Some(b"/=") => Some(K::SlashEq),
            Some(b"..") => Some(K::Dots),
            Some(b"=>") => Some(K::Arrow),
            _ => None,
        };
        if let Some(k) = kind {
            self.pos += 2;
            return (k, None);
        }
        self.bump();
        let k = match b {
            b'{' => K::LeftBrace,
            b'}' => K::RightBrace,
            b'[' => K::LeftBracket,
            b']' => K::RightBracket,
            b'(' => K::LeftParen,
            b')' => K::RightParen,
            b',' => K::Comma,
            b';' => K::Semicolon,
            b':' => K::Colon,
            b'*' => K::Star,
            b'$' => K::Dollar,
            b'+' => K::Plus,
            b'-' => K::Minus,
            b'/' => K::Slash,
            b'.' => K::Dot,
            b'=' => K::Eq,
            b'<' => K::Lt,
            b'>' => K::Gt,
            b'#' => K::Hash,
            _ => K::Error,
        };
        (
            k,
            if k == K::Error {
                Some(ErrorKind::UnexpectedToken)
            } else {
                None
            },
        )
    }
    fn math(&mut self) -> (K, Option<ErrorKind>) {
        let b = self.source.as_bytes()[self.pos];
        let c = if b < 128 {
            b as char
        } else {
            self.ch().unwrap_or('\0')
        };
        if c == '"' {
            let e = self.string();
            return (if e.is_some() { K::Error } else { K::Str }, e);
        }
        if c != '_' && is_id_start(c) {
            return (self.identifier(true), None);
        }
        if c.is_ascii_digit() {
            let b = self.source.as_bytes();
            while b.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
            if b.get(self.pos) == Some(&b'.') && b.get(self.pos + 1).is_some_and(u8::is_ascii_digit)
            {
                self.pos += 1;
                while b.get(self.pos).is_some_and(u8::is_ascii_digit) {
                    self.pos += 1;
                }
            }
            return (K::MathText, None);
        }
        if let Some(n) = math_shorthand(self.rest()) {
            self.pos += n;
            return (K::MathShorthand, None);
        }
        let cluster_len = crate::grapheme::first_len(self.rest());
        self.pos += cluster_len;
        let kind = match c {
            '#' => K::Hash,
            '$' => K::Dollar,
            '_' => K::Underscore,
            '^' => K::Hat,
            '/' => K::Slash,
            '(' => K::LeftParen,
            ')' => K::RightParen,
            '[' => K::LeftBracket,
            ']' => K::RightBracket,
            '{' => K::LeftBrace,
            '}' => K::RightBrace,
            '&' => K::MathAlignPoint,
            '!' => K::Bang,
            ',' => K::Comma,
            ';' => K::Semicolon,
            ':' => K::Colon,
            '.' => K::Dot,
            '√' | '∛' | '∜' => K::Root,
            '\'' => {
                while self.starts("'") {
                    self.pos += 1;
                }
                K::MathPrimes
            }
            _ => K::MathText,
        };
        (kind, None)
    }
    fn markup(&mut self) -> (K, Option<ErrorKind>) {
        let start = self.pos;
        let b = self.source.as_bytes()[start];
        match b {
            b'#' => {
                self.pos += 1;
                return (K::Hash, None);
            }
            b'$' => {
                self.pos += 1;
                return (K::Dollar, None);
            }
            b'[' => {
                self.pos += 1;
                return (K::LeftBracket, None);
            }
            b']' => {
                self.pos += 1;
                return (K::RightBracket, None);
            }
            b':' => {
                self.pos += 1;
                return (K::Colon, None);
            }
            b'"' | b'\'' => {
                self.pos += 1;
                return (K::SmartQuote, None);
            }
            b'<' if self.label() => return (K::Label, None),
            b'@' if self.reference() => return (K::RefMarker, None),
            b'~' => {
                self.pos += 1;
                return (K::Shorthand, None);
            }
            b'-' if self.starts("---") => {
                self.pos += 3;
                return (K::Shorthand, None);
            }
            b'-' if self.starts("--") || self.starts("-?") => {
                self.pos += 2;
                return (K::Shorthand, None);
            }
            b'-' if self.source[start + 1..]
                .chars()
                .next()
                .is_some_and(char::is_numeric) =>
            {
                self.pos += 1;
                return (K::Shorthand, None);
            }
            b'.' if self.starts("...") => {
                self.pos += 3;
                return (K::Shorthand, None);
            }
            b'=' => {
                while self.starts("=") {
                    self.pos += 1;
                }
                if self.ch().map_or(true, char::is_whitespace)
                    || self.starts("//")
                    || self.starts("/*")
                {
                    return (K::HeadingMarker, None);
                }
                self.pos = start;
            }
            b'-' | b'+' | b'/'
                if self.source[start + 1..]
                    .chars()
                    .next()
                    .map_or(true, char::is_whitespace)
                    || self.source[start + 1..].starts_with("//")
                    || self.source[start + 1..].starts_with("/*") =>
            {
                self.pos += 1;
                return (
                    match b {
                        b'-' => K::ListMarker,
                        b'+' => K::EnumMarker,
                        _ => K::TermMarker,
                    },
                    None,
                );
            }
            b'*' | b'_' => {
                let prev = self.source[..start].chars().next_back();
                let next = self.source[start + 1..].chars().next();
                if !(prev.is_some_and(char::is_alphanumeric)
                    && next.is_some_and(char::is_alphanumeric))
                {
                    self.pos += 1;
                    return (if b == b'*' { K::Star } else { K::Underscore }, None);
                }
            }
            b'0'..=b'9' => {
                while self
                    .source
                    .as_bytes()
                    .get(self.pos)
                    .is_some_and(u8::is_ascii_digit)
                {
                    self.pos += 1;
                }
                if self.starts(".")
                    && (self.source[self.pos + 1..]
                        .chars()
                        .next()
                        .map_or(true, char::is_whitespace)
                        || self.source[self.pos + 1..].starts_with("//")
                        || self.source[self.pos + 1..].starts_with("/*"))
                {
                    self.pos += 1;
                    return (K::EnumMarker, None);
                }
                self.pos = start;
            }
            _ => {}
        }
        if self.starts("https://") || self.starts("http://") {
            self.link();
            return (K::Link, None);
        }
        // Plain text includes isolated ASCII spaces, preserving Typst's text runs.
        self.bump();
        while self.pos < self.source.len() {
            let b = self.source.as_bytes()[self.pos];
            if b == b' ' {
                let after = self.source.as_bytes().get(self.pos + 1).copied();
                if after.is_some_and(|c| {
                    c.is_ascii_alphanumeric()
                        || (c >= 128
                            && self.source[self.pos + 1..]
                                .chars()
                                .next()
                                .is_some_and(char::is_alphanumeric))
                }) {
                    self.pos += 1;
                    continue;
                }
                break;
            }
            if b < 128 {
                if b.is_ascii_whitespace() || b == 0x0b {
                    break;
                }
                if markup_special(b) {
                    let plain = match b {
                        b'+' | b'=' => true,
                        b'-' => !self.starts("--") && !self.starts("-?"),
                        b'/' => !self.starts("//") && !self.starts("/*"),
                        _ => false,
                    };
                    if !plain {
                        break;
                    }
                }
                if b == b'h' && (self.starts("http://") || self.starts("https://")) {
                    break;
                }
                if b == b'.' && self.starts("...") {
                    break;
                }
                self.pos += 1;
            } else {
                let c = self.ch().unwrap_or('\0');
                if c.is_whitespace() {
                    break;
                }
                self.pos += c.len_utf8();
            }
        }
        (K::Text, None)
    }
}

impl Iterator for Lexer<'_> {
    type Item = Token;
    #[inline]
    fn next(&mut self) -> Option<Token> {
        if self.pos >= self.source.len() {
            return None;
        }
        if self.source.len() >= u32::MAX as usize {
            self.pos = self.source.len();
            return Some(Token {
                kind: K::Error,
                span: Span::new(0, 0),
                error: Some(ErrorKind::SourceLimit),
            });
        }
        let start = self.pos;
        let first = self.source.as_bytes()[self.pos];
        if (first < 128 && (first.is_ascii_whitespace() || first == 0x0b))
            || (first >= 128 && self.ch().is_some_and(|c| is_space(c, self.mode)))
        {
            let kind = self.space();
            return Some(self.token(start, kind, None));
        }
        if self.pos == 0 && self.starts("#!") {
            self.line_comment();
            return Some(self.token(start, K::Shebang, None));
        }
        if first == b'/' && self.starts("//") {
            self.line_comment();
            return Some(self.token(start, K::LineComment, None));
        }
        if first == b'/' && self.starts("/*") {
            let error = self.block_comment();
            return Some(self.token(start, K::BlockComment, error));
        }
        if first == b'*' && self.starts("*/") {
            self.pos += 2;
            return Some(self.token(start, K::Error, Some(ErrorKind::UnexpectedToken)));
        }
        if first == b'`' {
            let error = self.raw();
            return Some(self.token(
                start,
                if error.is_some() { K::Error } else { K::Raw },
                error,
            ));
        }
        if first == b'\\' && self.mode != Mode::Code {
            let (kind, error) = self.escape();
            return Some(self.token(start, kind, error));
        }
        let (kind, error) = match self.mode {
            Mode::Markup => self.markup(),
            Mode::Code => self.code(),
            Mode::Math => self.math(),
        };
        Some(self.token(start, kind, error))
    }
}
impl core::iter::FusedIterator for Lexer<'_> {}

#[inline]
fn markup_special(b: u8) -> bool {
    matches!(
        b,
        b'#' | b'$'
            | b'['
            | b']'
            | b':'
            | b'"'
            | b'\''
            | b'<'
            | b'>'
            | b'@'
            | b'~'
            | b'-'
            | b'+'
            | b'/'
            | b'='
            | b'*'
            | b'_'
            | b'`'
            | b'\\'
    )
}

fn keyword(s: &str) -> K {
    match s {
        "_" => K::Underscore,
        "not" => K::Not,
        "and" => K::And,
        "or" => K::Or,
        "none" => K::None,
        "auto" => K::Auto,
        "let" => K::Let,
        "set" => K::Set,
        "show" => K::Show,
        "context" => K::Context,
        "if" => K::If,
        "else" => K::Else,
        "for" => K::For,
        "in" => K::In,
        "while" => K::While,
        "break" => K::Break,
        "continue" => K::Continue,
        "return" => K::Return,
        "import" => K::Import,
        "include" => K::Include,
        "as" => K::As,
        "true" | "false" => K::Bool,
        _ => K::Ident,
    }
}

fn math_shorthand(s: &str) -> Option<usize> {
    // Dispatch before matching: common delimiters and attachments need no search.
    // Candidates within each branch are ordered longest first.
    let candidates: &[&str] = match s.as_bytes().first().copied()? {
        b'<' => &[
            "<==>", "<-->", "<==", "<=>", "<--", "<->", "<~~", "<<<", "<-<", "<<-", "<~", "<-",
            "<=", "<<",
        ],
        b'>' => &[">>>", ">->", ">=", ">>"],
        b'-' => &["-->", "->>", "->", "-"],
        b'|' => &["|->", "|=>", "|]", "||"],
        b'=' => &["==>", "=>", "=:"],
        b'~' => &["~~>", "~>", "~"],
        b':' => &["::=", ":="],
        b'!' => &["!="],
        b'.' => &["..."],
        b'[' => &["[|"],
        b'*' => return Some(1),
        _ => return None,
    };
    candidates
        .iter()
        .find(|t| s.starts_with(**t))
        .map(|t| t.len())
}
