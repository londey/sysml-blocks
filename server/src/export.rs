//! PDF export: renders a workspace slice as either a structured
//! specification document ("doc") or a visual block rendering that echoes
//! the web UI ("blocks"), on top of the minimal writer in pdf.rs.

use crate::model::{FileModel, Workspace};
use crate::parser::Element;
use crate::pdf::{Font, Pdf, Rgb, PAGE_H, PAGE_W};

/// What part of the workspace to export.
pub enum ExportScope {
    /// One element (with all descendants) by id, e.g. "f0.2".
    Element(String),
    /// One file by workspace-relative path.
    File(String),
    /// Every file in the workspace.
    Project,
}

/// Output flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Doc,
    Blocks,
}

const MARGIN: f32 = 54.0;
const TOP_Y: f32 = PAGE_H - MARGIN;
const CONTENT_W: f32 = PAGE_W - 2.0 * MARGIN;
const MAX_DEPTH: usize = 64;
const RAIL_W: f32 = 3.0;

const INK: Rgb = (0.13, 0.15, 0.18);
const GREY: Rgb = (0.45, 0.48, 0.52);
const LIGHT: Rgb = (0.78, 0.80, 0.84);
const CODE_INK: Rgb = (0.25, 0.28, 0.32);
const CODE_BG: Rgb = (0.94, 0.95, 0.96);
const WHITE: Rgb = (1.0, 1.0, 1.0);

/// Build a PDF for the given scope. Returns (bytes, suggested filename).
/// Errors only for an unknown element id / file path.
pub fn export_pdf(
    ws: &Workspace,
    scope: &ExportScope,
    format: ExportFormat,
) -> Result<(Vec<u8>, String), String> {
    enum Target<'a> {
        El(&'a Element),
        File(usize),
        Project,
    }
    let target = match scope {
        ExportScope::Element(id) => {
            let (_, el) = Workspace::find(&ws.files, id)
                .ok_or_else(|| format!("element '{}' not found", id))?;
            Target::El(el)
        }
        ExportScope::File(path) => {
            let i = ws
                .files
                .iter()
                .position(|f| f.path == *path)
                .ok_or_else(|| format!("file '{}' not found", path))?;
            Target::File(i)
        }
        ExportScope::Project => Target::Project,
    };
    let (title, base) = match &target {
        Target::El(el) => (display_name(el), display_name(el)),
        Target::File(i) => (ws.files[*i].path.clone(), file_stem(&ws.files[*i].path)),
        Target::Project => ("SysML project".to_string(), "sysml-project".to_string()),
    };
    let filename = format!("{}.pdf", sanitize_filename(&base));

    let mut l = Layout::new(&title);
    title_block(&mut l, &title);
    match (&target, format) {
        (Target::El(el), ExportFormat::Doc) => {
            let mut nums = vec![1];
            doc_section(&mut l, el, &mut nums, 1);
        }
        (Target::El(el), ExportFormat::Blocks) => blocks_element(&mut l, el, MARGIN, 0),
        (Target::File(i), ExportFormat::Doc) => {
            let mut nums = Vec::new();
            doc_children(&mut l, &ws.files[*i].elements, &mut nums, 1);
        }
        (Target::File(i), ExportFormat::Blocks) => {
            blocks_children(&mut l, &ws.files[*i].elements, MARGIN, 0)
        }
        (Target::Project, ExportFormat::Doc) => {
            if ws.files.is_empty() {
                l.line_text(Font::HelveticaOblique, 10.0, GREY, MARGIN, "(no files in workspace)");
            }
            for (i, f) in ws.files.iter().enumerate() {
                let mut nums = vec![i + 1];
                doc_file_section(&mut l, f, &mut nums);
            }
        }
        (Target::Project, ExportFormat::Blocks) => {
            if ws.files.is_empty() {
                l.line_text(Font::HelveticaOblique, 10.0, GREY, MARGIN, "(no files in workspace)");
            }
            for f in &ws.files {
                l.ensure(34.0);
                l.y -= 6.0;
                l.pdf.bookmark(&f.path, l.page, l.y + 4.0, 0);
                let heading =
                    truncate_to_width(Font::HelveticaBold, 12.0, &f.path, CONTENT_W);
                l.pdf.text(l.page, Font::HelveticaBold, 12.0, INK, MARGIN, l.y - 12.0, &heading);
                l.y -= 12.0 * 1.6;
                if f.elements.is_empty() {
                    l.line_text(Font::HelveticaOblique, 9.0, GREY, MARGIN, "(empty file)");
                }
                blocks_children(&mut l, &f.elements, MARGIN, 1);
                l.y -= 8.0;
            }
        }
    }
    add_footers(&mut l, &title);
    Ok((l.pdf.finish(), filename))
}

