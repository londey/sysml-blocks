//! Minimal, dependency-free PDF 1.4 writer — just enough for model export.
//!
//! Deliberately small scope:
//!   * Base-14 fonts only (Helvetica regular/bold/oblique + Courier) with
//!     WinAnsiEncoding and no embedding. Width tables are the public Adobe
//!     Core-14 AFM metrics so callers can measure text before placing it.
//!   * A4 portrait pages, uncompressed content streams (no flate).
//!   * Text runs, filled (optionally rounded) rectangles, stroked lines.
//!   * A nested document outline (bookmarks) and an Info dictionary.
//!
//! Coordinates are PDF user space: points, origin at the bottom-left.

/// RGB fill/stroke color, each channel in 0.0..=1.0.
pub type Rgb = (f32, f32, f32);

/// A4 portrait page width in points.
pub const PAGE_W: f32 = 595.28;
/// A4 portrait page height in points.
pub const PAGE_H: f32 = 841.89;

/// The four base fonts the writer knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    Helvetica,
    HelveticaBold,
    HelveticaOblique,
    Courier,
}

impl Font {
    /// Resource name used in content streams (`/F1` .. `/F4`).
    fn res(self) -> &'static str {
        match self {
            Font::Helvetica => "F1",
            Font::HelveticaBold => "F2",
            Font::HelveticaOblique => "F3",
            Font::Courier => "F4",
        }
    }
    /// `/BaseFont` name in the font dictionary.
    fn base_font(self) -> &'static str {
        match self {
            Font::Helvetica => "Helvetica",
            Font::HelveticaBold => "Helvetica-Bold",
            Font::HelveticaOblique => "Helvetica-Oblique",
            Font::Courier => "Courier",
        }
    }
}

struct Bookmark {
    title: String,
    page: usize,
    y: f32,
    level: usize,
}

/// An in-progress PDF document. Draw on pages, then call `finish()`.
pub struct Pdf {
    title: String,
    pages: Vec<Vec<u8>>, // raw content-stream operators per page
    outline: Vec<Bookmark>,
}

impl Pdf {
    pub fn new(title: &str) -> Pdf {
        Pdf { title: title.to_string(), pages: Vec::new(), outline: Vec::new() }
    }

