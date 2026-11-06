# Architecture

One engine, several front-ends. Every front-end loads the same `Lawbook` and reports the same
`Diagnostic`s, so a law broken in the editor is the same law broken in CI and in an agent's hook.

```
                 ┌──────────────────────── tinstar-core (no I/O but reading files) ───────────────────────┐
 .tinstar/laws ─▶│ Law::from_yaml ─▶ Check (strategy per kind)                                            │
   *.yaml        │                    ├─ pattern  ast-grep rule on the syntax tree (code, YAML, HCL, Bash) │
                 │                    ├─ config   assertions on YAML/JSON documents, with spans            │
                 │                    └─ layout   files and folders (no parsing)                          │
                 │ Lawbook::check_source(path, text)  ← one parse per file, shared by every law           │
                 │ Lawbook::check_layout(files)       ← one pass over the file list                       │
                 │ Lawbook::check_project(targets)    ← both, for files on disk                           │
                 └───────────────┬───────────────────────────────┬──────────────────────┬────────────────┘
                                 │                               │                      │
                        tinstar check / test            tinstar lsp             tinstar hook
                        human · json · agent            Zed · VS Code · agents  Claude Code & co.
```

## Language server

Simple to read: one file per concern, one owner for the state, no locks.

```
crates/tinstar/src/lsp/
  mod.rs          handshake, capabilities, the event loop
  state.rs        ServerState: open documents (version + text), the Lawbook snapshot, what was published
  dispatch.rs     method name → handler, one table; unknown requests get MethodNotFound
  documents.rs    didOpen / didChange / didSave / didClose
  diagnostics.rs  schedule → check on a worker → publish (debounce, stale results dropped)
  actions.rs      codeAction: a law's `fix` becomes a quick fix
  convert.rs      tinstar Diagnostic ⇄ lsp_types (UTF-16 columns, codes, links)
  watch.rs        .tinstar/laws/ changed → reload the Lawbook, recheck open files
```

### The loop

```
             ┌──────────── main thread (owns ServerState) ────────────┐
 stdin  ───▶ │ select! {                                               │ ───▶ stdout
 (LSP)       │   lsp message     → dispatch table → handler            │      publishDiagnostics
             │   worker result   → drop if stale, else publish         │
             │   debounce tick   → send due files to the workers       │
             │ }                                                       │
             └───────────▲──────────────────────────────┬──────────────┘
                         │ CheckDone{uri, version, …}   │ CheckJob{uri, version, text: Arc<str>, book: Arc<Lawbook>}
                         └────────── worker threads ◀───┘
```

### Patterns, and what each one buys

| Pattern | Where | Why |
|---|---|---|
| **Ports and adapters** | `tinstar-core` vs CLI / LSP / hook | The engine knows nothing about terminals or JSON-RPC. Front-ends stay thin and report identical results. |
| **Strategy** | `Check` (pattern / config / layout) | A new kind of law (dockerfile, imports) is one new variant; nothing else changes. |
| **Parse once, run many** | `Lawbook::check_source` | The file is parsed once per language and the tree is shared by every law of that language. With many laws this is where the time goes. |
| **Single-owner event loop** (actor) | `lsp/mod.rs` | Only the main thread touches the state, so there are no locks and messages are handled in order. |
| **Dispatch table** (command) | `lsp/dispatch.rs` | Every LSP method maps to one function in a known file: easy to find, easy to add. |
| **Worker pool with message passing** | `lsp/diagnostics.rs` | Checks never block the loop; typing stays responsive while a big file is checked. |
| **Versioned results, "latest wins"** | `CheckDone.version` | A result for an older version of the document is dropped. Cheaper and simpler than cancelling work. |
| **Debounce** | `didChange` → ~150 ms | Keystrokes are coalesced: one check per pause, not one per key. `didSave` checks at once. |
| **Immutable snapshot** (copy-on-write) | `Arc<Lawbook>` | Editing a law swaps in a new book; running jobs finish with the old one and never see half-loaded laws. |
| **Memoization** | `(content hash, book generation) → diagnostics` | Saving unchanged text costs nothing. |
| **Mapper** | `lsp/convert.rs` | All LSP details (UTF-16 columns, severities, codes) live in one place. |

### What a diagnostic carries in the editor

- `code`: the law's id (`no-float-money`), stable, so agents and people can refer to it;
- `codeDescription.href`: a link to the law's file;
- `message`: the law's message, plus what was found;
- `relatedInformation`: the law as written in plain language and where it lives (`AGENTS.md#money`);
- `data`: the fix, if the law has one, so `codeAction` can offer it without checking again.

The whole project is checked from disk on start, on save and when files are created or deleted
(`workspace/didChangeWatchedFiles`), so problems show up in files nobody has opened (the editor's
project diagnostics list them all). For a closed file the server shows the results for the file as
saved; once it is opened, the results for the text in the editor take over. Layout laws are about
the project, not one file: their diagnostics are published on the offending file or, for a folder,
on its first file (usually its index), where people work.

Law files get diagnostics of their own: while one is being typed it is compiled and run against its
examples, so a typo, a bad rule or a wrong example is underlined in the YAML before it is saved.
Saving a law reloads the book.

### Speed, in numbers to keep

- per-file checks: one parse + one tree walk per law, no I/O beyond the text the editor sent;
- layout checks: one pass over the file list, glob sets compiled once when the book loads;
- the loop does no checking itself; its only job is ordering messages.

The CLI uses the same engine; `tinstar check` on a whole repository is one walk honouring
.gitignore plus the per-file checks above.
