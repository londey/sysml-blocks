//! Workspace model: scans the mapped volume for .sysml files, keeps a parsed
//! snapshot, and applies edits as *text splices* so untouched lines keep
//! their exact formatting (git-diff friendly).

use crate::parser::{assign_ids, Element, Parser, Span};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize)]
pub struct FileModel {
    pub path: String, // relative to workspace root
    pub elements: Vec<Element>,
    #[serde(skip)]
    pub source: String,
    #[serde(skip)]
    pub mtime: Option<SystemTime>,
}

#[derive(Debug, Default, Serialize)]
pub struct Workspace {
    pub root: String,
    pub files: Vec<FileModel>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditOp {
    Rename { id: String, name: String },
    SetValue { id: String, value: String },
    AddChild { parent: String, kind: String, name: String, extra: Option<String> },
    AddRoot { file: String, kind: String, name: String },
    Delete { id: String },
    /// Replace an element's full source text (power-user / raw block editing)
    SetRaw { id: String, text: String },
    /// Move an element to a new parent (or to file root when new_parent is
    /// None) at child position `index` (position in the *current* child list,
    /// dragged element still counted).
    Move { id: String, new_parent: Option<String>, file: Option<String>, index: usize },
    NewFile { path: String },
}

impl Workspace {
    pub fn load(root: &Path) -> Workspace {
        let mut files = Vec::new();
        let mut paths = Vec::new();
        collect_sysml(root, root, &mut paths);
        paths.sort();
        for rel in paths {
            let abs = root.join(&rel);
            let source = fs::read_to_string(&abs).unwrap_or_default();
            let mtime = fs::metadata(&abs).and_then(|m| m.modified()).ok();
            let mut elements = Parser::new(&source).parse_file();
            files.push(FileModel {
                path: rel.to_string_lossy().replace('\\', "/"),
                elements: {
                    assign_ids(files.len(), &mut elements);
                    elements
                },
                source,
                mtime,
            });
        }
        Workspace { root: root.to_string_lossy().to_string(), files }
    }

    /// Re-read any file whose mtime changed on disk (external edits, git pull,
    /// OneDrive sync, ...). Also picks up new/removed files.
    pub fn refresh(&mut self, root: &Path) {
        let fresh = Workspace::load(root);
        self.files = fresh.files;
        self.root = fresh.root;
    }

    fn find<'a>(files: &'a [FileModel], id: &str) -> Option<(usize, &'a Element)> {
        let fidx: usize = id
            .strip_prefix('f')?
            .split('.')
            .next()?
            .parse()
            .ok()?;
        let file = files.get(fidx)?;
        let mut cur: Option<&Element> = None;
        let mut list = &file.elements;
        for part in id.split('.').skip(1) {
            let i: usize = part.parse().ok()?;
            cur = list.get(i);
            list = &cur?.children;
        }
        cur.map(|e| (fidx, e))
    }

