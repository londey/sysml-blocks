# sysml-blocks

A Scratch-style visual editor for **SysML v2 textual models**. Run it as a
Docker container with your model folder mapped in; it parses and indexes every
`.sysml` file into a runtime model, serves a local web UI that renders the
model as colored nested blocks, and writes edits back to the files as
**surgical text splices** — untouched lines keep their exact formatting, so
diffs stay reviewable in a git PR workflow.

```
┌────────────────────────────────────────────────┐
│ browser  ── blocks UI (TypeScript, no framework)│
└──────────────▲─────────────────────────────────┘
               │ JSON: GET /api/model, POST /api/edit
┌──────────────┴─────────────────────────────────┐
│ Rust server (tiny_http)                         │
│  lexer → tolerant parser → span-annotated tree  │
│  edits = byte-span splices → write → reparse    │
└──────────────▲─────────────────────────────────┘
               │ mapped volume
        /models/**.sysml   (git worktree / OneDrive)
```

## Quick start

```bash
docker compose up --build
# or, against your own models:
docker build -t sysml-blocks .
docker run --rm -p 8080:8080 -v /path/to/your/models:/models sysml-blocks
```

Open http://localhost:8080. Local dev without Docker:

```bash
(cd web && npm install && npm run build)
(cd server && cargo build --release)
MODELS_DIR=examples WEB_ROOT=web/dist server/target/release/sysml-blocks-server
```

## Using the UI

- Files in the sidebar; each file renders as a stack of blocks colored by
  construct family (structure blue/purple, data green, ports orange,
  connections teal, behavior gold, requirements red).
- **Click a name** to rename; **click a `= value` chip** to edit a value.
- **Hover a block** for `＋` (add child, from a per-kind palette) and `✕`
  (delete).
- **Drag a block** to rearrange: drop on the top/bottom quarter of a sibling
  to insert before/after it, on the middle of a container to nest inside it,
  or on empty canvas to move it to the file root. The moved text is
  re-indented for its destination; nested bodies come along verbatim.
- **Text** toggles a read-only raw-source view of the file.
- Anything the parser doesn't model yet is preserved verbatim and shown as a
  gray dashed **unparsed** block — it round-trips untouched.
- The model re-indexes on every fetch and on window focus, so external edits
  (git pull, OneDrive sync, your `$EDITOR`) show up automatically.

## Edit semantics (why diffs stay small)

The parser records byte spans for each element, its name, its value, and its
body. Edits are applied by splicing exactly those spans:

- `rename` — replaces just the name token.
- `set_value` — replaces the value expression, or inserts ` = expr` before the
  terminating `;` when there wasn't one.
- `add_child` — inserts an indented statement before the closing brace; a
  bodiless `part def X;` is converted to `part def X { ... }`.
- `delete` — removes the element's span plus its line remainder.
- `move` — extracts the element's lines, re-indents them for the destination,
  and applies removal + insertion as offset-sorted splices computed on one
  snapshot (works across files too; refuses to move an element into itself).
- `set_raw` (API only) — replaces an element's full source text.

Change approval stays external: point the volume at a git worktree and review
the resulting diffs as PRs, or at a synced OneDrive/SharePoint folder.

## API

| Route | Description |
|---|---|
| `GET /api/model` | Full parsed workspace (re-scans the volume) |
| `GET /api/source?file=rel/path.sysml` | Raw text of one file |
| `POST /api/edit` | One edit op (JSON, tagged by `"op"`), returns new model |

Edit ops: `rename`, `set_value`, `add_child`, `add_root`, `delete`,
`move`, `set_raw`, `new_file` — see `server/src/model.rs`.

## Parser coverage (first pass)

Understood: packages, imports, `doc`/comments, defs and usages for part /
attribute / port / item / action / state / requirement / constraint /
connection / interface / enum / use case and friends; modifiers (`abstract`,
`ref`, `in/out/inout`, `end`, visibility, ...); short names `<REQ1>`; typing
`: T` (incl. conjugated `~T`), specialization `:>`, redefinition `:>>`,
`defined by` / `specializes` / `subsets` keywords; multiplicity `[0..*]`;
values `= expr`; `connect a.x to b.y`.

Not yet modeled (preserved as raw): expression bodies of constraints,
`interface` bodies with `end` bindings beyond simple usages, `flow` payload
syntax, metadata annotations, alias, filtering imports.

## Layout

```
server/   Rust: lexer.rs, parser.rs (span-annotated tolerant parser),
          model.rs (workspace + splice edit engine), main.rs (HTTP)
web/      TypeScript UI, esbuild bundle, no framework
examples/ Multidisciplinary drone model used by docker-compose
```

## Known limitations

- IDs are positional (`f0.1.2`) and refresh after every edit; the UI always
  works from the latest snapshot it fetched.
- Single-writer assumption per instance; last write wins if the volume changes
  between fetch and edit (stale spans are rejected with 409 when detectable).
- No cross-file name resolution / semantic validation yet — it's a structural
  editor, not a full KerML semantic engine.
