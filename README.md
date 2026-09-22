# ipython-notebook-zed

Notebook-aware editing for Jupyter/IPython `.ipynb` files in [Zed](https://zed.dev),
plus a jupytext-style pairing workflow that turns notebooks into `%`-cell python
scripts you can execute with Zed's built-in REPL — a Colab-like cell-execution
experience without leaving the editor.

## Why it exists

Zed's extension API cannot (yet) render notebooks Colab-style: there is no
custom editor API, no webview, and no UI surface for extensions. Zed's own
notebook editor exists behind a feature flag. This extension delivers what the
API *can* support today, in two pillars:

1. **Notebook-aware raw editing** — a dedicated tree-sitter grammar for
   `.ipynb` so the raw JSON becomes readable: cells are first-class syntax
   (folding, outline navigation), python code and markdown inside the JSON get
   real syntax highlighting, and structural validity is checked by a
   companion language server.
2. **Colab-like execution** — one keystroke pairs the notebook with a sibling
   `foo.nb.py` percent-script (`# %%` cells). Open that script and run cells
   with Zed's built-in REPL (`ctrl-shift-enter`), with real Jupyter kernels
   and inline output. The pair stays in sync in both directions: notebook
   edits regenerate the script; script edits update the notebook with
   outputs, execution counts and metadata merged back by cell order.

## Install

- **From the extension registry**: search for "IPython Notebook" in Zed's
  extensions page (after publishing).
- **Dev install**: clone this repo, run `zed: install dev extension`, and
  select the repo root.

On first launch the extension downloads the companion language server binary
(`ipynb-lsp`) for your platform from GitHub Releases. For local development
you can skip the download:

```sh
cargo build --release
export IPYNB_LSP_BIN=$PWD/companion/target/release/ipynb-lsp
```

## Using it

1. Open a `.ipynb` file. You get cell-aware highlighting, folding, and an
   outline panel listing cells by id. Structural problems (bad JSON, invalid
   `cell_type`, duplicate ids, missing sources/outputs) show as diagnostics.
2. Code actions (`editor: show code actions`, default `ctrl-.` / `cmd-.`):
   - *Clear all outputs*
   - *Strip empty cells*
   - *Add code cell* / *Add markdown cell*
   - *Pair with percent script (`foo.nb.py`)*
   - *Sync notebook -> script* / *Sync script -> notebook*
   - *Unpair (delete script)*
3. After pairing, open `foo.nb.py` and execute cells with Zed's built-in REPL
   (`repl: run`, default `ctrl-shift-enter`). Choose a kernel with
   `repl: refresh kernelspecs` / the kernel picker.

### Pairing semantics

- Pairing is explicit (a code action); a sibling `.nb.py` is only created
  when you ask.
- **Notebook edits** regenerate the script. Lossless.
- **Script edits** are imported into the notebook automatically, merging
  outputs / execution counts / cell metadata back from the notebook by code
  cell order, so running cells in the script never loses results.
- If both sides change at once, the pair is marked **divergent** with a
  warning diagnostic; resolve with the sync actions. Nothing is written.
- An empty script never wipes a non-empty notebook.

## Limitations (by design, given today's extension API)

- Markdown and outputs are **not rendered** inside the `.ipynb` view. True
  Colab-style rendering requires Zed's native notebook editor (in
  development) or new extension APIs. This project deliberately keeps the
  converter and notebook logic portable so it can migrate upstream.
- REPL output does not flow back into the `.ipynb`; the paired script is the
  executable artifact and execution results stay in Zed's REPL view.
- Cell metadata is not stored in the percent script (the notebook keeps it).
- A python comment line that is exactly `# %%` cannot be represented in the
  script (same trade-off as jupytext).
- Python highlighting inside the raw JSON drops `\"` escape sequences (they
  are illegal in bare python source); multi-line structure is preserved.
- If a cell's `cell_type` value is misspelled, the cell degrades to generic
  JSON highlighting until fixed (diagnostics will point at it).

## Repository layout

```
extension.toml                     Zed extension manifest
src/lib.rs                         WASM shim: resolves/downloads the LSP binary
languages/ipynb-notebook/          language config + tree-sitter queries
grammars/tree-sitter-ipynb-notebook/   the .ipynb grammar
companion/                         ipynb-lsp: diagnostics, code actions, pairing
fixtures/                          sample notebooks for tests
.github/workflows/                 CI + cross-platform release
```

## Development

```sh
# extension (wasm)
cargo check --target wasm32-wasip2

# companion LSP
cd companion && cargo test && cargo build --release

# grammar
cd grammars/tree-sitter-ipynb-notebook
tree-sitter generate
tree-sitter parse ../../fixtures/sample.ipynb
```

Rebuild the grammar after editing `grammar.js` (`tree-sitter generate`
commits `src/parser.c`; Zed compiles it with wasi-sdk).

## Publishing a release

1. Bump `version` in `extension.toml`, `Cargo.toml`, and `RELEASE_TAG` in
   `src/lib.rs` (and `companion/Cargo.toml`).
2. Push a tag `vX.Y.Z`. The release workflow builds the five platform
   binaries and attaches them to the GitHub release.
3. Open a PR against the
   [zed extension registry](https://github.com/zed-industries/extensions).

## License

MIT