    pub fn apply(&mut self, root: &Path, op: &EditOp) -> Result<(), String> {
        match op {
            EditOp::Rename { id, name } => {
                let (fidx, el) =
                    Self::find(&self.files, id).ok_or("element not found")?;
                let ns = el
                    .name_span
                    .clone()
                    .ok_or("element has no name to rename")?;
                let new = sanitize_name(name)?;
                self.splice(root, fidx, ns, &new)
            }
            EditOp::SetValue { id, value } => {
                let (fidx, el) =
                    Self::find(&self.files, id).ok_or("element not found")?;
                if let Some(vs) = el.value_span.clone() {
                    self.splice(root, fidx, vs, value.trim())
                } else {
                    // insert ` = value` before the terminating `;`
                    let sp = el.span.clone();
                    let src = &self.files[fidx].source;
                    let stmt = &src[sp.start..sp.end];
                    let semi = stmt
                        .rfind(';')
                        .ok_or("cannot add a value to this element")?;
                    let at = sp.start + semi;
                    self.splice(
                        root,
                        fidx,
                        Span { start: at, end: at },
                        &format!(" = {}", value.trim()),
                    )
                }
            }
            EditOp::AddChild { parent, kind, name, extra } => {
                let (fidx, el) =
                    Self::find(&self.files, parent).ok_or("parent not found")?;
                let new_name = sanitize_name(name)?;
                let extra = extra.clone().unwrap_or_default();
                let stmt = build_stmt(kind, &new_name, &extra)?;
                if let Some(bs) = el.body_span.clone() {
                    let indent = infer_indent(&self.files[fidx].source, el.span.start) + "    ";
                    let closing_indent = infer_indent(&self.files[fidx].source, el.span.start);
                    let src = &self.files[fidx].source;
                    let body = &src[bs.start..bs.end];
                    let insertion = if body.trim().is_empty() {
                        format!("\n{}{}\n{}", indent, stmt, closing_indent)
                    } else {
                        format!("{}{}\n", indent, stmt)
                    };
                    let at = if body.trim().is_empty() {
                        // rewrite whole empty body
                        return self.splice(root, fidx, bs, &insertion);
                    } else {
                        // insert just before closing brace, after last newline
                        let rel = body.rfind('\n').map(|i| i + 1).unwrap_or(body.len());
                        bs.start + rel
                    };
                    self.splice(root, fidx, Span { start: at, end: at }, &insertion)
                } else {
                    // element ends with `;` — convert to a body
                    let sp = el.span.clone();
                    let src = &self.files[fidx].source;
                    let stmt_txt = &src[sp.start..sp.end];
                    let semi = stmt_txt.rfind(';').ok_or("cannot add children here")?;
                    let indent = infer_indent(src, sp.start);
                    let replacement =
                        format!(" {{\n{}    {}\n{}}}", indent, stmt, indent);
                    let at = sp.start + semi;
                    self.splice(root, fidx, Span { start: at, end: at + 1 }, &replacement)
                }
            }
            EditOp::AddRoot { file, kind, name } => {
                let fidx = self
                    .files
                    .iter()
                    .position(|f| f.path == *file)
                    .ok_or("file not found")?;
                let new_name = sanitize_name(name)?;
                let stmt = build_stmt(kind, &new_name, "")?;
                let end = self.files[fidx].source.len();
                let sep = if self.files[fidx].source.ends_with('\n') || end == 0 {
                    ""
                } else {
                    "\n"
                };
                self.splice(
                    root,
                    fidx,
                    Span { start: end, end },
                    &format!("{}{}\n", sep, stmt),
                )
            }
            EditOp::Delete { id } => {
                let (fidx, el) =
                    Self::find(&self.files, id).ok_or("element not found")?;
                let sp = expanded_span(&self.files[fidx].source, &el.span);
                self.splice(root, fidx, sp, "")
            }
            EditOp::SetRaw { id, text } => {
                let (fidx, el) =
                    Self::find(&self.files, id).ok_or("element not found")?;
                let sp = el.span.clone();
                self.splice(root, fidx, sp, text)
            }
            EditOp::Move { id, new_parent, file, index } => {
                self.apply_move(root, id, new_parent.as_deref(), file.as_deref(), *index)
            }
            EditOp::NewFile { path } => {
                let rel = PathBuf::from(path);
                if rel.is_absolute()
                    || rel.components().any(|c| {
                        matches!(c, std::path::Component::ParentDir)
                    })
                {
                    return Err("invalid path".into());
                }
                let mut rel = rel;
                if rel.extension().map(|e| e != "sysml").unwrap_or(true) {
                    rel.set_extension("sysml");
                }
                let abs = root.join(&rel);
                if abs.exists() {
                    return Err("file already exists".into());
                }
                if let Some(dir) = abs.parent() {
                    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                }
                fs::write(&abs, "").map_err(|e| e.to_string())?;
                self.refresh(root);
                Ok(())
            }
        }
    }

