// sysml-blocks web UI
// Renders the parsed SysML v2 workspace as Scratch-style nested blocks and
// sends small edit operations back to the server, which splices them into
// the .sysml files on the mapped volume.

interface Span { start: number; end: number }
interface Element_ {
  id: string;
  kind: string;
  modifiers: string[];
  name: string | null;
  short_name: string | null;
  typed_by: string[];
  specializes: string[];
  redefines: string[];
  multiplicity: string | null;
  value: string | null;
  connect_ends: string[];
  text: string | null;
  raw: string | null;
  children: Element_[];
  has_body: boolean;
  span: Span;
  name_span: Span | null;
  value_span: Span | null;
  body_span: Span | null;
}
interface FileModel { path: string; elements: Element_[] }
interface Workspace { root: string; files: FileModel[] }

type EditOp =
  | { op: "rename"; id: string; name: string }
  | { op: "set_value"; id: string; value: string }
  | { op: "add_child"; parent: string; kind: string; name: string; extra?: string }
  | { op: "add_root"; file: string; kind: string; name: string }
  | { op: "delete"; id: string }
  | { op: "move"; id: string; new_parent: string | null; file?: string; index: number }
  | { op: "new_file"; path: string };

type ViewMode = "blocks" | "source" | "deps";

let ws: Workspace | null = null;
let activeFile = 0;
let viewMode: ViewMode = "blocks";

const canvas = document.getElementById("canvas")!;
const fileList = document.getElementById("file-list")!;
const rootPath = document.getElementById("root-path")!;
const toast = document.getElementById("toast")!;

// ---------- colors ----------
function blockColor(kind: string): string {
  const k = kind;
  if (k === "package") return "var(--c-package)";
  if (k.endsWith(" def")) {
    if (k.startsWith("part")) return "var(--c-partdef)";
    if (k.startsWith("attribute") || k.startsWith("enum")) return "var(--c-attrdef)";
    if (k.startsWith("port")) return "var(--c-port)";
    if (k.startsWith("interface") || k.startsWith("connection")) return "var(--c-conn)";
    if (k.startsWith("requirement")) return "var(--c-req)";
    if (k.startsWith("constraint")) return "var(--c-constraint)";
    if (k.startsWith("action") || k.startsWith("state") || k.startsWith("calc") ||
        k.startsWith("use case") || k.startsWith("analysis")) return "var(--c-behavior)";
    return "var(--c-partdef)";
  }
  if (k === "part" || k === "item" || k === "individual") return "var(--c-part)";
  if (k === "attribute" || k === "enum") return "var(--c-attr)";
  if (k === "port" || k === "end") return "var(--c-port)";
  if (k === "connect" || k === "connection" || k === "interface" || k === "bind" ||
      k === "flow") return "var(--c-conn)";
  if (k === "action" || k === "state" || k === "perform" || k === "exhibit" ||
      k === "transition" || k === "calc" || k === "use case") return "var(--c-behavior)";
  if (k === "requirement" || k === "satisfy" || k === "verify" ||
      k === "assume" || k === "require" || k === "objective") return "var(--c-req)";
  if (k === "constraint" || k === "assert") return "var(--c-constraint)";
  if (k === "import") return "var(--c-import)";
  if (k === "doc" || k === "comment") return "var(--c-doc)";
  return "var(--c-raw)";
}

// what the “+” palette offers, per container kind
function childKinds(kind: string): string[] {
  if (kind === "package")
    return ["package", "part def", "part", "attribute", "port def",
            "interface def", "requirement def", "requirement", "item def",
            "action def", "enum def", "constraint def", "import", "doc"];
  if (kind.endsWith(" def") || kind === "part" || kind === "item")
    return ["attribute", "part", "port", "action", "state", "constraint",
            "connect", "requirement", "doc"];
  return ["attribute", "doc"];
}

// ---------- API ----------
// every fresh model goes through adoptModel so the dependency graph is
// rebuilt and stale element ids (they shift on re-index) are remapped
function adoptModel(w: Workspace): void {
  ws = w;
  graph = buildGraph(w);
  closeExportDialog(); // a bound element id may have shifted — drop it
  if (activeFile >= w.files.length) activeFile = Math.max(0, w.files.length - 1);
  if (viewMode === "deps") remapDeps();
  render();
  if (!searchPanel.hidden && searchInput.value.trim()) runSearch();
}

async function fetchModel(): Promise<void> {
  const r = await fetch("/api/model");
  adoptModel(await r.json() as Workspace);
}

async function applyEdit(op: EditOp): Promise<void> {
  const r = await fetch("/api/edit", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(op),
  });
  if (r.ok) {
    const w = await r.json() as Workspace;
    adoptModel(w);
    showToast("Saved to " + (w.files[activeFile]?.path ?? "file"));
  } else {
    let msg = "Edit failed";
    try { msg = (await r.json()).error ?? msg; } catch { /* keep default */ }
    showToast(msg, true);
    await fetchModel(); // resync in case spans went stale
  }
}

let toastTimer = 0;
function showToast(msg: string, isErr = false): void {
  toast.textContent = msg;
  toast.className = "show" + (isErr ? " err" : "");
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (toast.className = ""), isErr ? 4200 : 1800);
}