// ---------------------------------------------------------------- layout --

/// A vertical color rail alongside an indented child list (blocks format).
/// `top` is the y where the current page's segment starts.
struct Rail {
    x: f32,
    color: Rgb,
    top: f32,
}

/// Cursor-based page layout on top of the Pdf primitives. `y` is the top of
/// the space still free on the current page.
struct Layout {
    pdf: Pdf,
    page: usize,
    y: f32,
    rails: Vec<Rail>,
}

impl Layout {
    fn new(title: &str) -> Layout {
        let mut pdf = Pdf::new(title);
        let page = pdf.add_page();
        Layout { pdf, page, y: TOP_Y, rails: Vec::new() }
    }

    /// Start a new page, closing every active rail at the bottom margin and
    /// restarting it at the top of the new page (clean page breaks).
    fn new_page(&mut self) {
        let page = self.page;
        for r in &self.rails {
            if r.top - MARGIN > 1.0 {
                self.pdf.rect(page, r.color, r.x, MARGIN, RAIL_W, r.top - MARGIN);
            }
        }
        self.page = self.pdf.add_page();
        self.y = TOP_Y;
        for r in &mut self.rails {
            r.top = TOP_Y;
        }
    }

    /// Page-break unless `h` points still fit above the bottom margin.
    fn ensure(&mut self, h: f32) {
        if self.y - h < MARGIN {
            self.new_page();
        }
    }

    fn push_rail(&mut self, x: f32, color: Rgb) {
        self.rails.push(Rail { x, color, top: self.y });
    }

    fn pop_rail(&mut self) {
        if let Some(r) = self.rails.pop() {
            if r.top - self.y > 1.0 {
                self.pdf.rect(self.page, r.color, r.x, self.y, RAIL_W, r.top - self.y);
            }
        }
    }

    /// One line of text at `x`; advances the cursor.
    fn line_text(&mut self, font: Font, size: f32, color: Rgb, x: f32, s: &str) {
        let lh = size * 1.35;
        self.ensure(lh);
        self.pdf.text(self.page, font, size, color, x, self.y - size, s);
        self.y -= lh;
    }

    /// Word-wrapped paragraph.
    fn para(&mut self, font: Font, size: f32, color: Rgb, x: f32, width: f32, text: &str) {
        for line in wrap_text(font, size, text, width) {
            self.line_text(font, size, color, x, &line);
        }
    }
}

fn title_block(l: &mut Layout, title: &str) {
    l.y -= 4.0;
    let t = truncate_to_width(Font::HelveticaBold, 20.0, title, CONTENT_W);
    l.pdf.text(l.page, Font::HelveticaBold, 20.0, INK, MARGIN, l.y - 20.0, &t);
    l.y -= 20.0 * 1.4;
    let sub = format!("SysML v2 model export \u{00B7} generated {}", today_string());
    l.line_text(Font::Helvetica, 9.5, GREY, MARGIN, &sub);
    l.y -= 4.0;
    l.pdf.line(l.page, LIGHT, 1.0, MARGIN, l.y, PAGE_W - MARGIN, l.y);
    l.y -= 16.0;
}

/// Small grey footer on every page. Called after all content is laid out so
/// the total page count is known.
fn add_footers(l: &mut Layout, title: &str) {
    let total = l.pdf.page_count();
    let t = truncate_to_width(Font::Helvetica, 8.0, title, CONTENT_W * 0.6);
    for p in 0..total {
        let s = format!("{}  \u{00B7}  page {} of {}", t, p + 1, total);
        let w = Pdf::text_width(Font::Helvetica, 8.0, &s);
        l.pdf.text(p, Font::Helvetica, 8.0, GREY, (PAGE_W - w) / 2.0, 30.0, &s);
    }
}

// ------------------------------------------------------------ doc format --