    /// Append an empty A4 page; returns its index.
    pub fn add_page(&mut self) -> usize {
        self.pages.push(Vec::new());
        self.pages.len() - 1
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Draw one text run. `y` is the baseline.
    pub fn text(&mut self, page: usize, font: Font, size: f32, color: Rgb, x: f32, y: f32, s: &str) {
        if s.is_empty() {
            return;
        }
        let Some(buf) = self.pages.get_mut(page) else { return };
        let head = format!(
            "BT /{} {} Tf {} {} {} rg {} {} Td (",
            font.res(),
            fnum(size),
            fnum(color.0),
            fnum(color.1),
            fnum(color.2),
            fnum(x),
            fnum(y)
        );
        buf.extend_from_slice(head.as_bytes());
        buf.extend_from_slice(&escape_string(&winansi(s)));
        buf.extend_from_slice(b") Tj ET\n");
    }

    /// Axis-aligned filled rectangle; (x, y) is the bottom-left corner.
    pub fn rect(&mut self, page: usize, color: Rgb, x: f32, y: f32, w: f32, h: f32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let Some(buf) = self.pages.get_mut(page) else { return };
        let op = format!(
            "{} {} {} rg {} {} {} {} re f\n",
            fnum(color.0), fnum(color.1), fnum(color.2),
            fnum(x), fnum(y), fnum(w), fnum(h)
        );
        buf.extend_from_slice(op.as_bytes());
    }

    /// Filled rectangle with bezier-rounded corners of radius `r`.
    pub fn rounded_rect(&mut self, page: usize, color: Rgb, x: f32, y: f32, w: f32, h: f32, r: f32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let r = r.min(w / 2.0).min(h / 2.0);
        if r <= 0.1 {
            return self.rect(page, color, x, y, w, h);
        }
        let Some(buf) = self.pages.get_mut(page) else { return };
        let k = 0.552_285 * r; // circle-approximation control-point offset
        let (x2, y2) = (x + w, y + h);
        let op = format!(
            "{} {} {} rg {} {} m {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c \
             {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c f\n",
            fnum(color.0), fnum(color.1), fnum(color.2),
            // bottom edge, then each corner as a cubic bezier
            fnum(x + r), fnum(y),
            fnum(x2 - r), fnum(y),
            fnum(x2 - r + k), fnum(y), fnum(x2), fnum(y + r - k), fnum(x2), fnum(y + r),
            fnum(x2), fnum(y2 - r),
            fnum(x2), fnum(y2 - r + k), fnum(x2 - r + k), fnum(y2), fnum(x2 - r), fnum(y2),
            fnum(x + r), fnum(y2),
            fnum(x + r - k), fnum(y2), fnum(x), fnum(y2 - r + k), fnum(x), fnum(y2 - r),
            fnum(x), fnum(y + r),
            fnum(x), fnum(y + r - k), fnum(x + r - k), fnum(y), fnum(x + r), fnum(y)
        );
        buf.extend_from_slice(op.as_bytes());
    }

    /// Stroked straight line.
    pub fn line(&mut self, page: usize, color: Rgb, width: f32, x1: f32, y1: f32, x2: f32, y2: f32) {
        let Some(buf) = self.pages.get_mut(page) else { return };
        let op = format!(
            "{} {} {} RG {} w {} {} m {} {} l S\n",
            fnum(color.0), fnum(color.1), fnum(color.2),
            fnum(width),
            fnum(x1), fnum(y1), fnum(x2), fnum(y2)
        );
        buf.extend_from_slice(op.as_bytes());
    }

    /// Add a document-outline entry pointing at (`page`, `y`). `level` nests
    /// entries: an entry becomes a child of the nearest preceding entry with
    /// a smaller level, so equal levels are always siblings (even when
    /// intermediate levels were skipped).
    pub fn bookmark(&mut self, title: &str, page: usize, y: f32, level: usize) {
        self.outline.push(Bookmark { title: title.to_string(), page, y, level });
    }

    /// Measure a string in points at the given size (WinAnsi glyph widths).
    pub fn text_width(font: Font, size: f32, s: &str) -> f32 {
        let table = widths(font);
        let mut units: u64 = 0;
        for c in s.chars() {
            units += table[(winansi_byte(c) - 32) as usize] as u64;
        }
        units as f32 * size / 1000.0
    }

    /// Width of a single character (same metrics as `text_width`).
    pub fn char_width(font: Font, size: f32, c: char) -> f32 {
        widths(font)[(winansi_byte(c) - 32) as usize] as f32 * size / 1000.0
    }

    /// Serialize the document: header, objects, xref table, trailer.
    pub fn finish(mut self) -> Vec<u8> {
        if self.pages.is_empty() {
            self.pages.push(Vec::new());
        }
        let n = self.pages.len();
        let k = self.outline.len();
        // Object numbering: 1 catalog, 2 page tree, 3-6 fonts, 7 info,
        // then (page, contents) pairs, then the outline root + entries.
        let page_obj = |i: usize| 8 + 2 * i;
        let outline_root = 8 + 2 * n;
        let entry_obj = |j: usize| outline_root + 1 + j;
        let total = 7 + 2 * n + if k > 0 { 1 + k } else { 0 };

        let mut out: Vec<u8> = Vec::with_capacity(1024 + self.pages.iter().map(|p| p.len()).sum::<usize>());
        out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
        let mut offsets = vec![0usize; total + 1];

        // 1: catalog
        let catalog = if k > 0 {
            format!(
                "<< /Type /Catalog /Pages 2 0 R /Outlines {} 0 R /PageMode /UseOutlines >>",
                outline_root
            )
        } else {
            "<< /Type /Catalog /Pages 2 0 R >>".to_string()
        };
        push_obj(&mut out, &mut offsets, 1, catalog.as_bytes());

        // 2: page tree
        let kid_refs: Vec<String> = (0..n).map(|i| format!("{} 0 R", page_obj(i))).collect();
        push_obj(
            &mut out,
            &mut offsets,
            2,
            format!("<< /Type /Pages /Kids [{}] /Count {} >>", kid_refs.join(" "), n).as_bytes(),
        );

        // 3-6: fonts
        for (i, f) in [Font::Helvetica, Font::HelveticaBold, Font::HelveticaOblique, Font::Courier]
            .iter()
            .enumerate()
        {
            push_obj(
                &mut out,
                &mut offsets,
                3 + i,
                format!(
                    "<< /Type /Font /Subtype /Type1 /BaseFont /{} /Encoding /WinAnsiEncoding >>",
                    f.base_font()
                )
                .as_bytes(),
            );
        }

        // 7: info
        let mut info = Vec::new();
        info.extend_from_slice(b"<< /Title ");
        info.extend_from_slice(&text_string(&self.title));
        info.extend_from_slice(b" /Producer (sysml-blocks) >>");
        push_obj(&mut out, &mut offsets, 7, &info);

        // pages + content streams
        for (i, content) in self.pages.iter().enumerate() {
            let pid = page_obj(i);
            let page = format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] \
                 /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R /F4 6 0 R >> >> \
                 /Contents {} 0 R >>",
                fnum(PAGE_W),
                fnum(PAGE_H),
                pid + 1
            );
            push_obj(&mut out, &mut offsets, pid, page.as_bytes());
            let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
            stream.extend_from_slice(content);
            stream.extend_from_slice(b"\nendstream");
            push_obj(&mut out, &mut offsets, pid + 1, &stream);
        }