// ---------- rendering ----------
function render(): void {
  if (!ws) return;
  rootPath.textContent = ws.root;

  fileList.innerHTML = "";
  ws.files.forEach((f, i) => {
    const li = document.createElement("li");
    li.textContent = f.path;
    li.className = i === activeFile ? "active" : "";
    li.onclick = () => { activeFile = i; viewMode = "blocks"; render(); };
    fileList.appendChild(li);
  });

  canvas.innerHTML = "";
  canvas.classList.toggle("deps-mode", viewMode === "deps");
  wireState = null;
  if (viewMode === "deps") {
    renderDepsView();
    return;
  }

  const file = ws.files[activeFile];
  if (!file) {
    const p = document.createElement("p");
    p.className = "empty-hint";
    p.textContent =
      "No .sysml files found in the mapped volume. Create one with “New file…” " +
      "or mount a folder containing SysML v2 textual models at /models.";
    canvas.appendChild(p);
    return;
  }

  const title = document.createElement("div");
  title.className = "file-title";
  const tspan = document.createElement("span");
  tspan.textContent = file.path;
  const toggle = document.createElement("button");
  toggle.className = "view-toggle";
  toggle.textContent = viewMode === "source" ? "Blocks" : "Text";
  toggle.onclick = () => {
    viewMode = viewMode === "source" ? "blocks" : "source";
    render();
  };
  title.append(tspan, toggle);
  canvas.appendChild(title);

  if (viewMode === "source") {
    const pre = document.createElement("pre");
    pre.className = "source-view";
    pre.textContent = "loading…";
    canvas.appendChild(pre);
    fetch("/api/source?file=" + encodeURIComponent(file.path))
      .then((r) => r.text())
      .then((t) => {
        pre.textContent = "";
        pre.appendChild(highlightSysml(t));
      });
    return;
  }

  for (const el of file.elements) canvas.appendChild(renderBlock(el));

  const addRoot = document.createElement("button");
  addRoot.className = "add-root";
  addRoot.textContent = "＋ Add top-level element";
  addRoot.onclick = (ev) =>
    openPalette(addRoot, ["package", "part def", "import", "doc"], (kind) =>
      promptNameThen(kind, (name) =>
        applyEdit({ op: "add_root", file: file.path, kind, name })));
  canvas.appendChild(addRoot);
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K, cls?: string, text?: string,
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function canContain(kind: string): boolean {
  return !["doc", "comment", "raw", "import", "connect"].includes(kind);
}

function parentIdOf(id: string): string | null {
  const parts = id.split(".");
  return parts.length > 2 ? parts.slice(0, -1).join(".") : null; // "f0.3" is a root
}
function childIndexOf(id: string): number {
  return parseInt(id.split(".").pop() ?? "0", 10);
}

let dragId: string | null = null;

type DropMode = "before" | "after" | "into";
function clearDropMarks(): void {
  document.querySelectorAll(".drop-before,.drop-after,.drop-into")
    .forEach((n) => n.classList.remove("drop-before", "drop-after", "drop-into"));
}

function dropModeFor(e: Element_, ev: DragEvent, block: HTMLElement): DropMode {
  const r = block.getBoundingClientRect();
  const y = (ev.clientY - r.top) / r.height;
  if (canContain(e.kind)) {
    if (y < 0.25) return "before";
    if (y > 0.75) return "after";
    return "into";
  }
  return y < 0.5 ? "before" : "after";
}

function performDrop(target: Element_, mode: DropMode): void {
  if (!dragId || !ws) return;
  const id = dragId;
  dragId = null;
  if (id === target.id) return;
  if (target.id.startsWith(id + ".")) {
    showToast("Cannot move an element into itself", true);
    return;
  }
  let op: EditOp;
  if (mode === "into") {
    op = { op: "move", id, new_parent: target.id, index: target.children.length };
  } else {
    const parent = parentIdOf(target.id);
    const index = childIndexOf(target.id) + (mode === "after" ? 1 : 0);
    op = {
      op: "move", id, new_parent: parent, index,
      ...(parent === null ? { file: ws.files[activeFile].path } : {}),
    };
  }
  void applyEdit(op);
}

function renderBlock(e: Element_): HTMLElement {
  const b = el("div", "block kind-" + e.kind.replace(/\s+/g, "-"));
  b.style.setProperty("--bc", blockColor(e.kind));
  b.dataset.id = e.id;

  // ---- drag & drop ----
  b.draggable = true;
  b.addEventListener("dragstart", (ev) => {
    ev.stopPropagation();
    dragId = e.id;
    ev.dataTransfer?.setData("text/plain", e.id);
    if (ev.dataTransfer) ev.dataTransfer.effectAllowed = "move";
    b.classList.add("dragging");
  });
  b.addEventListener("dragend", () => {
    b.classList.remove("dragging");
    clearDropMarks();
    dragId = null;
  });
  b.addEventListener("dragover", (ev) => {
    if (!dragId || dragId === e.id || e.id.startsWith(dragId + ".")) return;
    ev.preventDefault();
    ev.stopPropagation();
    if (ev.dataTransfer) ev.dataTransfer.dropEffect = "move";
    clearDropMarks();
    b.classList.add("drop-" + dropModeFor(e, ev, b));
  });
  b.addEventListener("dragleave", (ev) => {
    if (ev.target === b) {
      b.classList.remove("drop-before", "drop-after", "drop-into");
    }
  });
  b.addEventListener("drop", (ev) => {
    ev.preventDefault();
    ev.stopPropagation();
    const mode = dropModeFor(e, ev, b);
    clearDropMarks();
    performDrop(e, mode);
  });

  const head = el("div", "block-head");

  if (e.kind === "doc" || e.kind === "comment") {
    head.appendChild(el("span", "kind", e.kind));
    head.appendChild(el("span", "doc-text", e.text ?? ""));
  } else if (e.kind === "raw") {
    head.appendChild(el("span", "kind", "unparsed"));
    head.appendChild(el("span", "raw-text", (e.raw ?? "").trim()));
  } else if (e.kind === "connect") {
    head.appendChild(el("span", "kind", "connect"));
    const a = el("span", "rel", e.connect_ends[0] ?? "?");
    const to = el("span", "kind", "to");
    const c = el("span", "rel", e.connect_ends[1] ?? "?");
    head.append(a, to, c);
  } else if (e.kind === "import") {
    head.appendChild(el("span", "kind", "import"));
    head.appendChild(el("span", "rel", e.name ?? ""));
  } else {
    if (e.modifiers.length)
      head.appendChild(el("span", "mods", e.modifiers.join(" ")));
    head.appendChild(el("span", "kind", e.kind));
    if (e.short_name)
      head.appendChild(el("span", "shortname", "<" + e.short_name + ">"));

    const name = el("span", "name", e.name ?? "‹unnamed›");
    name.title = "Click to rename";
    name.tabIndex = 0;
    if (e.name_span) {
      const startEdit = () =>
        inlineEdit(name, e.name ?? "", (v) =>
          applyEdit({ op: "rename", id: e.id, name: v }));
      name.onclick = startEdit;
      name.onkeydown = (ev) => { if (ev.key === "Enter") startEdit(); };
    }
    head.appendChild(name);

    for (const t of e.typed_by) head.appendChild(relChip(":", t, e));
    for (const t of e.specializes) head.appendChild(relChip(":>", t, e));
    for (const t of e.redefines) head.appendChild(relChip(":>>", t, e));
    if (e.multiplicity !== null)
      head.appendChild(el("span", "mult", "[" + e.multiplicity + "]"));

    // value chip — click to edit; offer even when absent (for value-ish kinds)
    if (e.value !== null || ["attribute", "item", "calc"].includes(e.kind)) {
      const v = el("span", "value", e.value !== null ? "= " + e.value : "= …");
      v.title = "Click to set value";
      v.tabIndex = 0;
      const startEdit = () =>
        inlineEdit(v, e.value ?? "", (nv) =>
          applyEdit({ op: "set_value", id: e.id, value: nv }));
      v.onclick = startEdit;
      v.onkeydown = (ev) => { if (ev.key === "Enter") startEdit(); };
      head.appendChild(v);
    }
  }

  b.appendChild(head);

  // hover tools: add child, delete
  const tools = el("div", "block-tools");
  if (!["doc", "comment", "raw", "import", "connect"].includes(e.kind)) {
    const add = el("button", "tool-btn", "＋") as HTMLButtonElement;
    add.title = "Add child element";
    add.onclick = (ev) => {
      ev.stopPropagation();
      openPalette(b, childKinds(e.kind), (kind) => {
        if (kind === "connect") {
          const a = prompt("Connect: first end (e.g. battery.powerOut)");
          if (!a) return;
          const c = prompt("Connect: second end (e.g. fc.powerIn)");
          if (!c) return;
          applyEdit({ op: "add_child", parent: e.id, kind, name: a, extra: c });
        } else if (kind === "doc") {
          const text = prompt("Documentation text");
          if (text) applyEdit({ op: "add_child", parent: e.id, kind, name: text });
        } else {
          promptNameThen(kind, (name, extra) =>
            applyEdit({ op: "add_child", parent: e.id, kind, name, extra }));
        }
      });
    };
    tools.appendChild(add);
  }
  const depsBtn = el("button", "tool-btn", "⇄");
  depsBtn.title = "Dependencies";
  depsBtn.onclick = (ev) => {
    ev.stopPropagation();
    openDeps(e.id);
  };
  tools.appendChild(depsBtn);
  const expBtn = el("button", "tool-btn", "⤓");
  expBtn.title = "Export as PDF…";
  expBtn.onclick = (ev) => {
    ev.stopPropagation();
    openExportDialog(expBtn, e);
  };
  tools.appendChild(expBtn);
  const del = el("button", "tool-btn", "✕") as HTMLButtonElement;
  del.title = "Delete element";
  del.onclick = (ev) => {
    ev.stopPropagation();
    const label = (e.kind + " " + (e.name ?? "")).trim();
    if (confirm("Delete " + label + "? This edits the .sysml file.")) {
      applyEdit({ op: "delete", id: e.id });
    }
  };
  tools.appendChild(del);
  b.appendChild(tools);

  if (e.children.length) {
    const body = el("div", "block-body");
    for (const c of e.children) body.appendChild(renderBlock(c));
    b.appendChild(body);
  }
  return b;
}

// relation chip (`: T`, `:> T`, `:>> T`) — click opens the dependency
// navigator centered on the referenced element (or this one as fallback)
function relChip(op: string, t: string, e: Element_): HTMLElement {
  const r = el("span", "rel rel-link");
  r.append(el("span", "op", op), document.createTextNode(t));
  r.title = "Show dependencies";
  r.tabIndex = 0;
  const go = (ev: Event): void => {
    ev.stopPropagation();
    const res = graph ? resolveRef(graph, t, e.id) : null;
    openDeps(res ? res.id : e.id);
  };
  r.onclick = go;
  r.onkeydown = (ev) => { if (ev.key === "Enter") go(ev); };
  return r;
}

// swap a span for an <input>, commit on Enter / blur, cancel on Escape
function inlineEdit(
  target: HTMLElement, initial: string, commit: (v: string) => void,
): void {
  if (target.querySelector("input")) return;
  const host = target.closest(".block") as HTMLElement | null;
  if (host) host.draggable = false;
  const input = document.createElement("input");
  input.className = "inline-edit";
  input.value = initial;
  input.size = Math.max(6, initial.length + 2);
  const old = target.textContent;
  target.textContent = "";
  target.appendChild(input);
  input.focus();
  input.select();
  let done = false;
  const finish = (save: boolean) => {
    if (done) return;
    done = true;
    const v = input.value.trim();
    target.textContent = old;
    if (host) host.draggable = true;
    if (save && v && v !== initial) commit(v);
  };
  input.onkeydown = (ev) => {
    if (ev.key === "Enter") finish(true);
    if (ev.key === "Escape") finish(false);
    ev.stopPropagation();
  };
  input.onblur = () => finish(true);
  input.onclick = (ev) => ev.stopPropagation();
}

function openPalette(
  anchor: HTMLElement, kinds: string[], pick: (kind: string) => void,
): void {
  document.querySelectorAll(".palette").forEach((p) => p.remove());
  const pal = el("div", "palette");
  for (const k of kinds) {
    const btn = el("button", "", k);
    btn.style.background = blockColor(k);
    btn.onclick = (ev) => {
      ev.stopPropagation();
      pal.remove();
      pick(k);
    };
    pal.appendChild(btn);
  }
  anchor.appendChild(pal);
  const close = (ev: MouseEvent) => {
    if (!pal.contains(ev.target as Node)) {
      pal.remove();
      document.removeEventListener("click", close, true);
    }
  };
  setTimeout(() => document.addEventListener("click", close, true), 0);
}

function promptNameThen(
  kind: string, go: (name: string, extra?: string) => void,
): void {
  const name = prompt("Name for new " + kind);
  if (!name) return;
  if (["part", "attribute", "port", "item", "requirement", "action", "state"]
      .includes(kind)) {
    const extra = prompt(
      "Type / details (optional) — e.g.  MyType   or  : Real = 1.0", "") ?? "";
    go(name.trim(), extra.trim() || undefined);
  } else {
    go(name.trim());
  }
}

// ---------- syntax highlighting (source view) ----------
// mirrors the server lexer/parser token categories (lexer.rs, parser.rs)
const HL_KINDS = new Set([
  "package", "part", "attribute", "port", "item", "action", "state",
  "requirement", "constraint", "connection", "interface", "allocation",
  "analysis", "calc", "case", "concern", "enum", "flow", "metadata",
  "occurrence", "rendering", "verification", "view", "viewpoint", "use",
  "individual", "snapshot", "timeslice", "transition", "exhibit",
  "perform", "satisfy", "verify", "assert", "assume", "require",
  "subject", "actor", "stakeholder", "objective", "return", "bind",
  "def", "import", "connect", "doc", "comment",
  "specializes", "subsets", "redefines", "defined", "by", "to",
]);
const HL_MODIFIERS = new Set([
  "abstract", "variation", "variant", "ref", "in", "out", "inout",
  "readonly", "derived", "end", "private", "protected", "public",
  "nonunique", "ordered", "default", "constant",
]);

// tokenize into styled spans; plain runs stay text nodes, so the fragment's
// textContent always equals the input source exactly
function highlightSysml(src: string): DocumentFragment {
  const frag = document.createDocumentFragment();
  let plain = "";
  const flush = (): void => {
    if (plain) {
      frag.appendChild(document.createTextNode(plain));
      plain = "";
    }
  };
  const tok = (cls: string, text: string): void => {
    flush();
    frag.appendChild(el("span", cls, text));
  };
  const n = src.length;
  let i = 0;
  while (i < n) {
    const c = src[i];
    if (c === "/" && src[i + 1] === "*") {
      const close = src.indexOf("*/", i + 2);
      const j = close < 0 ? n : close + 2;
      tok("tok-com", src.slice(i, j));
      i = j;
    } else if (c === "/" && src[i + 1] === "/") {
      let j = src.indexOf("\n", i);
      if (j < 0) j = n;
      tok("tok-com", src.slice(i, j));
      i = j;
    } else if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < n && src[j] !== c && src[j] !== "\n") {
        if (src[j] === "\\") j++;
        j++;
      }
      if (j < n && src[j] === c) j++;
      tok(c === '"' ? "tok-str" : "tok-name", src.slice(i, j));
      i = j;
    } else if (c >= "0" && c <= "9") {
      let j = i + 1;
      while (j < n) {
        const d = src[j];
        if (d >= "0" && d <= "9") { j++; continue; }
        // decimal point only when followed by a digit (spares `[0..*]`)
        if (d === "." && src[j + 1] >= "0" && src[j + 1] <= "9") { j++; continue; }
        if ((d === "e" || d === "E") && /[0-9+-]/.test(src[j + 1] ?? "")) {
          j += 2;
          continue;
        }
        break;
      }
      tok("tok-num", src.slice(i, j));
      i = j;
    } else if (/[A-Za-z_]/.test(c)) {
      let j = i + 1;
      while (j < n && /[A-Za-z0-9_]/.test(src[j])) j++;
      const w = src.slice(i, j);
      if (HL_KINDS.has(w)) tok("tok-kw", w);
      else if (HL_MODIFIERS.has(w)) tok("tok-mod", w);
      else plain += w;
      i = j;
    } else if (src.startsWith(":>>", i)) {
      tok("tok-op", ":>>");
      i += 3;
    } else if (src.startsWith(":>", i)) {
      tok("tok-op", ":>");
      i += 2;
    } else if (src.startsWith("::", i)) {
      tok("tok-op", "::");
      i += 2;
    } else if (c === ":" || c === "=" || c === "~") {
      tok("tok-op", c);
      i++;
    } else {
      plain += c;
      i++;
    }
  }
  flush();
  return frag;
}