/// A file as a top-level numbered section (project scope).
fn doc_file_section(l: &mut Layout, f: &FileModel, nums: &mut Vec<usize>) {
    let num: String = nums.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(".");
    l.ensure(44.0);
    l.y -= 8.0;
    l.pdf.bookmark(&format!("{} {}", num, f.path), l.page, l.y + 4.0, 0);
    let heading = truncate_to_width(
        Font::HelveticaBold,
        15.0,
        &format!("{}  {}", num, f.path),
        CONTENT_W - 40.0,
    );
    let hw = Pdf::text_width(Font::HelveticaBold, 15.0, &heading);
    l.pdf.text(l.page, Font::HelveticaBold, 15.0, INK, MARGIN, l.y - 15.0, &heading);
    l.pdf.text(l.page, Font::Helvetica, 6.5, GREY, MARGIN + hw + 8.0, l.y - 15.0, "FILE");
    l.y -= 15.0 * 1.6;
    if f.elements.is_empty() {
        l.line_text(Font::HelveticaOblique, 9.5, GREY, MARGIN, "(empty file)");
    }
    doc_children(l, &f.elements, nums, 1);
    l.y -= 6.0;
}

/// One structural element as a numbered section: heading, bookmark, body.
fn doc_section(l: &mut Layout, el: &Element, nums: &mut Vec<usize>, depth: usize) {
    if depth > MAX_DEPTH {
        l.line_text(Font::HelveticaOblique, 9.0, GREY, MARGIN, "\u{2026} (nesting too deep)");
        return;
    }
    let num: String = nums.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(".");
    let mut name = display_name(el);
    if let Some(sn) = &el.short_name {
        name = format!("{} <{}>", name, sn);
    }
    let size = match nums.len() {
        1 => 15.0,
        2 => 12.5,
        3 => 11.0,
        _ => 10.0,
    };
    l.ensure(size * 3.0);
    l.y -= 8.0;
    l.pdf.bookmark(
        &format!("{} {}", num, name),
        l.page,
        l.y + 4.0,
        nums.len().saturating_sub(1).min(6),
    );
    let heading =
        truncate_to_width(Font::HelveticaBold, size, &format!("{}  {}", num, name), CONTENT_W - 60.0);
    let hw = Pdf::text_width(Font::HelveticaBold, size, &heading);
    l.pdf.text(l.page, Font::HelveticaBold, size, INK, MARGIN, l.y - size, &heading);
    l.pdf.text(
        l.page,
        Font::Helvetica,
        6.5,
        GREY,
        MARGIN + hw + 8.0,
        l.y - size,
        &el.kind.to_uppercase(),
    );
    l.y -= size * 1.6;
    // typing / specialization / value line, when present
    let rel = rel_text(el);
    if !rel.is_empty() {
        let line = truncate_to_width(Font::Courier, 8.5, &rel, CONTENT_W);
        l.line_text(Font::Courier, 8.5, GREY, MARGIN, &line);
        l.y -= 2.0;
    }
    // a doc/comment/raw element exported directly (scope=element) carries
    // its content on itself, not in children — render it here
    if matches!(el.kind.as_str(), "doc" | "comment") && el.text.is_some() {
        doc_paragraph(l, el);
    } else if el.kind == "raw" {
        raw_block(l, el);
    }
    doc_children(l, &el.children, nums, depth + 1);
    l.y -= 4.0;
}