        // outline tree
        if k > 0 {
            // parent of each entry = nearest preceding entry with a smaller
            // declared level; comparing declared levels (not stack depth)
            // keeps same-level entries siblings across skipped levels
            let mut parent: Vec<Option<usize>> = vec![None; k];
            let mut stack: Vec<usize> = Vec::new(); // strictly increasing levels
            for j in 0..k {
                while stack
                    .last()
                    .is_some_and(|&t| self.outline[t].level >= self.outline[j].level)
                {
                    stack.pop();
                }
                parent[j] = stack.last().copied();
                stack.push(j);
            }
            let mut kids: Vec<Vec<usize>> = vec![Vec::new(); k];
            let mut roots: Vec<usize> = Vec::new();
            for j in 0..k {
                match parent[j] {
                    Some(p) => kids[p].push(j),
                    None => roots.push(j),
                }
            }
            // descendant counts (all entries open) — children come after
            // their parent, so a reverse pass sees children first
            let mut count = vec![0usize; k];
            for j in (0..k).rev() {
                count[j] = kids[j].len() + kids[j].iter().map(|&c| count[c]).sum::<usize>();
            }
            let mut prev: Vec<Option<usize>> = vec![None; k];
            let mut next: Vec<Option<usize>> = vec![None; k];
            for list in std::iter::once(&roots).chain(kids.iter()) {
                for w in list.windows(2) {
                    next[w[0]] = Some(w[1]);
                    prev[w[1]] = Some(w[0]);
                }
            }

            push_obj(
                &mut out,
                &mut offsets,
                outline_root,
                format!(
                    "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {} >>",
                    entry_obj(roots[0]),
                    entry_obj(*roots.last().unwrap()),
                    k
                )
                .as_bytes(),
            );
            for j in 0..k {
                let b = &self.outline[j];
                let mut d = Vec::new();
                d.extend_from_slice(b"<< /Title ");
                d.extend_from_slice(&text_string(&b.title));
                d.extend_from_slice(
                    format!(
                        " /Parent {} 0 R /Dest [{} 0 R /XYZ null {} null]",
                        parent[j].map(entry_obj).unwrap_or(outline_root),
                        page_obj(b.page.min(n - 1)),
                        fnum(b.y)
                    )
                    .as_bytes(),
                );
                if let Some(p) = prev[j] {
                    d.extend_from_slice(format!(" /Prev {} 0 R", entry_obj(p)).as_bytes());
                }
                if let Some(x) = next[j] {
                    d.extend_from_slice(format!(" /Next {} 0 R", entry_obj(x)).as_bytes());
                }
                if !kids[j].is_empty() {
                    d.extend_from_slice(
                        format!(
                            " /First {} 0 R /Last {} 0 R /Count {}",
                            entry_obj(kids[j][0]),
                            entry_obj(*kids[j].last().unwrap()),
                            count[j]
                        )
                        .as_bytes(),
                    );
                }
                d.extend_from_slice(b" >>");
                push_obj(&mut out, &mut offsets, entry_obj(j), &d);
            }
        }

        // xref + trailer — offsets must be exact byte positions
        let xref_pos = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", total + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for id in 1..=total {
            out.extend_from_slice(format!("{:010} 00000 n \n", offsets[id]).as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R /Info 7 0 R >>\nstartxref\n{}\n%%EOF\n",
                total + 1,
                xref_pos
            )
            .as_bytes(),
        );
        out
    }
}