    fn apply_move(
        &mut self,
        root: &Path,
        id: &str,
        new_parent: Option<&str>,
        file: Option<&str>,
        index: usize,
    ) -> Result<(), String> {
        // ---- source ----
        let (sfidx, sel) = Self::find(&self.files, id).ok_or("element not found")?;
        let s_span_raw = sel.span.clone();
        let s_src = self.files[sfidx].source.clone();
        let s_span = expanded_span(&s_src, &s_span_raw);
        let s_indent = infer_indent(&s_src, s_span_raw.start);
        let mut moved_text = s_src[s_span.start..s_span.end].to_string();
        if !moved_text.ends_with('\n') {
            moved_text.push('\n');
        }

        // ---- target ----
        // (dfidx, plain insertion offset OR full body-replacement splice)
        enum Target {
            At { dfidx: usize, offset: usize, indent: String },
            ReplaceBody { dfidx: usize, span: Span, indent: String, closing: String },
            ConvertSemi { dfidx: usize, semi_at: usize, indent: String },
        }
        let target = match new_parent {
            Some(pid) => {
                if pid == id || pid.starts_with(&format!("{}.", id)) {
                    return Err("cannot move an element into itself".into());
                }
                let (dfidx, pel) =
                    Self::find(&self.files, pid).ok_or("target parent not found")?;
                let d_src = &self.files[dfidx].source;
                let p_indent = infer_indent(d_src, pel.span.start);
                let child_indent = format!("{}    ", p_indent);
                match &pel.body_span {
                    Some(bs) => {
                        let kids = &pel.children;
                        if kids.is_empty() {
                            Target::ReplaceBody {
                                dfidx,
                                span: bs.clone(),
                                indent: child_indent,
                                closing: p_indent,
                            }
                        } else {
                            let i = index.min(kids.len());
                            let offset = if i == 0 {
                                line_start(d_src, kids[0].span.start)
                            } else {
                                expanded_span(d_src, &kids[i - 1].span).end
                            };
                            Target::At { dfidx, offset, indent: child_indent }
                        }
                    }
                    None => {
                        let stmt = &d_src[pel.span.start..pel.span.end];
                        let semi = stmt
                            .rfind(';')
                            .ok_or("target cannot contain children")?;
                        Target::ConvertSemi {
                            dfidx,
                            semi_at: pel.span.start + semi,
                            indent: p_indent,
                        }
                    }
                }
            }
            None => {
                let dfidx = match file {
                    Some(p) => self
                        .files
                        .iter()
                        .position(|f| f.path == *p)
                        .ok_or("target file not found")?,
                    None => sfidx,
                };
                let d_src = &self.files[dfidx].source;
                let roots = &self.files[dfidx].elements;
                let offset = if roots.is_empty() {
                    d_src.len()
                } else {
                    let i = index.min(roots.len());
                    if i == 0 {
                        line_start(d_src, roots[0].span.start)
                    } else {
                        expanded_span(d_src, &roots[i - 1].span).end
                    }
                };
                Target::At { dfidx, offset, indent: String::new() }
            }
        };

        // build splices
        match target {
            Target::At { dfidx, offset, indent } => {
                let text = reindent(&moved_text, &s_indent, &indent);
                if dfidx == sfidx {
                    if offset >= s_span.start && offset <= s_span.end {
                        return Ok(()); // dropped onto itself: no-op
                    }
                    self.splice_many(
                        root,
                        sfidx,
                        vec![
                            (s_span, String::new()),
                            (Span { start: offset, end: offset }, text),
                        ],
                    )
                } else {
                    self.splice(root, dfidx, Span { start: offset, end: offset }, &text)?;
                    // source spans still valid: different file untouched
                    self.splice(root, sfidx, s_span, "")
                }
            }
            Target::ReplaceBody { dfidx, span, indent, closing } => {
                let text = reindent(&moved_text, &s_indent, &indent);
                let replacement = format!("\n{}{}", text, closing);
                if dfidx == sfidx {
                    if span.start >= s_span.start && span.start <= s_span.end {
                        return Err("cannot move an element into itself".into());
                    }
                    self.splice_many(
                        root,
                        sfidx,
                        vec![(s_span, String::new()), (span, replacement)],
                    )
                } else {
                    self.splice(root, dfidx, span, &replacement)?;
                    self.splice(root, sfidx, s_span, "")
                }
            }
            Target::ConvertSemi { dfidx, semi_at, indent } => {
                let text = reindent(&moved_text, &s_indent, &format!("{}    ", indent));
                let replacement = format!(" {{\n{}{}}}", text, indent);
                let span = Span { start: semi_at, end: semi_at + 1 };
                if dfidx == sfidx {
                    if semi_at >= s_span.start && semi_at < s_span.end {
                        return Err("cannot move an element into itself".into());
                    }
                    self.splice_many(
                        root,
                        sfidx,
                        vec![(s_span, String::new()), (span, replacement)],
                    )
                } else {
                    self.splice(root, dfidx, span, &replacement)?;
                    self.splice(root, sfidx, s_span, "")
                }
            }
        }
    }

    /// Apply several non-overlapping splices to one file atomically
    /// (computed against the same snapshot; applied high-offset first).
    fn splice_many(
        &mut self,
        root: &Path,
        fidx: usize,
        mut edits: Vec<(Span, String)>,
    ) -> Result<(), String> {
        let file = &mut self.files[fidx];
        for (sp, _) in &edits {
            if sp.start > file.source.len() || sp.end > file.source.len() {
                return Err("stale span; refresh and retry".into());
            }
        }
        edits.sort_by(|a, b| b.0.start.cmp(&a.0.start));
        // overlap check
        for w in edits.windows(2) {
            if w[1].0.end > w[0].0.start {
                return Err("conflicting edit spans".into());
            }
        }
        let mut new_src = file.source.clone();
        for (sp, rep) in edits {
            new_src.replace_range(sp.start..sp.end, &rep);
        }
        let abs = root.join(&file.path);
        fs::write(&abs, &new_src).map_err(|e| e.to_string())?;
        let mut elements = Parser::new(&new_src).parse_file();
        assign_ids(fidx, &mut elements);
        file.source = new_src;
        file.elements = elements;
        file.mtime = fs::metadata(&abs).and_then(|m| m.modified()).ok();
        Ok(())
    }