/// Render the members of a section: docs, tables, lists, then nested
/// numbered subsections for structural children.
fn doc_children(l: &mut Layout, children: &[Element], nums: &mut Vec<usize>, depth: usize) {
    if depth > MAX_DEPTH {
        l.line_text(Font::HelveticaOblique, 9.0, GREY, MARGIN, "\u{2026} (nesting too deep)");
        return;
    }
    let mut docs: Vec<&Element> = Vec::new();
    let mut attrs: Vec<&Element> = Vec::new();
    let mut ports: Vec<&Element> = Vec::new();
    let mut connects: Vec<&Element> = Vec::new();
    let mut imports: Vec<&Element> = Vec::new();
    let mut raws: Vec<&Element> = Vec::new();
    let mut leaves: Vec<&Element> = Vec::new();
    let mut sections: Vec<&Element> = Vec::new();
    for c in children {
        if is_structural(c) {
            sections.push(c);
            continue;
        }
        match c.kind.as_str() {
            "doc" | "comment" => docs.push(c),
            "attribute" | "enum" => attrs.push(c),
            "port" => ports.push(c),
            "connect" => connects.push(c),
            "import" => imports.push(c),
            "raw" => raws.push(c),
            _ => leaves.push(c),
        }
    }

    for d in &docs {
        doc_paragraph(l, d);
    }
    if !attrs.is_empty() {
        list_label(l, "Attributes");
        let rows: Vec<Vec<String>> = attrs
            .iter()
            .map(|a| {
                let mut ty = a.typed_by.join(", ");
                if ty.is_empty() && !a.specializes.is_empty() {
                    ty = format!(":> {}", a.specializes.join(", "));
                }
                if ty.is_empty() && !a.redefines.is_empty() {
                    ty = format!(":>> {}", a.redefines.join(", "));
                }
                if let Some(m) = &a.multiplicity {
                    ty = format!("{} [{}]", ty, m).trim().to_string();
                }
                vec![
                    display_name(a),
                    ty,
                    a.value.as_deref().map(collapse_ws).unwrap_or_default(),
                ]
            })
            .collect();
        table(l, &["Name", "Type", "Value"], &[0.34, 0.38, 0.28], &rows);
    }
    if !ports.is_empty() {
        list_label(l, "Ports");
        let rows: Vec<Vec<String>> = ports
            .iter()
            .map(|p| {
                let dir: Vec<&str> = p
                    .modifiers
                    .iter()
                    .filter(|m| matches!(m.as_str(), "in" | "out" | "inout"))
                    .map(|m| m.as_str())
                    .collect();
                vec![display_name(p), p.typed_by.join(", "), dir.join(" ")]
            })
            .collect();
        table(l, &["Name", "Type", "Direction"], &[0.4, 0.4, 0.2], &rows);
    }
    for leaf in &leaves {
        let s = format!("\u{2022} {}", one_liner(leaf));
        l.para(Font::Courier, 8.5, INK, MARGIN + 4.0, CONTENT_W - 4.0, &s);
    }
    if !leaves.is_empty() {
        l.y -= 3.0;
    }
    if !connects.is_empty() {
        list_label(l, "Connections");
        for c in &connects {
            let s = if c.connect_ends.is_empty() {
                "connect".to_string()
            } else {
                c.connect_ends.join(" <-> ")
            };
            let line = truncate_to_width(Font::Courier, 8.5, &s, CONTENT_W - 10.0);
            l.line_text(Font::Courier, 8.5, INK, MARGIN + 10.0, &line);
        }
        l.y -= 3.0;
    }
    if !imports.is_empty() {
        list_label(l, "Imports");
        for im in &imports {
            let s = im.name.clone().unwrap_or_else(|| "(import)".to_string());
            let line = truncate_to_width(Font::Courier, 8.5, &s, CONTENT_W - 10.0);
            l.line_text(Font::Courier, 8.5, INK, MARGIN + 10.0, &line);
        }
        l.y -= 3.0;
    }
    for r in &raws {
        raw_block(l, r);
    }
    for (i, s) in sections.iter().enumerate() {
        nums.push(i + 1);
        doc_section(l, s, nums, depth + 1);
        nums.pop();
    }
}

/// Italic grey paragraph for a doc/comment member (whitespace collapsed).
fn doc_paragraph(l: &mut Layout, el: &Element) {
    let flat = collapse_ws(el.text.as_deref().unwrap_or(""));
    if flat.is_empty() {
        return;
    }
    l.para(Font::HelveticaOblique, 9.5, GREY, MARGIN, CONTENT_W, &flat);
    l.y -= 4.0;
}

fn list_label(l: &mut Layout, s: &str) {
    l.y -= 2.0;
    l.line_text(Font::HelveticaBold, 9.0, GREY, MARGIN, s);
}

/// Verbatim Courier block on a light grey background, line breaks kept.
fn raw_block(l: &mut Layout, el: &Element) {
    let text = el.raw.clone().or_else(|| el.text.clone()).unwrap_or_default();
    if text.trim().is_empty() {
        return;
    }
    let size = 8.0;
    let lh = size * 1.3;
    let maxc = (((CONTENT_W - 12.0) / (size * 0.6)) as usize).max(8);
    l.y -= 3.0;
    for line in text.lines() {
        for chunk in chunk_chars(line, maxc) {
            l.ensure(lh);
            l.pdf.rect(l.page, CODE_BG, MARGIN, l.y - lh + 2.0, CONTENT_W, lh);
            l.pdf.text(l.page, Font::Courier, size, CODE_INK, MARGIN + 6.0, l.y - size, &chunk);
            l.y -= lh;
        }
    }
    l.y -= 6.0;
}