// ---------- dependency graph ----------
// Rebuilt from scratch after every model fetch (ids shift on re-index, so
// nothing here survives an adoptModel — cross-fetch identity is by qual name).
type EdgeKind = "typed" | "spec" | "redef" | "import" | "connect";

interface DepEdge {
  from: string;
  to: string | null; // null = unresolved / external (standard library etc.)
  toRef: string;     // the textual reference, for external cards
  kind: EdgeKind;
  label: string;
  ambiguous: boolean;
}

interface GraphNode {
  id: string;
  e: Element_;
  fileIdx: number;
  filePath: string;
  qual: string | null;   // qualified name (named ancestors :: own name)
  scopeQuals: string[];  // enclosing named scopes, innermost first
}

interface DepGraph {
  nodes: Map<string, GraphNode>;
  byQual: Map<string, string>;
  dupQuals: Set<string>; // qualified names declared more than once
  byName: Map<string, string[]>;
  edges: DepEdge[];
}

let graph: DepGraph | null = null;

// an import's "name" is really its target, not a declared name
function hasOwnName(e: Element_): e is Element_ & { name: string } {
  return e.name !== null && e.kind !== "import";
}

function buildGraph(w: Workspace): DepGraph {
  const g: DepGraph = {
    nodes: new Map(),
    byQual: new Map(),
    dupQuals: new Set(),
    byName: new Map(),
    edges: [],
  };

  // pass 1: nodes + name indexes
  w.files.forEach((f, fi) => {
    const walk = (e: Element_, anc: string[]): void => {
      const named = hasOwnName(e);
      const qual = named ? [...anc, e.name as string].join("::") : null;
      const scopeQuals: string[] = [];
      for (let i = anc.length; i >= 1; i--) scopeQuals.push(anc.slice(0, i).join("::"));
      g.nodes.set(e.id, { id: e.id, e, fileIdx: fi, filePath: f.path, qual, scopeQuals });
      if (qual !== null) {
        if (g.byQual.has(qual)) g.dupQuals.add(qual);
        else g.byQual.set(qual, e.id);
      }
      if (named) {
        const list = g.byName.get(e.name as string);
        if (list) list.push(e.id);
        else g.byName.set(e.name as string, [e.id]);
      }
      const next = named ? [...anc, e.name as string] : anc;
      for (const c of e.children) walk(c, next);
    };
    for (const e of f.elements) walk(e, []);
  });

  // pass 2: edges (deduped)
  const seen = new Set<string>();
  const addEdge = (edge: DepEdge): void => {
    // connections are symmetric: dedupe direction- and label-agnostically so
    // `connect a to b` and a redundant `connect b to a` yield one edge
    const key = edge.kind === "connect"
      ? "connect|" + [edge.from, edge.to ?? "@" + edge.toRef].sort().join("|")
      : edge.from + "|" + (edge.to ?? "@" + edge.toRef) + "|" + edge.kind + "|" + edge.label;
    if (seen.has(key)) return;
    seen.add(key);
    g.edges.push(edge);
  };

  for (const n of g.nodes.values()) {
    const e = n.e;
    if (e.kind === "import") {
      const target = (e.name ?? "").trim();
      if (target) {
        const res = resolveRef(g, target, e.id);
        // the *parent* depends on the imported package (import itself if root)
        const from = parentIdOf(e.id) ?? e.id;
        addEdge({
          from, to: res ? res.id : null, toRef: target, kind: "import",
          label: "import " + target, ambiguous: res ? res.ambiguous : false,
        });
      }
      continue;
    }
    if (e.kind === "connect") {
      const a = e.connect_ends[0] ?? "";
      const b = e.connect_ends[1] ?? "";
      const label = (a || "?") + " <-> " + (b || "?");
      const ra = a ? resolveConnectEnd(g, a, e.id) : null;
      const rb = b ? resolveConnectEnd(g, b, e.id) : null;
      if (ra !== null && rb !== null) {
        addEdge({ from: ra, to: rb, toRef: b, kind: "connect", label, ambiguous: false });
      } else if (ra !== null) {
        addEdge({ from: ra, to: null, toRef: b || "?", kind: "connect", label, ambiguous: false });
      } else if (rb !== null) {
        addEdge({ from: rb, to: null, toRef: a || "?", kind: "connect", label, ambiguous: false });
      }
      continue;
    }
    const rel = (refs: string[], kind: EdgeKind, op: string): void => {
      for (const t of refs) {
        const res = resolveRef(g, t, e.id);
        addEdge({
          from: e.id, to: res ? res.id : null, toRef: t, kind,
          label: op + " " + t, ambiguous: res ? res.ambiguous : false,
        });
      }
    };
    rel(e.typed_by, "typed", ":");
    rel(e.specializes, "spec", ":>");
    rel(e.redefines, "redef", ":>>");
  }
  return g;
}

