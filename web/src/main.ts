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

let ws: Workspace | null = null;
let activeFile = 0;
let showSource = false;

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
async function fetchModel(): Promise<void> {
  const r = await fetch("/api/model");
  ws = await r.json();
  render();
}

async function applyEdit(op: EditOp): Promise<void> {
  const r = await fetch("/api/edit", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(op),
  });
  if (r.ok) {
    ws = await r.json();
    render();
    showToast("Saved to " + (ws!.files[activeFile]?.path ?? "file"));
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
    li.onclick = () => { activeFile = i; showSource = false; render(); };
    fileList.appendChild(li);
  });

  canvas.innerHTML = "";
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
  toggle.textContent = showSource ? "Blocks" : "Text";
  toggle.onclick = async () => {
    showSource = !showSource;
    render();
  };
  title.append(tspan, toggle);
  canvas.appendChild(title);

  if (showSource) {
    const pre = document.createElement("pre");
    pre.className = "source-view";
    pre.textContent = "loading…";
    canvas.appendChild(pre);
    fetch("/api/source?file=" + encodeURIComponent(file.path))
      .then((r) => r.text())
      .then((t) => (pre.textContent = t));
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

    for (const t of e.typed_by) {
      const r = el("span", "rel");
      r.append(el("span", "op", ":"), document.createTextNode(t));
      head.appendChild(r);
    }
    for (const t of e.specializes) {
      const r = el("span", "rel");
      r.append(el("span", "op", ":>"), document.createTextNode(t));
      head.appendChild(r);
    }
    for (const t of e.redefines) {
      const r = el("span", "rel");
      r.append(el("span", "op", ":>>"), document.createTextNode(t));
      head.appendChild(r);
    }
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

void fetchModel();