/// Simple three-column table with a bold header row and hairline rules.
/// Rows wrap and may break across pages (the header is repeated).
fn table(l: &mut Layout, headers: &[&str], fracs: &[f32], rows: &[Vec<String>]) {
    let x = MARGIN;
    let widths: Vec<f32> = fracs.iter().map(|f| f * CONTENT_W).collect();
    let size = 8.5;
    let lh = size * 1.35;
    let pad = 3.0;
    table_header(l, x, &widths, headers, size, lh, pad);
    for row in rows {
        let cells: Vec<Vec<String>> = row
            .iter()
            .zip(widths.iter())
            .map(|(c, w)| wrap_text(Font::Helvetica, size, c, (w - 2.0 * pad).max(10.0)))
            .collect();
        let nlines = cells.iter().map(|c| c.len().max(1)).max().unwrap_or(1);
        let rh = nlines as f32 * lh + 2.0 * pad;
        if l.y - rh < MARGIN && rh < PAGE_H - 2.0 * MARGIN - 2.0 * lh {
            l.new_page();
            table_header(l, x, &widths, headers, size, lh, pad);
        }
        let mut cx = x;
        for (ci, cell) in cells.iter().enumerate() {
            for (li, line) in cell.iter().enumerate() {
                let by = l.y - pad - size - li as f32 * lh;
                if by < 4.0 {
                    break; // clip a pathological row at the page bottom
                }
                l.pdf.text(l.page, Font::Helvetica, size, INK, cx + pad, by, line);
            }
            cx += widths[ci];
        }
        l.y -= rh;
        l.pdf.line(l.page, LIGHT, 0.5, x, l.y, x + CONTENT_W, l.y);
    }
    l.y -= 8.0;
}

fn table_header(l: &mut Layout, x: f32, widths: &[f32], headers: &[&str], size: f32, lh: f32, pad: f32) {
    l.ensure(lh + 2.0 * pad + lh); // header plus room for one row
    l.pdf.line(l.page, GREY, 0.8, x, l.y, x + CONTENT_W, l.y);
    let mut cx = x;
    for (i, h) in headers.iter().enumerate() {
        l.pdf.text(l.page, Font::HelveticaBold, size, INK, cx + pad, l.y - pad - size, h);
        cx += widths[i];
    }
    l.y -= lh + 2.0 * pad;
    l.pdf.line(l.page, GREY, 0.8, x, l.y, x + CONTENT_W, l.y);
}

// --------------------------------------------------------- blocks format --

fn blocks_children(l: &mut Layout, children: &[Element], x: f32, depth: usize) {
    if depth > MAX_DEPTH {
        l.line_text(Font::HelveticaOblique, 8.5, GREY, x, "\u{2026} (nesting too deep)");
        return;
    }
    for c in children {
        blocks_element(l, c, x, depth);
    }
}

/// One element as a colored header bar (or doc/raw/connect special forms),
/// then its children indented with a color rail.
fn blocks_element(l: &mut Layout, el: &Element, x: f32, depth: usize) {
    if depth > MAX_DEPTH {
        l.line_text(Font::HelveticaOblique, 8.5, GREY, x, "\u{2026} (nesting too deep)");
        return;
    }
    let w = PAGE_W - MARGIN - x;
    match el.kind.as_str() {
        "doc" | "comment" => {
            let flat = collapse_ws(el.text.as_deref().unwrap_or(""));
            if !flat.is_empty() {
                l.para(Font::HelveticaOblique, 8.5, GREY, x + 2.0, (w - 4.0).max(30.0), &flat);
                l.y -= 3.0;
            }
        }
        "raw" => {
            let text = el.raw.clone().unwrap_or_default();
            let size = 7.5;
            let maxc = (((w - 4.0) / (size * 0.6)) as usize).max(8);
            for line in text.lines() {
                for chunk in chunk_chars(line, maxc) {
                    l.ensure(size * 1.3);
                    l.pdf.text(l.page, Font::Courier, size, CODE_INK, x + 2.0, l.y - size, &chunk);
                    l.y -= size * 1.3;
                }
            }
            l.y -= 3.0;
        }
        "connect" => {
            let s = if el.connect_ends.is_empty() {
                "connect".to_string()
            } else {
                el.connect_ends.join(" <-> ")
            };
            let size = 8.0;
            let s = truncate_to_width(Font::Courier, size, &s, (w - 12.0).max(20.0));
            let tw = Pdf::text_width(Font::Courier, size, &s);
            let h = 12.0;
            l.ensure(h + 2.0);
            l.pdf.rounded_rect(l.page, kind_color("connect"), x, l.y - h, tw + 10.0, h, 3.0);
            l.pdf.text(l.page, Font::Courier, size, WHITE, x + 5.0, l.y - h + 3.5, &s);
            l.y -= h + 3.0;
        }
        _ => {
            let color = kind_color(&el.kind);
            let h = 15.0;
            l.ensure(h + 4.0);
            if is_structural(el) && el.name.is_some() {
                l.pdf.bookmark(&display_name(el), l.page, l.y + 2.0, depth.min(6));
            }
            l.pdf.rounded_rect(l.page, color, x, l.y - h, w, h, 3.0);
            let base = l.y - h + 4.5;
            let mut cx = x + 6.0;
            let kindlab = el.kind.to_uppercase();
            l.pdf.text(l.page, Font::Helvetica, 6.0, WHITE, cx, base + 0.5, &kindlab);
            cx += Pdf::text_width(Font::Helvetica, 6.0, &kindlab) + 6.0;
            let mut name = el.name.clone().or_else(|| el.short_name.clone()).unwrap_or_default();
            if let (Some(_), Some(sn)) = (&el.name, &el.short_name) {
                name = format!("{} <{}>", name, sn);
            }
            if !name.is_empty() {
                let nt = truncate_to_width(Font::HelveticaBold, 9.0, &name, (x + w - cx - 8.0).max(10.0));
                l.pdf.text(l.page, Font::HelveticaBold, 9.0, WHITE, cx, base, &nt);
                cx += Pdf::text_width(Font::HelveticaBold, 9.0, &nt) + 6.0;
            }
            let mut meta = el.modifiers.join(" ");
            let rel = rel_text(el);
            if !rel.is_empty() {
                if !meta.is_empty() {
                    meta.push(' ');
                }
                meta.push_str(&rel);
            }
            if !meta.is_empty() && x + w - cx > 24.0 {
                let mt = truncate_to_width(Font::Helvetica, 8.0, &meta, x + w - cx - 6.0);
                l.pdf.text(l.page, Font::Helvetica, 8.0, WHITE, cx, base, &mt);
            }
            l.y -= h + 3.0;
            if !el.children.is_empty() {
                l.push_rail(x + 3.0, color);
                blocks_children(l, &el.children, x + 14.0, depth + 1);
                l.pop_rail();
                l.y -= 3.0;
            }
        }
    }
}