/// Write one indirect object body and record its byte offset.
fn push_obj(out: &mut Vec<u8>, offsets: &mut [usize], id: usize, body: &[u8]) {
    offsets[id] = out.len();
    out.extend_from_slice(format!("{} 0 obj\n", id).as_bytes());
    out.extend_from_slice(body);
    out.extend_from_slice(b"\nendobj\n");
}

/// Compact number formatting for content streams ("12", "0.5", not "12.00").
fn fnum(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{:.2}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" || s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// Map one Unicode scalar to its WinAnsi (CP1252) byte; `?` if unencodable.
fn winansi_byte(c: char) -> u8 {
    let u = c as u32;
    match u {
        0x20..=0x7E => u as u8,
        0xA0..=0xFF => u as u8,
        _ => match c {
            '€' => 0x80,
            '‚' => 0x82,
            'ƒ' => 0x83,
            '„' => 0x84,
            '…' => 0x85,
            '†' => 0x86,
            '‡' => 0x87,
            'ˆ' => 0x88,
            '‰' => 0x89,
            'Š' => 0x8A,
            '‹' => 0x8B,
            'Œ' => 0x8C,
            'Ž' => 0x8E,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            '˜' => 0x98,
            '™' => 0x99,
            'š' => 0x9A,
            '›' => 0x9B,
            'œ' => 0x9C,
            'ž' => 0x9E,
            'Ÿ' => 0x9F,
            '\t' | '\n' | '\r' => b' ',
            _ => b'?',
        },
    }
}

fn winansi(s: &str) -> Vec<u8> {
    s.chars().map(winansi_byte).collect()
}

/// Escape a byte string for inclusion inside PDF `( ... )` literals.
fn escape_string(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 4);
    for &b in bytes {
        match b {
            b'\\' | b'(' | b')' => {
                out.push(b'\\');
                out.push(b);
            }
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            _ => out.push(b),
        }
    }
    out
}

/// Encode a string for a PDF *text string* context (Info, outline titles):
/// pure ASCII stays literal; anything else becomes UTF-16BE with a BOM.
/// Returns the full `( ... )` literal including parentheses.
fn text_string(s: &str) -> Vec<u8> {
    let ascii = s.chars().all(|c| (' '..='~').contains(&c));
    let mut out = vec![b'('];
    if ascii {
        out.extend_from_slice(&escape_string(s.as_bytes()));
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.push((u >> 8) as u8);
            b.push((u & 0xFF) as u8);
        }
        out.extend_from_slice(&escape_string(&b));
    }
    out.push(b')');
    out
}

fn widths(font: Font) -> &'static [u16; 224] {
    match font {
        Font::Helvetica | Font::HelveticaOblique => &HELVETICA_WIDTHS,
        Font::HelveticaBold => &HELVETICA_BOLD_WIDTHS,
        Font::Courier => &COURIER_WIDTHS,
    }
}

const COURIER_WIDTHS: [u16; 224] = [600; 224];