interface Resolved { id: string; ambiguous: boolean }

// resolve a textual reference from an element's position: enclosing named
// scopes innermost→outward, then absolute, then unique-bare-name fallback
function resolveRef(g: DepGraph, ref: string, fromId: string): Resolved | null {
  let r = ref.trim();
  if (r.startsWith("~")) r = r.slice(1).trim(); // conjugated port type
  r = r.replace(/(::\*\*|::\*|\.\*)$/, "");     // import wildcards
  if (!r) return null;
  const from = g.nodes.get(fromId);
  for (const s of from ? from.scopeQuals : []) {
    const q = s + "::" + r;
    const hit = g.byQual.get(q);
    if (hit !== undefined) return { id: hit, ambiguous: g.dupQuals.has(q) };
  }
  const abs = g.byQual.get(r);
  if (abs !== undefined) return { id: abs, ambiguous: g.dupQuals.has(r) };
  if (!r.includes("::")) {
    const cands = g.byName.get(r) ?? [];
    if (cands.length === 1) return { id: cands[0], ambiguous: false };
    if (cands.length > 1) {
      const ffi = from ? from.fileIdx : -1;
      const same = cands.filter((c) => g.nodes.get(c)?.fileIdx === ffi);
      return { id: same[0] ?? cands[0], ambiguous: true };
    }
  }
  return null;
}

// "battery.powerOut" → resolve "battery" in scope, then follow its type to
// find "powerOut"; returns the deepest element that resolves
function resolveConnectEnd(g: DepGraph, end: string, fromId: string): string | null {
  const segs = end.trim().split(".").map((s) => s.trim()).filter((s) => s.length > 0);
  if (!segs.length) return null;
  const first = resolveRef(g, segs[0], fromId);
  if (!first) return null;
  const start = g.nodes.get(first.id);
  if (!start) return null;
  let cur: GraphNode = start;
  for (let i = 1; i < segs.length; i++) {
    const seg = segs[i];
    let next: Element_ | undefined =
      cur.e.children.find((c) => c.name === seg && c.kind !== "import");
    if (!next) {
      const t = cur.e.typed_by[0];
      if (t) {
        const def = resolveRef(g, t, cur.id);
        const dn = def ? g.nodes.get(def.id) : undefined;
        if (dn) next = dn.e.children.find((c) => c.name === seg && c.kind !== "import");
      }
    }
    const nextNode: GraphNode | undefined = next ? g.nodes.get(next.id) : undefined;
    if (!nextNode) break; // best effort: keep the deepest resolved element
    cur = nextNode;
  }
  return cur.id;
}

interface SideItem {
  otherId: string | null;
  otherRef: string;
  kind: EdgeKind;
  label: string;
  ambiguous: boolean;
}

function dedupItems(items: SideItem[]): SideItem[] {
  const seen = new Set<string>();
  const out: SideItem[] = [];
  for (const it of items) {
    const key = it.kind === "connect"
      ? "connect|" + (it.otherId ?? "@" + it.otherRef)
      : (it.otherId ?? "@" + it.otherRef) + "|" + it.kind + "|" + it.label;
    if (!seen.has(key)) {
      seen.add(key);
      out.push(it);
    }
  }
  return out;
}

// Profiler-style subtree aggregation: the selected element "depends on"
// whatever its subtree references *outside* itself (a part def's parts are
// typed by other defs — those are the def's dependencies), and its
// dependants are outside elements referencing into its subtree. Ids are
// hierarchical paths, so subtree membership is a prefix test.
function inSubtree(rootId: string, id: string | null): boolean {
  return id !== null && (id === rootId || id.startsWith(rootId + "."));
}