// -------------------------------------------------------------- helpers --

/// Does this element get its own numbered section / header bar hierarchy?
fn is_structural(el: &Element) -> bool {
    if matches!(el.kind.as_str(), "doc" | "comment" | "raw" | "import" | "connect") {
        return false;
    }
    !el.children.is_empty() || el.kind == "package" || el.kind.ends_with(" def")
}

fn display_name(el: &Element) -> String {
    el.name
        .clone()
        .or_else(|| el.short_name.clone())
        .unwrap_or_else(|| format!("({})", el.kind))
}

/// `: T`, `:> S`, `:>> R`, `[m]`, `= v` — whichever apply, joined.
fn rel_text(el: &Element) -> String {
    let mut p: Vec<String> = Vec::new();
    if !el.typed_by.is_empty() {
        p.push(format!(": {}", el.typed_by.join(", ")));
    }
    if !el.specializes.is_empty() {
        p.push(format!(":> {}", el.specializes.join(", ")));
    }
    if !el.redefines.is_empty() {
        p.push(format!(":>> {}", el.redefines.join(", ")));
    }
    if let Some(m) = &el.multiplicity {
        p.push(format!("[{}]", collapse_ws(m)));
    }
    if let Some(v) = &el.value {
        p.push(format!("= {}", collapse_ws(v)));
    }
    p.join(" ")
}

/// `part battery : BatteryPack [4]`-style single line for leaf usages.
fn one_liner(el: &Element) -> String {
    let mut s = String::new();
    if !el.modifiers.is_empty() {
        s.push_str(&el.modifiers.join(" "));
        s.push(' ');
    }
    s.push_str(&el.kind);
    if let Some(sn) = &el.short_name {
        s.push_str(&format!(" <{}>", sn));
    }
    if let Some(n) = &el.name {
        s.push(' ');
        s.push_str(n);
    }
    let rel = rel_text(el);
    if !rel.is_empty() {
        s.push(' ');
        s.push_str(&rel);
    }
    s
}