/// Adobe Core-14 AFM widths for Helvetica (and Helvetica-Oblique),
/// chars 32..=255 in WinAnsi order, in 1/1000 em units.
const HELVETICA_WIDTHS: [u16; 224] = [
    278, 278, 355, 556, 556, 889, 667, 191, // 32..39   !"#$%&'
    333, 333, 389, 584, 278, 333, 278, 278, // 40..47  ()*+,-./
    556, 556, 556, 556, 556, 556, 556, 556, // 48..55  01234567
    556, 556, 278, 278, 584, 584, 584, 556, // 56..63  89:;<=>?
    1015, 667, 667, 722, 722, 667, 611, 778, // 64..71 @ABCDEFG
    722, 278, 500, 667, 556, 833, 722, 778, // 72..79  HIJKLMNO
    667, 778, 722, 667, 611, 722, 667, 944, // 80..87  PQRSTUVW
    667, 667, 611, 278, 278, 278, 469, 556, // 88..95  XYZ[\]^_
    333, 556, 556, 500, 556, 556, 278, 556, // 96..103 `abcdefg
    556, 222, 222, 500, 222, 833, 556, 556, // 104..111 hijklmno
    556, 556, 333, 500, 278, 556, 500, 722, // 112..119 pqrstuvw
    500, 500, 500, 334, 260, 334, 584, 350, // 120..127 xyz{|}~
    556, 350, 222, 556, 333, 1000, 556, 556, // 128..135 €.‚ƒ„…†‡
    333, 1000, 667, 333, 1000, 350, 611, 350, // 136..143 ˆ‰Š‹Œ.Ž.
    350, 222, 222, 333, 333, 350, 556, 1000, // 144..151 .‘’“”•–—
    333, 1000, 500, 333, 944, 350, 500, 667, // 152..159 ˜™š›œ.žŸ
    278, 333, 556, 556, 556, 556, 260, 556, // 160..167  ¡¢£¤¥¦§
    333, 737, 370, 556, 584, 333, 737, 333, // 168..175 ¨©ª«¬­®¯
    400, 584, 333, 333, 333, 556, 537, 278, // 176..183 °±²³´µ¶·
    333, 333, 365, 556, 834, 834, 834, 611, // 184..191 ¸¹º»¼½¾¿
    667, 667, 667, 667, 667, 667, 1000, 722, // 192..199 ÀÁÂÃÄÅÆÇ
    667, 667, 667, 667, 278, 278, 278, 278, // 200..207 ÈÉÊËÌÍÎÏ
    722, 722, 778, 778, 778, 778, 778, 584, // 208..215 ÐÑÒÓÔÕÖ×
    778, 722, 722, 722, 722, 667, 667, 611, // 216..223 ØÙÚÛÜÝÞß
    556, 556, 556, 556, 556, 556, 889, 500, // 224..231 àáâãäåæç
    556, 556, 556, 556, 278, 278, 278, 278, // 232..239 èéêëìíîï
    556, 556, 556, 556, 556, 556, 556, 584, // 240..247 ðñòóôõö÷
    611, 556, 556, 556, 556, 500, 556, 500, // 248..255 øùúûüýþÿ
];

/// Adobe Core-14 AFM widths for Helvetica-Bold, chars 32..=255, WinAnsi order.
const HELVETICA_BOLD_WIDTHS: [u16; 224] = [
    278, 333, 474, 556, 556, 889, 722, 238, // 32..39
    333, 333, 389, 584, 278, 333, 278, 278, // 40..47
    556, 556, 556, 556, 556, 556, 556, 556, // 48..55
    556, 556, 333, 333, 584, 584, 584, 611, // 56..63
    975, 722, 722, 722, 722, 667, 611, 778, // 64..71
    722, 278, 556, 722, 611, 833, 722, 778, // 72..79
    667, 778, 722, 667, 611, 722, 667, 944, // 80..87
    667, 667, 611, 333, 278, 333, 584, 556, // 88..95
    333, 556, 611, 556, 611, 556, 333, 611, // 96..103
    611, 278, 278, 556, 278, 889, 611, 611, // 104..111
    611, 611, 389, 556, 333, 611, 556, 778, // 112..119
    556, 556, 500, 389, 280, 389, 584, 350, // 120..127
    556, 350, 278, 556, 500, 1000, 556, 556, // 128..135
    333, 1000, 667, 333, 1000, 350, 611, 350, // 136..143
    350, 278, 278, 500, 500, 350, 556, 1000, // 144..151
    333, 1000, 556, 333, 944, 350, 500, 667, // 152..159
    278, 333, 556, 556, 556, 556, 280, 556, // 160..167
    333, 737, 370, 556, 584, 333, 737, 333, // 168..175
    400, 584, 333, 333, 333, 611, 556, 278, // 176..183
    333, 333, 365, 556, 834, 834, 834, 611, // 184..191
    722, 722, 722, 722, 722, 722, 1000, 722, // 192..199
    667, 667, 667, 667, 278, 278, 278, 278, // 200..207
    722, 722, 778, 778, 778, 778, 778, 584, // 208..215
    778, 722, 722, 722, 722, 667, 667, 611, // 216..223
    556, 556, 556, 556, 556, 556, 889, 556, // 224..231
    556, 556, 556, 556, 278, 278, 278, 278, // 232..239
    611, 611, 611, 611, 611, 611, 611, 584, // 240..247
    611, 611, 611, 611, 611, 556, 611, 556, // 248..255
];