// "battery : BatteryPack" instead of ": BatteryPack" when the edge comes
// from a descendant of the centered element
function originPrefix(g: DepGraph, rootId: string, fromId: string): string {
  if (fromId === rootId) return "";
  const n = g.nodes.get(fromId);
  const nm = n ? n.e.name ?? "‹" + n.e.kind + "›" : "";
  return nm ? nm + " " : "";
}

// edges leaving the subtree; connections (symmetric) show here for both
// endpoints, so they never appear on the dependants side
function dependenciesOf(g: DepGraph, id: string): SideItem[] {
  const items: SideItem[] = [];
  for (const e of g.edges) {
    const fromIn = inSubtree(id, e.from);
    if (e.kind === "connect") {
      const toIn = inSubtree(id, e.to);
      if (fromIn && !toIn) {
        items.push({ otherId: e.to, otherRef: e.toRef, kind: e.kind, label: e.label, ambiguous: e.ambiguous });
      } else if (toIn && !fromIn) {
        items.push({ otherId: e.from, otherRef: "", kind: e.kind, label: e.label, ambiguous: e.ambiguous });
      }
      continue;
    }
    if (!fromIn || inSubtree(id, e.to)) continue;
    items.push({
      otherId: e.to, otherRef: e.toRef, kind: e.kind,
      label: originPrefix(g, id, e.from) + e.label, ambiguous: e.ambiguous,
    });
  }
  return dedupItems(items);
}

// edges entering the subtree from outside
function dependantsOf(g: DepGraph, id: string): SideItem[] {
  const items: SideItem[] = [];
  for (const e of g.edges) {
    if (e.kind === "connect") continue; // symmetric — shown as dependencies
    if (!inSubtree(id, e.to) || inSubtree(id, e.from)) continue;
    items.push({ otherId: e.from, otherRef: "", kind: e.kind, label: e.label, ambiguous: e.ambiguous });
  }
  return dedupItems(items);
}

// ---------- deps view (dependency navigator) ----------
interface TrailEntry { id: string; qual: string | null; label: string }

let depCenter: string | null = null;
let depTrail: TrailEntry[] = []; // visited elements; last entry = current center
const depFilters: Record<EdgeKind, boolean> = {
  typed: true, spec: true, redef: true, connect: true, import: true,
};

const EDGE_GROUPS: [EdgeKind, string][] = [
  ["typed", "types"], ["spec", "specializes"], ["redef", "redefines"],
  ["connect", "connections"], ["import", "imports"],
];
const FILTER_LABELS: [EdgeKind, string][] = [
  ["typed", "types (:)"], ["spec", "specializes (:>)"], ["redef", "redefines (:>>)"],
  ["connect", "connections"], ["import", "imports"],
];

function wireColor(kind: EdgeKind): string {
  if (kind === "typed") return "var(--c-partdef)";
  if (kind === "spec" || kind === "redef") return "var(--c-behavior)";
  if (kind === "connect") return "var(--c-conn)";
  return "var(--c-import)";
}

function edgeChipText(item: SideItem): string {
  if (item.kind === "import") return "import";
  if (item.kind === "connect") return "connect " + item.label;
  return item.label;
}

// entry point from blocks view / search — starts a fresh breadcrumb trail
function openDeps(id: string): void {
  if (viewMode !== "deps") depTrail = [];
  centerDeps(id);
}

function centerDeps(id: string): void {
  const n = graph ? graph.nodes.get(id) : undefined;
  if (!n) {
    showToast("Element not found in model", true);
    return;
  }
  viewMode = "deps";
  depCenter = id;
  const last = depTrail.length ? depTrail[depTrail.length - 1] : undefined;
  if (!last || last.id !== id) {
    depTrail.push({ id, qual: n.qual, label: n.e.name ?? "‹" + n.e.kind + "›" });
  }
  if (depTrail.length > 32) depTrail = depTrail.slice(-32);
  render();
}

// after a refetch ids can shift: re-resolve the trail by qualified name,
// falling back to the raw id (unnamed elements — doc/connect/raw — have no
// qual but usually keep their id; a rename keeps the id too)
function remapDeps(): void {
  const g = graph;
  if (!g) return;
  const relocate = (t: TrailEntry): string | undefined =>
    (t.qual !== null ? g.byQual.get(t.qual) : undefined) ??
    (g.nodes.has(t.id) ? t.id : undefined);
  const cur = depTrail.length ? depTrail[depTrail.length - 1] : undefined;
  const newCenter = cur ? relocate(cur) : undefined;
  if (newCenter === undefined) {
    viewMode = "blocks";
    depCenter = null;
    depTrail = [];
    showToast("Centered element is gone — back to blocks view", true);
    return;
  }
  const next: TrailEntry[] = [];
  for (const t of depTrail) {
    const nid = relocate(t);
    if (nid === undefined) continue;
    const n = g.nodes.get(nid);
    const label = n ? n.e.name ?? "‹" + n.e.kind + "›" : t.label;
    next.push({ id: nid, qual: n ? n.qual : t.qual, label });
  }
  depTrail = next;
  depCenter = newCenter;
}

// switch to blocks view, scroll the element into view and flash it
function revealInBlocks(id: string): void {
  const n = graph ? graph.nodes.get(id) : undefined;
  if (!n) return;
  viewMode = "blocks";
  activeFile = n.fileIdx;
  render();
  window.requestAnimationFrame(() => {
    const blk = canvas.querySelector('.block[data-id="' + id + '"]');
    if (!(blk instanceof HTMLElement)) return;
    blk.scrollIntoView({ block: "center" });
    blk.classList.add("flash");
    window.setTimeout(() => blk.classList.remove("flash"), 2100);
  });
}

// SVG connector overlay: side card → center card bezier per visible card
const SVG_NS = "http://www.w3.org/2000/svg";

interface WireCard { elem: HTMLElement; col: HTMLElement; side: "left" | "right"; kind: EdgeKind }
interface WireState { grid: HTMLElement; svg: SVGSVGElement; center: HTMLElement; cards: WireCard[] }

let wireState: WireState | null = null;
let wireRaf = 0;

function scheduleWires(): void {
  if (wireRaf) return;
  wireRaf = window.requestAnimationFrame(() => {
    wireRaf = 0;
    drawWires();
  });
}

function drawWires(): void {
  const st = wireState;
  if (!st || !st.grid.isConnected) return;
  while (st.svg.firstChild) st.svg.removeChild(st.svg.firstChild);
  const gr = st.grid.getBoundingClientRect();
  const cr = st.center.getBoundingClientRect();
  const cy = cr.top + cr.height / 2 - gr.top;
  for (const c of st.cards) {
    const colR = c.col.getBoundingClientRect();
    const r = c.elem.getBoundingClientRect();
    if (r.bottom < colR.top || r.top > colR.bottom) continue; // scrolled away
    const yCard = r.top + r.height / 2 - gr.top;
    let sx: number, sy: number, ex: number, ey: number;
    if (c.side === "left") {
      sx = r.right - gr.left; sy = yCard;
      ex = cr.left - gr.left; ey = cy;
    } else {
      sx = cr.right - gr.left; sy = cy;
      ex = r.left - gr.left; ey = yCard;
    }
    const mx = (sx + ex) / 2;
    const p = document.createElementNS(SVG_NS, "path");
    p.setAttribute("d", "M " + sx + " " + sy + " C " + mx + " " + sy + ", " + mx + " " + ey + ", " + ex + " " + ey);
    p.setAttribute("fill", "none");
    p.setAttribute("stroke", wireColor(c.kind));
    p.setAttribute("stroke-width", "1.5");
    p.setAttribute("opacity", "0.55");
    st.svg.appendChild(p);
  }
}