/// Kind → color, mirroring the web UI's blockColor() precedence.
fn kind_color(kind: &str) -> Rgb {
    if kind == "package" {
        return rgb(0x8e5bc6);
    }
    if kind.ends_with(" def") {
        if kind.starts_with("part") {
            return rgb(0x2f66c4);
        }
        if kind.starts_with("attribute") || kind.starts_with("enum") {
            return rgb(0x1f8a70);
        }
        if kind.starts_with("port") {
            return rgb(0xe68a2e);
        }
        if kind.starts_with("interface") || kind.starts_with("connection") {
            return rgb(0x159ca3);
        }
        if kind.starts_with("requirement") {
            return rgb(0xc94f4f);
        }
        if kind.starts_with("constraint") {
            return rgb(0xb03a5b);
        }
        if kind.starts_with("action")
            || kind.starts_with("state")
            || kind.starts_with("calc")
            || kind.starts_with("use case")
            || kind.starts_with("analysis")
        {
            return rgb(0xc9a227);
        }
        return rgb(0x2f66c4);
    }
    match kind {
        "part" | "item" | "individual" => rgb(0x4e8fe0),
        "attribute" | "enum" => rgb(0x2e9e5b),
        "port" | "end" => rgb(0xe68a2e),
        "connect" | "connection" | "interface" | "bind" | "flow" => rgb(0x159ca3),
        "action" | "state" | "perform" | "exhibit" | "transition" | "calc" | "use case" => {
            rgb(0xc9a227)
        }
        "requirement" | "satisfy" | "verify" | "assume" | "require" | "objective" => rgb(0xc94f4f),
        "constraint" | "assert" => rgb(0xb03a5b),
        "import" => rgb(0x5b6b85),
        "doc" | "comment" => rgb(0x7c8698),
        _ => rgb(0x6b7280),
    }
}

fn rgb(hex: u32) -> Rgb {
    (
        ((hex >> 16) & 0xFF) as f32 / 255.0,
        ((hex >> 8) & 0xFF) as f32 / 255.0,
        (hex & 0xFF) as f32 / 255.0,
    )
}

/// Greedy word wrap using the writer's width metrics. Newlines in `text`
/// force breaks; single over-wide words are hard-broken by character.
fn wrap_text(font: Font, size: f32, text: &str, width: f32) -> Vec<String> {
    let width = width.max(20.0);
    let space = Pdf::char_width(font, size, ' ');
    let mut out = Vec::new();
    for src in text.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0.0f32;
        let mut any = false;
        for word in src.split_whitespace() {
            any = true;
            let ww = Pdf::text_width(font, size, word);
            if cur.is_empty() {
                if ww <= width {
                    cur = word.to_string();
                    cur_w = ww;
                } else {
                    cur_w = hard_break(font, size, word, width, &mut out, &mut cur);
                }
            } else if cur_w + space + ww <= width {
                cur.push(' ');
                cur.push_str(word);
                cur_w += space + ww;
            } else {
                out.push(std::mem::take(&mut cur));
                if ww <= width {
                    cur = word.to_string();
                    cur_w = ww;
                } else {
                    cur_w = hard_break(font, size, word, width, &mut out, &mut cur);
                }
            }
        }
        if !cur.is_empty() || !any {
            out.push(cur);
        }
        let _ = cur_w;
    }
    out
}

/// Break one over-wide word character-by-character; leaves the trailing
/// partial piece in `cur` and returns its width.
fn hard_break(
    font: Font,
    size: f32,
    word: &str,
    width: f32,
    out: &mut Vec<String>,
    cur: &mut String,
) -> f32 {
    let mut w = 0.0f32;
    for ch in word.chars() {
        let cw = Pdf::char_width(font, size, ch);
        if w + cw > width && !cur.is_empty() {
            out.push(std::mem::take(cur));
            w = 0.0;
        }
        cur.push(ch);
        w += cw;
    }
    w
}

/// Split a line into fixed-size character chunks (for fixed-pitch Courier).
fn chunk_chars(line: &str, maxc: usize) -> Vec<String> {
    if line.chars().count() <= maxc {
        return vec![line.to_string()];
    }
    let chars: Vec<char> = line.chars().collect();
    chars.chunks(maxc).map(|c| c.iter().collect()).collect()
}

