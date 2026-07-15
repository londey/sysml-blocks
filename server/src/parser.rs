//! Tolerant parser for a practical subset of SysML v2 textual notation.
//!
//! Design goals, in order:
//!   1. Never lose text. Anything not understood becomes a `raw` element that
//!      round-trips verbatim.
//!   2. Record byte spans for the whole element, its name, its value and its
//!      body so the edit engine can splice text without reformatting files.
//!   3. Understand enough structure (packages, defs, usages, attributes,
//!      ports, connections, requirements, ...) to drive a block UI.

use crate::lexer::{lex, TokKind, Token};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Element {
    pub id: String,
    /// e.g. "package", "part def", "part", "attribute", "port def", "port",
    /// "connection", "connect", "requirement def", "requirement", "item",
    /// "action", "constraint", "interface def", "import", "doc", "comment",
    /// "state", "enum def", "raw", ...
    pub kind: String,
    /// prefix modifiers seen before the kind keywords: abstract, variation,
    /// ref, in, out, inout, readonly, derived, private, protected, public...
    pub modifiers: Vec<String>,
    pub name: Option<String>,
    pub short_name: Option<String>,
    /// `: Type1, Type2`
    pub typed_by: Vec<String>,
    /// `:> Super1, Super2` (specializes / subsets)
    pub specializes: Vec<String>,
    /// `:>> Redefined`
    pub redefines: Vec<String>,
    /// `[0..*]` — raw text between the brackets
    pub multiplicity: Option<String>,
    /// `= <expr>` — raw expression text
    pub value: Option<String>,
    /// For `connect a.x to b.y`
    pub connect_ends: Vec<String>,
    /// doc/comment body text (comment markers stripped)
    pub text: Option<String>,
    /// Verbatim source for `raw` elements
    pub raw: Option<String>,
    pub children: Vec<Element>,
    /// true when the element has a `{ ... }` body (even if empty)
    pub has_body: bool,

    // ---- spans (byte offsets into the file) ----
    pub span: Span,
    pub name_span: Option<Span>,
    pub value_span: Option<Span>,
    /// span strictly inside the braces of the body
    pub body_span: Option<Span>,
}

impl Element {
    fn new(kind: &str, start: usize) -> Self {
        Element {
            id: String::new(),
            kind: kind.to_string(),
            modifiers: vec![],
            name: None,
            short_name: None,
            typed_by: vec![],
            specializes: vec![],
            redefines: vec![],
            multiplicity: None,
            value: None,
            connect_ends: vec![],
            text: None,
            raw: None,
            children: vec![],
            has_body: false,
            span: Span { start, end: start },
            name_span: None,
            value_span: None,
            body_span: None,
        }
    }
}

/// Keywords that begin a member we understand. `def` may follow many of them.
const KIND_KEYWORDS: &[&str] = &[
    "package", "part", "attribute", "port", "item", "action", "state",
    "requirement", "constraint", "connection", "interface", "allocation",
    "analysis", "calc", "case", "concern", "enum", "flow", "metadata",
    "occurrence", "rendering", "verification", "view", "viewpoint", "use",
    "individual", "snapshot", "timeslice", "transition", "exhibit",
    "perform", "satisfy", "verify", "assert", "assume", "require",
    "subject", "actor", "stakeholder", "objective", "return", "bind",
];

const MODIFIER_KEYWORDS: &[&str] = &[
    "abstract", "variation", "variant", "ref", "in", "out", "inout",
    "readonly", "derived", "end", "private", "protected", "public",
    "redefines", "nonunique", "ordered", "default", "constant",
];