function renderDepsView(): void {
  const g = graph;
  const center = g && depCenter !== null ? g.nodes.get(depCenter) : undefined;
  if (!g || !center) {
    viewMode = "blocks";
    render();
    return;
  }

  // header: back button, breadcrumbs, edge-kind filters
  const head = el("div", "deps-head");
  const back = el("button", "deps-back", "← Blocks");
  back.onclick = () => {
    viewMode = "blocks";
    render();
  };
  head.appendChild(back);

  const crumbs = el("div", "deps-crumbs");
  const shown = depTrail.slice(-8);
  shown.forEach((t, i) => {
    if (i > 0) crumbs.appendChild(el("span", "crumb-sep", "›"));
    const isCurrent = i === shown.length - 1;
    const c = el("button", "crumb" + (isCurrent ? " current" : ""), t.label);
    if (!isCurrent) {
      c.onclick = () => {
        const real = depTrail.length - shown.length + i;
        depTrail = depTrail.slice(0, real + 1);
        depCenter = depTrail[depTrail.length - 1].id;
        render();
      };
    }
    crumbs.appendChild(c);
  });
  head.appendChild(crumbs);

  const filters = el("div", "deps-filters");
  for (const [k, label] of FILTER_LABELS) {
    const lab = el("label", "deps-filter");
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = depFilters[k];
    cb.onchange = () => {
      depFilters[k] = cb.checked;
      render();
    };
    lab.append(cb, document.createTextNode(label));
    filters.appendChild(lab);
  }
  head.appendChild(filters);
  canvas.appendChild(head);

  // three columns + wire overlay
  const grid = el("div", "deps-grid");
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("class", "deps-wires");
  svg.setAttribute("aria-hidden", "true");
  const leftCol = el("div", "deps-col deps-side");
  const centerCol = el("div", "deps-col deps-center");
  const rightCol = el("div", "deps-col deps-side");

  const dependants = dependantsOf(g, center.id);
  const dependencies = dependenciesOf(g, center.id);

  const e = center.e;
  const card = el("div", "dep-center-card");
  const chead = el("div", "dep-center-head");
  chead.style.background = blockColor(e.kind);
  chead.appendChild(el("span", "kind", e.kind));
  chead.appendChild(el("span", "dep-center-name", e.name ?? "‹unnamed›"));
  if (e.short_name) chead.appendChild(el("span", "shortname", "<" + e.short_name + ">"));
  card.appendChild(chead);
  const cbody = el("div", "dep-center-body");
  if (center.qual !== null) cbody.appendChild(el("div", "dep-qual", center.qual));
  cbody.appendChild(el("div", "dep-file", center.filePath));
  cbody.appendChild(el("div", "dep-counts",
    dependants.length + " dependant" + (dependants.length === 1 ? "" : "s") + " · " +
    dependencies.length + (dependencies.length === 1 ? " dependency" : " dependencies")));
  const show = el("button", "dep-show-btn", "Show in blocks");
  show.onclick = () => revealInBlocks(center.id);
  cbody.appendChild(show);
  card.appendChild(cbody);
  centerCol.appendChild(card);

  const cards: WireCard[] = [];
  fillDepsColumn(leftCol, dependants, "left", g, "no dependants", cards);
  fillDepsColumn(rightCol, dependencies, "right", g, "no dependencies", cards);

  grid.append(leftCol, centerCol, rightCol, svg);
  canvas.appendChild(grid);

  wireState = { grid, svg, center: card, cards };
  leftCol.addEventListener("scroll", scheduleWires);
  rightCol.addEventListener("scroll", scheduleWires);
  centerCol.addEventListener("scroll", scheduleWires);
  window.requestAnimationFrame(drawWires);
}

function fillDepsColumn(
  col: HTMLElement, items: SideItem[], side: "left" | "right",
  g: DepGraph, hint: string, cards: WireCard[],
): void {
  const visible = items.filter((it) => depFilters[it.kind]);
  if (!visible.length) {
    col.appendChild(el("div", "deps-hint", hint));
    return;
  }
  const kindCount = new Set(visible.map((it) => it.kind)).size;
  for (const [k, groupName] of EDGE_GROUPS) {
    const group = visible.filter((it) => it.kind === k);
    if (!group.length) continue;
    if (kindCount > 1) col.appendChild(el("div", "deps-group-head", groupName));
    for (const it of group) {
      const cardEl = depSideCard(it, g);
      col.appendChild(cardEl);
      cards.push({ elem: cardEl, col, side, kind: it.kind });
    }
  }
}

function depSideCard(item: SideItem, g: DepGraph): HTMLElement {
  const n = item.otherId !== null ? g.nodes.get(item.otherId) : undefined;
  if (!n) {
    // unresolved reference — grey, dashed, not clickable
    const c = el("div", "dep-card external");
    const top = el("div", "dep-card-top");
    top.appendChild(el("span", "dep-edge-chip", edgeChipText(item)));
    top.appendChild(el("span", "dep-ext-tag", "external"));
    c.appendChild(top);
    c.appendChild(el("div", "dep-name", item.otherRef || item.label));
    return c;
  }
  const c = el("div", "dep-card");
  c.style.setProperty("--bc", blockColor(n.e.kind));
  c.tabIndex = 0;
  const top = el("div", "dep-card-top");
  const chip = el("span", "dep-edge-chip", edgeChipText(item));
  if (item.ambiguous) chip.title = "Ambiguous reference — resolved to best match";
  top.appendChild(chip);
  if (item.ambiguous) top.appendChild(el("span", "dep-ext-tag", "ambiguous"));
  c.appendChild(top);
  const main = el("div", "dep-card-main");
  main.appendChild(el("span", "dep-kind", n.e.kind));
  main.appendChild(el("span", "dep-name", n.e.name ?? "‹unnamed›"));
  c.appendChild(main);
  c.appendChild(el("div", "dep-sub", (n.qual !== null ? n.qual + " · " : "") + n.filePath));
  const go = (): void => centerDeps(n.id);
  c.onclick = go;
  c.onkeydown = (ev) => { if (ev.key === "Enter") go(); };
  return c;
}

// ---------- search ----------
const searchWrap = document.getElementById("search-wrap")!;
const searchInput = document.getElementById("search-input") as HTMLInputElement;
const searchPanel = document.getElementById("search-panel")!;
const searchRegexBtn = document.getElementById("search-regex") as HTMLButtonElement;
const searchCaseBtn = document.getElementById("search-case") as HTMLButtonElement;
const searchFieldsBtn = document.getElementById("search-fields-btn") as HTMLButtonElement;
const searchFieldsMenu = document.getElementById("search-fields-menu")!;

interface SearchHit {
  id: string;
  fileIdx: number;
  filePath: string;
  e: Element_;
  field: string;
  text: string;
  start: number;
  end: number;
}

let searchHits: SearchHit[] = [];
let searchSel = -1;
let searchRegex = false;
let searchCase = false;
let searchTimer = 0;
const SEARCH_ROW_CAP = 200;

const searchFields = { name: true, kind: true, refs: true, value: true, doc: true, raw: false };
const SEARCH_FIELD_DEFS: [keyof typeof searchFields, string][] = [
  ["name", "name"], ["kind", "kind"], ["refs", "refs"],
  ["value", "value"], ["doc", "doc text"], ["raw", "raw"],
];