#[cfg(test)]
mod tests {
    use super::*;

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }
    fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).rposition(|w| w == needle)
    }

    #[test]
    fn escapes_parens_and_backslash() {
        assert_eq!(escape_string(b"(a)\\"), b"\\(a\\)\\\\".to_vec());
        assert_eq!(escape_string(b"plain"), b"plain".to_vec());
    }

    #[test]
    fn winansi_maps_punctuation_and_falls_back() {
        assert_eq!(winansi("a\u{2013}b"), vec![b'a', 0x96, b'b']); // en dash
        assert_eq!(winansi("\u{201C}x\u{201D}"), vec![0x93, b'x', 0x94]); // curly quotes
        assert_eq!(winansi("\u{2022}"), vec![0x95]); // bullet
        assert_eq!(winansi("\u{2603}"), vec![b'?']); // snowman: not in CP1252
        assert_eq!(winansi("é"), vec![0xE9]); // Latin-1 range passes through
    }

    #[test]
    fn width_measurement_sane() {
        // Courier is fixed-pitch 600/1000 em
        let w = Pdf::text_width(Font::Courier, 10.0, "abc");
        assert!((w - 18.0).abs() < 1e-4, "courier abc = {}", w);
        // proportional: W wider than i, empty is zero
        assert!(Pdf::text_width(Font::Helvetica, 12.0, "W") > Pdf::text_width(Font::Helvetica, 12.0, "i"));
        assert_eq!(Pdf::text_width(Font::HelveticaBold, 12.0, ""), 0.0);
        // char_width consistent with text_width
        let cw = Pdf::char_width(Font::Helvetica, 10.0, 'M');
        assert!((cw - Pdf::text_width(Font::Helvetica, 10.0, "M")).abs() < 1e-5);
    }

    #[test]
    fn structural_output_valid() {
        let mut pdf = Pdf::new("Test Doc");
        let p0 = pdf.add_page();
        pdf.text(p0, Font::Helvetica, 10.0, (0.0, 0.0, 0.0), 72.0, 700.0, "Hello (pdf) \\ test");
        pdf.rect(p0, (0.5, 0.5, 0.5), 100.0, 100.0, 50.0, 20.0);
        pdf.rounded_rect(p0, (0.1, 0.2, 0.3), 100.0, 200.0, 80.0, 16.0, 4.0);
        pdf.line(p0, (0.0, 0.0, 0.0), 0.5, 72.0, 650.0, 300.0, 650.0);
        pdf.bookmark("Section 1", p0, 700.0, 0);
        let p1 = pdf.add_page();
        pdf.text(p1, Font::Courier, 9.0, (0.0, 0.0, 0.0), 72.0, 700.0, "page two");
        pdf.bookmark("Section 1.1", p1, 700.0, 1);
        pdf.bookmark("Section 2", p1, 600.0, 0);
        let bytes = pdf.finish();

        assert!(bytes.starts_with(b"%PDF-"));
        let tail: Vec<u8> = bytes.iter().rev().take(16).rev().copied().collect();
        assert!(find(&tail, b"%%EOF").is_some(), "must end with %%EOF");

        // xref offsets must point at "N 0 obj"
        let sx = rfind(&bytes, b"startxref").expect("startxref");
        let after = &bytes[sx + b"startxref".len()..];
        let digits: String = after
            .iter()
            .map(|&b| b as char)
            .skip_while(|c| c.is_whitespace())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let xref_off: usize = digits.parse().expect("xref offset");
        assert_eq!(&bytes[xref_off..xref_off + 4], b"xref");

        // parse "0 N" then N 20-byte entries
        let hdr_start = xref_off + 5; // past "xref\n"
        let line_end = hdr_start + bytes[hdr_start..].iter().position(|&b| b == b'\n').unwrap();
        let hdr = String::from_utf8_lossy(&bytes[hdr_start..line_end]).to_string();
        let n: usize = hdr.split_whitespace().nth(1).unwrap().parse().unwrap();
        let entries = line_end + 1;
        for i in 1..n {
            let e = &bytes[entries + 20 * i..entries + 20 * i + 20];
            let off: usize = String::from_utf8_lossy(&e[..10]).parse().unwrap();
            let expect = format!("{} 0 obj", i);
            assert_eq!(
                &bytes[off..off + expect.len()],
                expect.as_bytes(),
                "xref entry {} should point at its object",
                i
            );
        }
        // content is uncompressed: the text is visible
        assert!(find(&bytes, b"Hello").is_some());
        assert!(find(&bytes, b"/Outlines").is_some());
    }
}