    fn splice(
        &mut self,
        root: &Path,
        fidx: usize,
        span: Span,
        replacement: &str,
    ) -> Result<(), String> {
        let file = &mut self.files[fidx];
        if span.start > file.source.len() || span.end > file.source.len() {
            return Err("stale span; refresh and retry".into());
        }
        let mut new_src = String::with_capacity(
            file.source.len() + replacement.len(),
        );
        new_src.push_str(&file.source[..span.start]);
        new_src.push_str(replacement);
        new_src.push_str(&file.source[span.end..]);
        let abs = root.join(&file.path);
        fs::write(&abs, &new_src).map_err(|e| e.to_string())?;
        // reparse this file in place
        let mut elements = Parser::new(&new_src).parse_file();
        assign_ids(fidx, &mut elements);
        file.source = new_src;
        file.elements = elements;
        file.mtime = fs::metadata(&abs).and_then(|m| m.modified()).ok();
        Ok(())
    }
}

fn collect_sysml(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if p.is_dir() {
            collect_sysml(root, &p, out);
        } else if p.extension().map(|e| e == "sysml").unwrap_or(false) {
            if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_path_buf());
            }
        }
    }
}

fn sanitize_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("name cannot be empty".into());
    }
    let plain = n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain && !n.chars().next().unwrap().is_ascii_digit() {
        Ok(n.to_string())
    } else if !n.contains('\'') && !n.contains('\n') {
        Ok(format!("'{}'", n)) // SysML v2 unrestricted name
    } else {
        Err("name contains unsupported characters".into())
    }
}

fn build_stmt(kind: &str, name: &str, extra: &str) -> Result<String, String> {
    const ALLOWED: &[&str] = &[
        "package", "part def", "part", "attribute", "port def", "port",
        "item def", "item", "action def", "action", "requirement def",
        "requirement", "constraint def", "constraint", "connection def",
        "interface def", "enum def", "state def", "state", "doc", "import",
        "connect", "use case def", "use case", "analysis def", "calc def",
        "view def", "concern def",
    ];
    if !ALLOWED.contains(&kind) {
        return Err(format!("unsupported kind '{}'", kind));
    }
    if kind == "doc" {
        let body = if name.is_empty() { "..." } else { name };
        return Ok(format!("doc /* {} */", body.replace("*/", "* /")));
    }
    if kind == "import" {
        return Ok(format!("import {};", name));
    }
    if kind == "connect" {
        // `extra` carries the second end
        let b = if extra.is_empty() { "TODO" } else { extra };
        return Ok(format!("connect {} to {};", name, b));
    }
    let extra = extra.trim();
    if extra.is_empty() {
        Ok(format!("{} {};", kind, name))
    } else if extra.starts_with(':') || extra.starts_with('=') || extra.starts_with('[') {
        Ok(format!("{} {} {};", kind, name, extra))
    } else {
        Ok(format!("{} {} : {};", kind, name, extra))
    }
}

fn infer_indent(src: &str, at: usize) -> String {
    let line_start = src[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    src[line_start..at]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Grow an element span to swallow its leading indentation and the trailing
/// remainder of its final line (whitespace + newline).
fn expanded_span(src: &str, sp: &Span) -> Span {
    let bytes = src.as_bytes();
    let mut s = sp.start;
    while s > 0 && (bytes[s - 1] == b' ' || bytes[s - 1] == b'\t') {
        s -= 1;
    }
    let mut e = sp.end;
    while e < bytes.len() && (bytes[e] == b' ' || bytes[e] == b'\t') {
        e += 1;
    }
    if e < bytes.len() && bytes[e] == b'\n' {
        e += 1;
    }
    Span { start: s, end: e }
}

fn line_start(src: &str, at: usize) -> usize {
    src[..at].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

/// Re-indent a block of lines from one leading indent to another. Lines that
/// don't start with the old indent (e.g. blank lines) are left untouched
/// apart from the new prefix on non-empty lines.
fn reindent(text: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches('\n');
        if bare.trim().is_empty() {
            out.push_str(line); // keep blank lines blank
            continue;
        }
        let stripped = bare.strip_prefix(from).unwrap_or(bare.trim_start());
        out.push_str(to);
        out.push_str(stripped);
        if line.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}