function collectSearchFields(e: Element_): [string, string][] {
  const out: [string, string][] = [];
  if (searchFields.name) {
    if (e.name !== null && e.kind !== "import") out.push(["name", e.name]);
    if (e.short_name !== null) out.push(["short name", e.short_name]);
  }
  if (searchFields.kind) out.push(["kind", e.kind]);
  if (searchFields.refs) {
    for (const t of e.typed_by) out.push(["typed by", t]);
    for (const t of e.specializes) out.push(["specializes", t]);
    for (const t of e.redefines) out.push(["redefines", t]);
    for (const t of e.connect_ends) out.push(["connect end", t]);
    if (e.kind === "import" && e.name !== null) out.push(["import", e.name]);
  }
  if (searchFields.value && e.value !== null) out.push(["value", e.value]);
  if (searchFields.doc && e.text !== null) out.push(["doc", e.text]);
  if (searchFields.raw && e.raw !== null) out.push(["raw", e.raw]);
  return out;
}

function hideSearchPanel(): void {
  searchPanel.hidden = true;
  searchPanel.innerHTML = "";
  searchHits = [];
  searchSel = -1;
}

function hideFieldsMenu(): void {
  searchFieldsMenu.hidden = true;
  searchFieldsBtn.setAttribute("aria-expanded", "false");
}

function renderSearchError(msg: string): void {
  searchPanel.innerHTML = "";
  searchPanel.hidden = false;
  searchHits = [];
  searchSel = -1;
  searchPanel.appendChild(el("div", "search-error", "Invalid regex: " + msg));
}

function runSearch(): void {
  const q = searchInput.value;
  searchInput.classList.remove("invalid");
  if (!q.trim() || !ws) {
    hideSearchPanel();
    return;
  }
  let matcher: (s: string) => { start: number; end: number } | null;
  if (searchRegex) {
    let re: RegExp;
    try {
      re = new RegExp(q, searchCase ? "u" : "ui");
    } catch (err) {
      searchInput.classList.add("invalid");
      renderSearchError(err instanceof Error ? err.message : String(err));
      return;
    }
    matcher = (s) => {
      const m = re.exec(s);
      if (!m || m[0].length === 0) return null;
      return { start: m.index, end: m.index + m[0].length };
    };
  } else {
    const needle = searchCase ? q : q.toLowerCase();
    matcher = (s) => {
      const hay = searchCase ? s : s.toLowerCase();
      const i = hay.indexOf(needle);
      return i < 0 ? null : { start: i, end: i + needle.length };
    };
  }
  const hits: SearchHit[] = [];
  ws.files.forEach((f, fi) => {
    const walk = (e: Element_): void => {
      for (const [field, text] of collectSearchFields(e)) {
        const m = matcher(text);
        if (m) hits.push({ id: e.id, fileIdx: fi, filePath: f.path, e, field, text, start: m.start, end: m.end });
      }
      for (const c of e.children) walk(c);
    };
    for (const e of f.elements) walk(e);
  });
  searchHits = hits;
  searchSel = hits.length ? 0 : -1;
  renderSearchPanel();
}

function renderSearchPanel(): void {
  searchPanel.innerHTML = "";
  searchPanel.hidden = false;
  searchPanel.appendChild(el("div", "search-count",
    searchHits.length + (searchHits.length === 1 ? " match" : " matches")));
  let rendered = 0;
  let curFile = -1;
  for (let i = 0; i < searchHits.length && rendered < SEARCH_ROW_CAP; i++) {
    const h = searchHits[i];
    if (h.fileIdx !== curFile) {
      curFile = h.fileIdx;
      searchPanel.appendChild(el("div", "search-file", h.filePath));
    }
    searchPanel.appendChild(searchRow(h, i));
    rendered++;
  }
  if (searchHits.length > SEARCH_ROW_CAP) {
    searchPanel.appendChild(el("div", "search-more",
      "… and " + (searchHits.length - SEARCH_ROW_CAP) + " more"));
  }
  if (!searchHits.length) searchPanel.appendChild(el("div", "search-empty", "No matches"));
  updateSearchSel();
}

function searchRow(h: SearchHit, idx: number): HTMLElement {
  const row = el("div", "search-row");
  row.setAttribute("role", "option");
  row.setAttribute("aria-selected", "false");
  row.dataset.idx = String(idx);
  const chip = el("span", "search-kind", h.e.kind);
  chip.style.background = blockColor(h.e.kind);
  const nm = h.e.name ?? (h.e.raw !== null ? (h.e.raw.trim().split("\n")[0] || "‹unnamed›") : "‹unnamed›");
  row.append(chip, el("span", "search-name", nm),
    el("span", "search-fieldname", h.field), markedText(h));
  const dep = el("button", "search-dep", "⇄");
  dep.title = "Dependencies";
  dep.onclick = (ev) => {
    ev.stopPropagation();
    hideSearchPanel();
    openDeps(h.id);
  };
  row.appendChild(dep);
  row.onclick = () => activateHit(h);
  // mousemove, not mouseenter: panel scrolls during keyboard navigation
  // emit synthetic enter events under a stationary cursor
  row.onmousemove = () => {
    if (searchSel !== idx) {
      searchSel = idx;
      updateSearchSel();
    }
  };
  return row;
}

// matched text with the matching substring in <mark> — built from split
// text nodes, never innerHTML
function markedText(h: SearchHit): HTMLElement {
  const s = el("span", "search-text");
  const from = Math.max(0, h.start - 24);
  const to = Math.min(h.text.length, h.end + 60);
  if (from > 0) s.appendChild(document.createTextNode("…"));
  s.appendChild(document.createTextNode(h.text.slice(from, h.start)));
  const mk = document.createElement("mark");
  mk.textContent = h.text.slice(h.start, h.end);
  s.appendChild(mk);
  s.appendChild(document.createTextNode(h.text.slice(h.end, to)));
  if (to < h.text.length) s.appendChild(document.createTextNode("…"));
  return s;
}

function activateHit(h: SearchHit): void {
  hideSearchPanel();
  revealInBlocks(h.id);
}

function updateSearchSel(): void {
  const rows = searchPanel.querySelectorAll<HTMLElement>(".search-row");
  rows.forEach((r) => {
    const sel = Number(r.dataset.idx) === searchSel;
    r.classList.toggle("selected", sel);
    r.setAttribute("aria-selected", String(sel));
    if (sel) r.scrollIntoView({ block: "nearest" });
  });
}

function moveSearchSel(delta: number): void {
  const max = Math.min(searchHits.length, SEARCH_ROW_CAP) - 1;
  if (max < 0) return;
  searchSel = searchSel < 0 ? 0 : Math.min(max, Math.max(0, searchSel + delta));
  updateSearchSel();
}

// ---------- export to PDF ----------
// server contract (built separately): GET /api/export?scope=element|file|project
//   [&id=…][&file=…][&format=doc|blocks] → application/pdf attachment
let exportDialog: HTMLElement | null = null;
let exportOutsideClose: ((ev: MouseEvent) => void) | null = null;

function closeExportDialog(): void {
  if (exportOutsideClose) {
    document.removeEventListener("click", exportOutsideClose, true);
    exportOutsideClose = null;
  }
  if (exportDialog) {
    exportDialog.remove();
    exportDialog = null;
  }
}

function exportRadio(
  name: string, value: string, label: string, checked: boolean, disabled: boolean,
): HTMLLabelElement {
  const lab = el("label", "export-radio" + (disabled ? " disabled" : ""));
  const r = document.createElement("input");
  r.type = "radio";
  r.name = name;
  r.value = value;
  r.checked = checked;
  r.disabled = disabled;
  lab.append(r, document.createTextNode(label));
  return lab;
}