pub struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str) -> Self {
        Parser { src, toks: lex(src), pos: 0 }
    }

    fn peek(&self) -> &Token {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }
    fn peek2(&self) -> &Token {
        &self.toks[(self.pos + 1).min(self.toks.len() - 1)]
    }
    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos.min(self.toks.len() - 1)].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn at_punct(&self, p: &str) -> bool {
        let t = self.peek();
        t.kind == TokKind::Punct && t.text(self.src) == p
    }
    fn eat_punct(&mut self, p: &str) -> bool {
        if self.at_punct(p) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn at_ident(&self, w: &str) -> bool {
        let t = self.peek();
        t.kind == TokKind::Ident && t.text(self.src) == w
    }
    fn at_eof(&self) -> bool {
        self.peek().kind == TokKind::Eof
    }

    /// Parse a whole file: a sequence of members.
    pub fn parse_file(&mut self) -> Vec<Element> {
        let mut out = Vec::new();
        while !self.at_eof() {
            if self.at_punct("}") || self.at_punct(";") {
                // stray closer: swallow to keep going
                self.bump();
                continue;
            }
            out.push(self.parse_member());
        }
        out
    }

    /// qualified name: A::B::C or A.B, optionally conjugated (~T)
    fn parse_qualified_name(&mut self) -> Option<String> {
        let mut prefix_start: Option<usize> = None;
        if self.at_punct("~")
            && matches!(self.peek2().kind, TokKind::Ident | TokKind::QuotedName)
        {
            prefix_start = Some(self.peek().start);
            self.bump();
        }
        let t = self.peek().clone();
        if t.kind != TokKind::Ident && t.kind != TokKind::QuotedName {
            return None;
        }
        let start = prefix_start.unwrap_or(t.start);
        let mut end = t.end;
        self.bump();
        loop {
            if self.at_punct("::") || self.at_punct(".") {
                let nt = self.peek2().clone();
                if nt.kind == TokKind::Ident || nt.kind == TokKind::QuotedName {
                    self.bump(); // separator
                    let nt = self.bump();
                    end = nt.end;
                    continue;
                }
            }
            break;
        }
        Some(self.src[start..end].to_string())
    }

    fn parse_name_list(&mut self) -> Vec<String> {
        let mut v = Vec::new();
        loop {
            match self.parse_qualified_name() {
                Some(n) => v.push(n),
                None => break,
            }
            if !self.eat_punct(",") {
                break;
            }
        }
        v
    }

    /// Consume a balanced blob until `;` at depth 0 or a `{` at depth 0
    /// (which is left for the caller). Returns end byte of the blob.
    fn skip_expr(&mut self) -> (usize, usize) {
        let start = self.peek().start;
        let mut end = start;
        let mut depth = 0i32;
        loop {
            let t = self.peek().clone();
            match t.kind {
                TokKind::Eof => break,
                TokKind::Punct => {
                    let s = t.text(self.src);
                    match s {
                        "(" | "[" => depth += 1,
                        ")" | "]" => depth -= 1,
                        ";" if depth <= 0 => break,
                        "{" if depth <= 0 => break,
                        "}" if depth <= 0 => break,
                        _ => {}
                    }
                }
                _ => {}
            }
            end = t.end;
            self.bump();
        }
        (start, end)
    }

    /// Fallback: consume one whole unrecognized statement (to `;` or a
    /// balanced `{...}`) and preserve it verbatim.
    fn parse_raw(&mut self, start: usize) -> Element {
        let mut el = Element::new("raw", start);
        let mut end = start;
        loop {
            let t = self.peek().clone();
            match t.kind {
                TokKind::Eof => break,
                TokKind::Punct => {
                    let s = t.text(self.src).to_string();
                    if s == ";" {
                        end = t.end;
                        self.bump();
                        break;
                    }
                    if s == "}" {
                        // don't eat parent's closer
                        break;
                    }
                    if s == "{" {
                        // consume balanced body then stop
                        let mut depth = 0i32;
                        loop {
                            let t2 = self.bump();
                            if t2.kind == TokKind::Eof {
                                break;
                            }
                            let s2 = t2.text(self.src);
                            if t2.kind == TokKind::Punct && s2 == "{" {
                                depth += 1;
                            }
                            if t2.kind == TokKind::Punct && s2 == "}" {
                                depth -= 1;
                                if depth == 0 {
                                    end = t2.end;
                                    break;
                                }
                            }
                        }
                        break;
                    }
                    end = t.end;
                    self.bump();
                }
                _ => {
                    end = t.end;
                    self.bump();
                }
            }
        }
        el.span = Span { start, end };
        el.raw = Some(self.src[start..end].to_string());
        el
    }

    fn parse_member(&mut self) -> Element {
        let start_tok = self.peek().clone();
        let start = start_tok.start;

        // standalone doc / comment
        if self.at_ident("doc") || self.at_ident("comment") {
            let kw = self.bump().text(self.src).to_string();
            let mut el = Element::new(&kw, start);
            // optional name for comment: `comment Name /* .. */` (rare) — skip idents
            while self.peek().kind == TokKind::Ident {
                self.bump();
            }
            if self.peek().kind == TokKind::BlockComment {
                let t = self.bump();
                let body = t.text(self.src);
                let trimmed = body
                    .trim_start_matches("/*")
                    .trim_end_matches("*/")
                    .trim()
                    .to_string();
                el.text = Some(trimmed);
                el.span = Span { start, end: t.end };
            }
            // optional trailing ;
            if self.at_punct(";") {
                let t = self.bump();
                el.span.end = t.end;
            }
            el.raw = Some(self.src[el.span.start..el.span.end].to_string());
            return el;
        }

        // free-floating block comment between members: keep it
        if start_tok.kind == TokKind::BlockComment {
            let t = self.bump();
            let mut el = Element::new("comment", start);
            el.text = Some(
                t.text(self.src)
                    .trim_start_matches("/*")
                    .trim_end_matches("*/")
                    .trim()
                    .to_string(),
            );
            el.span = Span { start, end: t.end };
            el.raw = Some(self.src[start..t.end].to_string());
            return el;
        }

        // import
        if self.at_ident("import") {
            self.bump();
            let mut el = Element::new("import", start);
            let (_, mut end) = self.skip_expr();
            if self.at_punct(";") {
                end = self.bump().end;
            }
            el.span = Span { start, end };
            el.name = Some(
                self.src[start..end]
                    .trim_start_matches("import")
                    .trim()
                    .trim_end_matches(';')
                    .trim()
                    .to_string(),
            );
            el.raw = Some(self.src[start..end].to_string());
            return el;
        }

        // collect modifiers
        let mut modifiers: Vec<String> = Vec::new();
        loop {
            let t = self.peek().clone();
            if t.kind == TokKind::Ident {
                let w = t.text(self.src);
                if MODIFIER_KEYWORDS.contains(&w)
                    && !(w == "default" && self.peek2().kind != TokKind::Ident)
                {
                    modifiers.push(w.to_string());
                    self.bump();
                    continue;
                }
            }
            break;
        }

        // kind keyword(s)
        let t = self.peek().clone();
        let kw = if t.kind == TokKind::Ident {
            t.text(self.src).to_string()
        } else {
            String::new()
        };

        if kw == "connect" {
            return self.parse_connect(start, modifiers);
        }

        let bare_usage = !KIND_KEYWORDS.contains(&kw.as_str())
            && !modifiers.is_empty()
            && matches!(self.peek().kind, TokKind::Ident | TokKind::QuotedName);
        if !KIND_KEYWORDS.contains(&kw.as_str()) && !bare_usage {
            // not something we understand — preserve verbatim
            return self.parse_raw(start);
        }

        let mut kind;
        if bare_usage {
            // e.g. `end supplier : PowerPort;` or `ref x : T;`
            kind = modifiers.join(" ");
        } else {
            self.bump();
            kind = kw.clone();
        }
        // `part def`, `port def`, `enum def`, `use case`, `flow` ...
        if !bare_usage && self.at_ident("def") {
            self.bump();
            kind = format!("{} def", kw);
        } else if kw == "use" && self.at_ident("case") {
            self.bump();
            kind = "use case".into();
            if self.at_ident("def") {
                self.bump();
                kind = "use case def".into();
            }
        }

        let mut el = Element::new(&kind, start);
        el.modifiers = if bare_usage { vec![] } else { modifiers };

        // short name  <abbrev>
        if self.at_punct("<") {
            self.bump();
            if self.peek().kind == TokKind::Ident || self.peek().kind == TokKind::QuotedName {
                el.short_name = Some(self.bump().text(self.src).to_string());
            }
            self.eat_punct(">");
        }

        // name
        {
            let t = self.peek().clone();
            if t.kind == TokKind::Ident || t.kind == TokKind::QuotedName {
                // guard: `part def;` has no name; also stop before relation puncts
                let w = t.text(self.src);
                if !KIND_KEYWORDS.contains(&w) || self.peek2().kind != TokKind::Ident {
                    el.name = Some(w.to_string());
                    el.name_span = Some(Span { start: t.start, end: t.end });
                    self.bump();
                }
            }
        }

        // relationships / multiplicity / value, in any order
        loop {
            if self.at_punct(":>>") {
                self.bump();
                el.redefines.extend(self.parse_name_list());
            } else if self.at_punct(":>") {
                self.bump();
                el.specializes.extend(self.parse_name_list());
            } else if self.at_punct(":") {
                self.bump();
                el.typed_by.extend(self.parse_name_list());
            } else if self.at_ident("specializes") || self.at_ident("subsets") {
                self.bump();
                el.specializes.extend(self.parse_name_list());
            } else if self.at_ident("redefines") {
                self.bump();
                el.redefines.extend(self.parse_name_list());
            } else if self.at_ident("defined") {
                // `defined by X`
                self.bump();
                if self.at_ident("by") {
                    self.bump();
                }
                el.typed_by.extend(self.parse_name_list());
            } else if self.at_punct("[") {
                let open = self.bump();
                let mut depth = 1;
                let mstart = open.end;
                let mut mend = open.end;
                while depth > 0 && !self.at_eof() {
                    let t = self.bump();
                    let s = t.text(self.src);
                    if t.kind == TokKind::Punct && s == "[" {
                        depth += 1;
                    }
                    if t.kind == TokKind::Punct && s == "]" {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    mend = t.end;
                }
                el.multiplicity = Some(self.src[mstart..mend].trim().to_string());
            } else if self.at_punct("=") || self.at_punct(":=") {
                self.bump();
                let (vs, ve) = self.skip_expr();
                el.value = Some(self.src[vs..ve].trim().to_string());
                el.value_span = Some(Span { start: vs, end: ve });
            } else {
                break;
            }
        }

        // terminator: `;` or `{ body }` — otherwise consume leftovers as raw tail
        if self.at_punct(";") {
            let t = self.bump();
            el.span = Span { start, end: t.end };
        } else if self.at_punct("{") {
            let open = self.bump();
            el.has_body = true;
            let body_start = open.end;
            let mut children = Vec::new();
            loop {
                if self.at_eof() {
                    let end = self.peek().end;
                    el.body_span = Some(Span { start: body_start, end });
                    el.span = Span { start, end };
                    break;
                }
                if self.at_punct("}") {
                    let close = self.bump();
                    el.body_span = Some(Span { start: body_start, end: close.start });
                    el.span = Span { start, end: close.end };
                    // optional trailing ; after }
                    if self.at_punct(";") {
                        let t = self.bump();
                        el.span.end = t.end;
                    }
                    break;
                }
                if self.at_punct(";") {
                    self.bump();
                    continue;
                }
                children.push(self.parse_member());
            }
            el.children = children;
        } else if self.at_eof() {
            el.span = Span { start, end: self.peek().end };
        } else {
            // something unexpected (e.g. an expression form we don't model):
            // absorb to the statement end so nothing is lost
            let raw_tail = self.parse_raw(self.peek().start);
            el.span = Span { start, end: raw_tail.span.end };
        }
        el
    }

    fn parse_connect(&mut self, start: usize, modifiers: Vec<String>) -> Element {
        self.bump(); // connect
        let mut el = Element::new("connect", start);
        el.modifiers = modifiers;
        if let Some(a) = self.parse_qualified_name() {
            el.connect_ends.push(a);
        }
        if self.at_ident("to") {
            self.bump();
            if let Some(b) = self.parse_qualified_name() {
                el.connect_ends.push(b);
            }
        }
        let mut end = self.peek().start;
        if self.at_punct(";") {
            end = self.bump().end;
        } else if self.at_punct("{") {
            // connect with body: treat body as raw for now
            let raw = self.parse_raw(self.peek().start);
            end = raw.span.end;
        }
        el.span = Span { start, end };
        el
    }
}

/// Assign stable-within-snapshot ids: file index + path through the tree.
pub fn assign_ids(file_idx: usize, elems: &mut [Element]) {
    fn rec(prefix: &str, elems: &mut [Element]) {
        for (i, e) in elems.iter_mut().enumerate() {
            e.id = format!("{}.{}", prefix, i);
            rec(&e.id.clone(), &mut e.children);
        }
    }
    rec(&format!("f{}", file_idx), elems);
}