fn truncate_to_width(font: Font, size: f32, s: &str, width: f32) -> String {
    if Pdf::text_width(font, size, s) <= width {
        return s.to_string();
    }
    let ellw = Pdf::char_width(font, size, '\u{2026}');
    let mut out = String::new();
    let mut w = 0.0f32;
    for ch in s.chars() {
        let cw = Pdf::char_width(font, size, ch);
        if w + cw + ellw > width {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('\u{2026}');
    out
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn file_stem(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    base.strip_suffix(".sysml").unwrap_or(base).to_string()
}

/// Keep ASCII alnum, `-`, `_`, `.`; map separators to `_`; never empty.
fn sanitize_filename(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            out.push(c);
        } else if c == ' ' || c == '/' || c == '\\' {
            out.push('_');
        }
    }
    let out = out.trim_matches(|c| c == '.' || c == '_').to_string();
    if out.is_empty() {
        "export".into()
    } else {
        out
    }
}

fn today_string() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// UNIX day count → (year, month, day). Standard days-from-civil inverse
/// (Howard Hinnant's `civil_from_days`), valid far beyond any plausible date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{assign_ids, Parser};

    fn ws_from(src: &str) -> Workspace {
        let mut elements = Parser::new(src).parse_file();
        assign_ids(0, &mut elements);
        Workspace {
            root: ".".into(),
            files: vec![FileModel {
                path: "Test.sysml".into(),
                elements,
                source: src.to_string(),
                mtime: None,
            }],
        }
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    const SRC: &str = r#"package Demo {
        doc /* A demo package for export tests. */
        import Definitions::*;
        part def Widget {
            attribute mass : Real = 1.0;
            attribute label : String = "hello";
            port pwr : PowerPort;
            part gear : Gear[2];
            connect a.b to c.d;
        }
        requirement def <R1> MaxMass {
            doc /* Mass shall not exceed 1.5 kg. */
            attribute limit : Real = 1.5;
        }
        some unparsed nonsense here;
    }"#;

    #[test]
    fn doc_format_project_export() {
        let ws = ws_from(SRC);
        let (bytes, name) = export_pdf(&ws, &ExportScope::Project, ExportFormat::Doc).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.len() > 2000, "pdf too small: {}", bytes.len());
        assert_eq!(name, "sysml-project.pdf");
        assert!(contains(&bytes, b"Widget"), "element name in content");
        assert!(contains(&bytes, b"MaxMass"));
        assert!(contains(&bytes, b"a.b <-> c.d"), "connection line");
    }

    #[test]
    fn blocks_format_file_export() {
        let ws = ws_from(SRC);
        let (bytes, name) =
            export_pdf(&ws, &ExportScope::File("Test.sysml".into()), ExportFormat::Blocks).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.len() > 1500);
        assert_eq!(name, "Test.pdf");
        assert!(contains(&bytes, b"Widget"));
        assert!(contains(&bytes, b"PART DEF"), "uppercase kind label");
    }

    #[test]
    fn element_scope_by_id() {
        let ws = ws_from(SRC);
        let (bytes, name) =
            export_pdf(&ws, &ExportScope::Element("f0.0".into()), ExportFormat::Doc).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert_eq!(name, "Demo.pdf");
        assert!(contains(&bytes, b"Demo"));
    }

    #[test]
    fn unknown_id_and_file_error() {
        let ws = ws_from(SRC);
        assert!(export_pdf(&ws, &ExportScope::Element("f9.9".into()), ExportFormat::Doc).is_err());
        assert!(export_pdf(&ws, &ExportScope::File("Nope.sysml".into()), ExportFormat::Blocks).is_err());
    }

    #[test]
    fn robust_on_odd_input() {
        // unnamed defs, bare junk, empty file, empty project
        let ws = ws_from("part def ;\nattribute;\n@#$ nonsense !!;\n");
        for fmt in [ExportFormat::Doc, ExportFormat::Blocks] {
            let (bytes, _) = export_pdf(&ws, &ExportScope::Project, fmt).unwrap();
            assert!(bytes.starts_with(b"%PDF-"));
        }
        let empty_file = ws_from("");
        let (bytes, _) = export_pdf(&empty_file, &ExportScope::Project, ExportFormat::Doc).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        let empty = Workspace { root: String::new(), files: vec![] };
        let (bytes, name) = export_pdf(&empty, &ExportScope::Project, ExportFormat::Blocks).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert_eq!(name, "sysml-project.pdf");
    }

    #[test]
    fn deep_nesting_capped_not_paniced() {
        let mut src = String::new();
        for i in 0..80 {
            src.push_str(&format!("part a{} {{ ", i));
        }
        src.push_str("attribute x : Real;");
        for _ in 0..80 {
            src.push('}');
        }
        let ws = ws_from(&src);
        for fmt in [ExportFormat::Doc, ExportFormat::Blocks] {
            let (bytes, _) = export_pdf(&ws, &ExportScope::Project, fmt).unwrap();
            assert!(bytes.starts_with(b"%PDF-"));
            assert!(contains(&bytes, b"nesting too deep"), "depth cap notice present");
        }
    }

    #[test]
    fn filename_sanitized() {
        assert_eq!(sanitize_filename("Drone System/α β"), "Drone_System");
        assert_eq!(sanitize_filename("..."), "export");
        assert_eq!(sanitize_filename("Quad-copter_v2"), "Quad-copter_v2");
    }

    #[test]
    fn civil_date_inverse() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1)); // 2024-01-01
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