function openExportDialog(anchor: HTMLElement, selected?: Element_): void {
  closeExportDialog();
  const file = ws ? ws.files[activeFile] : undefined;
  const d = el("div", "export-dialog");
  d.appendChild(el("div", "export-title", "Export PDF"));

  const scopeBox = el("div", "export-group");
  scopeBox.appendChild(el("div", "export-label", "Scope"));
  scopeBox.appendChild(exportRadio("exp-scope", "project", "Whole project", !selected, false));
  scopeBox.appendChild(exportRadio("exp-scope", "file",
    "Current file (" + (file ? file.path : "none") + ")", false, !file));
  const selLabel = selected
    ? "Selected element — " + (selected.kind + " " + (selected.name ?? "‹unnamed›")).trim()
    : "Selected element";
  scopeBox.appendChild(exportRadio("exp-scope", "element", selLabel, !!selected, !selected));
  d.appendChild(scopeBox);

  const fmtBox = el("div", "export-group");
  fmtBox.appendChild(el("div", "export-label", "Format"));
  fmtBox.appendChild(exportRadio("exp-format", "doc", "Document", true, false));
  fmtBox.appendChild(exportRadio("exp-format", "blocks", "Visual blocks", false, false));
  d.appendChild(fmtBox);

  const go = el("button", "export-go", "Download PDF");
  go.onclick = () => {
    const scopeInput = d.querySelector('input[name="exp-scope"]:checked');
    const fmtInput = d.querySelector('input[name="exp-format"]:checked');
    const scope = scopeInput instanceof HTMLInputElement ? scopeInput.value : "project";
    const format = fmtInput instanceof HTMLInputElement ? fmtInput.value : "doc";
    let url = "/api/export?scope=" + encodeURIComponent(scope) +
      "&format=" + encodeURIComponent(format);
    if (scope === "element" && selected) url += "&id=" + encodeURIComponent(selected.id);
    else if (scope === "file" && file) url += "&file=" + encodeURIComponent(file.path);
    closeExportDialog();
    const a = document.createElement("a");
    a.href = url;
    document.body.appendChild(a);
    a.click();
    a.remove();
  };
  d.appendChild(go);

  document.body.appendChild(d);
  const r = anchor.getBoundingClientRect();
  const dw = d.offsetWidth;
  const dh = d.offsetHeight;
  const x = Math.max(10, Math.min(r.left, window.innerWidth - dw - 10));
  let y = r.bottom + 8;
  if (y + dh > window.innerHeight - 10) y = Math.max(10, r.top - dh - 8);
  d.style.left = x + "px";
  d.style.top = y + "px";

  exportDialog = d;
  const closeFn = (ev: MouseEvent): void => {
    if (exportDialog && !exportDialog.contains(ev.target as Node)) closeExportDialog();
  };
  exportOutsideClose = closeFn;
  window.setTimeout(() => {
    if (exportOutsideClose === closeFn) document.addEventListener("click", closeFn, true);
  }, 0);
}

// ---------- wiring ----------
document.getElementById("refresh")!.onclick = () => {
  fetchModel().then(() => showToast("Re-indexed"));
};
document.getElementById("new-file")!.onclick = () => {
  const p = prompt("New file path (relative, .sysml)", "NewModel.sysml");
  if (p) applyEdit({ op: "new_file", path: p.trim() });
};
window.addEventListener("focus", () => { void fetchModel(); });

canvas.addEventListener("dragover", (ev) => {
  if (!dragId || ev.target !== canvas) return;
  ev.preventDefault();
  if (ev.dataTransfer) ev.dataTransfer.dropEffect = "move";
});
canvas.addEventListener("drop", (ev) => {
  if (!dragId || ev.target !== canvas || !ws) return;
  ev.preventDefault();
  const f = ws.files[activeFile];
  const id = dragId;
  dragId = null;
  clearDropMarks();
  void applyEdit({
    op: "move", id, new_parent: null, file: f.path, index: f.elements.length,
  });
});

// search wiring
searchInput.addEventListener("input", () => {
  window.clearTimeout(searchTimer);
  searchTimer = window.setTimeout(() => {
    searchTimer = 0;
    runSearch();
  }, 80);
});
// run a pending debounced search now, so Enter/arrows right after typing
// act on the current query, not the previous one
function flushSearch(): void {
  if (searchTimer) {
    window.clearTimeout(searchTimer);
    searchTimer = 0;
    runSearch();
  }
}
searchInput.addEventListener("keydown", (ev) => {
  if (ev.key === "ArrowDown") {
    ev.preventDefault();
    flushSearch();
    moveSearchSel(1);
  } else if (ev.key === "ArrowUp") {
    ev.preventDefault();
    flushSearch();
    moveSearchSel(-1);
  } else if (ev.key === "Enter") {
    flushSearch();
    const h = searchSel >= 0 ? searchHits[searchSel] : undefined;
    if (h) activateHit(h);
  } else if (ev.key === "Escape") {
    ev.stopPropagation();
    hideSearchPanel();
    hideFieldsMenu();
    searchInput.blur();
  }
});
searchInput.addEventListener("focus", () => {
  if (searchInput.value.trim() && searchPanel.hidden) runSearch();
});
searchRegexBtn.onclick = () => {
  searchRegex = !searchRegex;
  searchRegexBtn.classList.toggle("on", searchRegex);
  searchRegexBtn.setAttribute("aria-pressed", String(searchRegex));
  if (searchInput.value.trim()) runSearch();
};
searchCaseBtn.onclick = () => {
  searchCase = !searchCase;
  searchCaseBtn.classList.toggle("on", searchCase);
  searchCaseBtn.setAttribute("aria-pressed", String(searchCase));
  if (searchInput.value.trim()) runSearch();
};
searchFieldsBtn.onclick = () => {
  const open = searchFieldsMenu.hidden;
  searchFieldsMenu.hidden = !open;
  searchFieldsBtn.setAttribute("aria-expanded", String(open));
};
for (const [key, label] of SEARCH_FIELD_DEFS) {
  const lab = el("label", "search-field-opt");
  const cb = document.createElement("input");
  cb.type = "checkbox";
  cb.checked = searchFields[key];
  cb.onchange = () => {
    searchFields[key] = cb.checked;
    if (searchInput.value.trim()) runSearch();
  };
  lab.append(cb, document.createTextNode(label));
  searchFieldsMenu.appendChild(lab);
}

// export button
const exportBtn = document.getElementById("export-pdf") as HTMLButtonElement;
exportBtn.onclick = () => openExportDialog(exportBtn);

// document-level listeners: attached exactly once (render() rebuilds the
// canvas DOM, so anything per-element lives and dies with its node)
document.addEventListener("click", (ev) => {
  if (!searchWrap.contains(ev.target as Node)) {
    if (!searchPanel.hidden) hideSearchPanel();
    if (!searchFieldsMenu.hidden) hideFieldsMenu();
  }
});
document.addEventListener("keydown", (ev) => {
  if (ev.key === "Escape") {
    if (exportDialog) {
      closeExportDialog();
      return;
    }
    if (!searchPanel.hidden || !searchFieldsMenu.hidden) {
      hideSearchPanel();
      hideFieldsMenu();
      return;
    }
    if (viewMode === "deps") {
      viewMode = "blocks";
      render();
    }
    return;
  }
  if (ev.key === "/" && !ev.ctrlKey && !ev.metaKey && !ev.altKey) {
    const t = ev.target instanceof HTMLElement ? ev.target : null;
    const tag = t ? t.tagName : "";
    if (tag !== "INPUT" && tag !== "TEXTAREA" && !(t && t.isContentEditable)) {
      ev.preventDefault();
      searchInput.focus();
      searchInput.select();
    }
  }
});
window.addEventListener("resize", () => {
  if (viewMode === "deps") scheduleWires();
});

void fetchModel();
