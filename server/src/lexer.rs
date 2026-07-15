//! Minimal span-preserving lexer for the SysML v2 textual notation.
//!
//! Every token records its byte span in the original source so the editor
//! can perform surgical text splices instead of re-printing whole files.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokKind {
    Ident,        // plain identifier or keyword (keywords resolved by parser)
    QuotedName,   // 'unrestricted name'
    Number,       // 42, 3.14
    Str,          // "string literal"
    BlockComment, // /* ... */  (kept: needed for `doc` bodies)
    Punct,        // single/multi char punctuation: { } ; : :> :>> = [ ] , . :: ~ etc.
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokKind,
    pub start: usize,
    pub end: usize,
}

impl Token {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
}

pub fn lex(src: &str) -> Vec<Token> {
    let bytes = src.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0usize;
    let n = bytes.len();

    while i < n {
        let c = bytes[i];
        // whitespace
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // line comment
        if c == b'/' && i + 1 < n && bytes[i + 1] == b'/' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // block comment (token — may be a doc body)
        if c == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            let mut depth = 1;
            while i < n && depth > 0 {
                if bytes[i] == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && i + 1 < n && bytes[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            toks.push(Token { kind: TokKind::BlockComment, start, end: i });
            continue;
        }
        // quoted name
        if c == b'\'' {
            let start = i;
            i += 1;
            while i < n && bytes[i] != b'\'' {
                i += 1;
            }
            i = (i + 1).min(n);
            toks.push(Token { kind: TokKind::QuotedName, start, end: i });
            continue;
        }
        // string
        if c == b'"' {
            let start = i;
            i += 1;
            while i < n && bytes[i] != b'"' {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(n);
            toks.push(Token { kind: TokKind::Str, start, end: i });
            continue;
        }
        // number
        if c.is_ascii_digit() {
            let start = i;
            while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_')
            {
                // stop "1..2" range dots from being eaten as one number
                if bytes[i] == b'.' && i + 1 < n && bytes[i + 1] == b'.' {
                    break;
                }
                i += 1;
            }
            toks.push(Token { kind: TokKind::Number, start, end: i });
            continue;
        }
        // identifier
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            toks.push(Token { kind: TokKind::Ident, start, end: i });
            continue;
        }
        // multi-char punctuation, longest first
        let rest = &src[i..];
        let mut matched = 0usize;
        for p in [":>>", "::>", ":>", "::", "..", "=>", ">=", "<=", "==", "!=", "->"] {
            if rest.starts_with(p) {
                matched = p.len();
                break;
            }
        }
        if matched == 0 {
            matched = 1;
        }
        toks.push(Token { kind: TokKind::Punct, start: i, end: i + matched });
        i += matched;
    }

    toks.push(Token { kind: TokKind::Eof, start: n, end: n });
    toks
}
