# Tinstar

**The law in your repo.** Write your project's rules in plain language; get checks for code, infra and
layout that people and AI agents can't ignore.

`AGENTS.md` and `CLAUDE.md` tell an agent what the rules are, but nothing makes sure they were followed,
and every rule costs tokens in every conversation. Tinstar turns each rule into a **law**: a small check
that runs without an LLM, in your terminal, your editor, CI and the agent's loop.

![tinstar in a terminal: the rules in AGENTS.md, the laws drafted from them, a check of code, infra
and CI, and what an AI agent is told after an edit](docs/demo/tinstar-terminal.gif)

<sub>The laws in this demo were drafted by an agent in Zed (DeepSeek) through `tinstar mcp`, then
approved. Re-record it with `vhs docs/demo/terminal.tape`.</sub>

```console
$ tinstar check        # in examples/shop (excerpt)
error[no-float-money] `discount` is money: use Money (integer cents), not number
  --> src/cart.ts:11:15
     |
  11 |   let discount: number = code === "WELCOME" ? 0.1 : 0;
     |               ^^^^^^^^
  = law: "Money is always an integer number of cents (the Money type), never a floating point number." (AGENTS.md#money)
  = help: Type it as Money and build it with Money.cents(...).

error[k8s-pinned-images] Image is not pinned to a version (found `registry.example.com/shop-api:latest`)
  --> deploy/api.yaml:11:18
     |
  11 |           image: registry.example.com/shop-api:latest
     |                  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  = law: "Images are pinned to a version, never latest, never without a tag." (AGENTS.md#containers)
  = help: Use an explicit version tag (api:1.4.2) or a digest (api@sha256:…).

error[feature-has-test] Feature folder is incomplete (missing `**/*.test.ts`)
  --> src/features/catalog
  = law: "Every feature folder in src/features/ has an index.ts and at least one test." (AGENTS.md#features)
  = help: Add the missing file next to the feature's code.

✗ 8 errors, 1 warning, from 9 laws
```

> **Status: early.** The engine, the CLI, `tinstar compile`, the MCP server, the language server
> (Zed) and the Claude Code hook work.

## Laws

A law is a YAML file in `.tinstar/laws/`. It keeps the rule as people wrote it (`statement`), how to
check it, and examples it must accept and reject. `tinstar test` runs those examples, so a law that
doesn't do what it says never reaches your project.

There are three kinds:

| Kind | Checks | Example |
|---|---|---|
| `pattern` | code, on the syntax tree, with [ast-grep](https://ast-grep.github.io) rules: TypeScript, JavaScript, Python, Rust, Go, Java, C/C++, C#, YAML, JSON, HCL, Bash and more | money is never a `number`; only `src/payments/` calls the payment API; no `console.log` (with an automatic fix) |
| `config` | YAML and JSON documents: Kubernetes, compose, GitHub Actions, `package.json`… | every Deployment container has resource limits; images are never `latest`; actions are pinned by SHA |
| `layout` | files and folders | no `.env` committed; every feature folder has a test; migrations are named `YYYYMMDD_name.sql` |

```yaml
# .tinstar/laws/k8s-pinned-images.yaml
id: k8s-pinned-images
kind: config
statement: Images are pinned to a version, never latest, never without a tag.
source: AGENTS.md#containers
message: Image is not pinned to a version
hint: Use an explicit version tag (api:1.4.2) or a digest (api@sha256:…).
language: yaml
files: ["deploy/**/*.yaml"]
match:
  kind: [Deployment, StatefulSet, DaemonSet, Job]
assert:
  - path: spec.template.spec.containers[*].image
    matches: (:[^/:]+|@sha256:[0-9a-f]{64})$
    not_matches: :latest$
examples:
  valid:
    - |
      kind: Deployment
      spec: { template: { spec: { containers: [ { image: "api:1.4.2" } ] } } }
  invalid:
    - |
      kind: Deployment
      spec: { template: { spec: { containers: [ { image: "api:latest" } ] } } }
```

Some rules can't be checked from the code ("a discount never exceeds 30% of the order"). Those stay in
`AGENTS.md`, for people and agents, and that's fine.

[`examples/shop`](examples/shop) is a small project with an `AGENTS.md`, nine laws and one planted
violation of each.

## From AGENTS.md to laws

```sh
tinstar compile                 # every section of AGENTS.md (or: tinstar compile docs/RULES.md)
tinstar compile --section money # one section
tinstar approve --all           # after reviewing .tinstar/laws/pending/
```

`tinstar compile` sends each section of the document, with a picture of the project, to an LLM and asks
for laws. Every law that comes back is checked twice before anyone sees it:

- **against its own examples**: a law that doesn't load or gets an example wrong goes back to the
  LLM with the error (`--attempts`, 3 by default; a law that comes back with the same error twice
  isn't asked for again);
- **against the project** (the noise check): a law that fires more than `--noise-limit` times (25)
  in a project that mostly follows its own rules is probably broader than the rule. It goes back
  with the numbers (how many hits are in test code, in which folders) and a sample of the code it
  caught, to be narrowed, confirmed, or left to the document. A law it can't settle is set aside
  with the rejected drafts, never offered for approval.

Only laws that pass are written, to `.tinstar/laws/pending/`, where nothing is enforced until a
person approves them; drafts that never pass are kept in `pending/rejected/` with their last error.
Rules that can't be checked from the code are listed and stay in the document.

Tokens are kept low in three ways: a first, cheap pass over the whole document sets aside sections
with nothing checkable (process, intent); the prompt's stable start (law format, project picture)
comes first so providers' prompt caches serve it; and `.tinstar/compiled.json` remembers what each
section gave, so running again only sends sections whose text changed (`--force` sends all).

```console
## Money (AGENTS.md#money)
  ✓ money-is-cents: 4 example(s) pass, 1 hit(s) in the project, fixed from feedback (answer 2)

## Things a checker can't see (AGENTS.md#things-a-checker-cant-see)
  · stays in AGENTS.md: "Discounts never exceed 30% of the order total." (depends on values at runtime)
```

Tokens are spent here, once; checking costs nothing afterwards. The provider is the first of
`DEEPSEEK_API_KEY`, `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` that is set, or `--provider`
(`anthropic`, `deepseek`, `openai`, or `compatible --base-url … --model …` for a local server such as
Ollama).

## Use

```sh
cargo install --git https://github.com/matpdev/tinstar tinstar   # or, in a clone: cargo install --path crates/tinstar

tinstar init                    # .tinstar/laws/ with an example law
tinstar test                    # every law against its own examples
tinstar check                   # the whole project
tinstar check src/cart.ts deploy/
tinstar check --changed         # files changed in git (modified, staged, untracked)
tinstar check --format agent    # one line per problem, for AI agents
tinstar check --format json     # for tools
tinstar laws                    # list the laws
tinstar lsp                     # language server, over stdio
tinstar hook claude-code        # agent hook (reads the tool call on stdin)
tinstar mcp                     # MCP server, over stdio: agents draft laws with their own model
```

### In your editor

`tinstar lsp` is a language server: broken laws show up as you type, with the plain-language law next
to them and a quick fix when the law has one. Law files are checked as you write them: a typo, a bad
rule or an example the law gets wrong is underlined in the YAML itself.

![tinstar in Zed: a hover with the law a line breaks, the quick fix for console.log, and every
problem in the project, from code to Kubernetes and CI](docs/demo/tinstar-zed.gif)

- **Zed:** install the binary, then *zed: install dev extension* and pick [`editors/zed`](editors/zed).
- **Any other editor:** run `tinstar lsp` over stdio for the languages you use.

### With AI agents

The agent edits a file, tinstar checks it and, if a law broke, tells the agent which one, where, how to
fix it, and the rule in plain language. The agent fixes it before going on.

- **Claude Code:** add [`integrations/claude-code/settings.json`](integrations/claude-code/settings.json)
  to your project's `.claude/settings.json` (a `PostToolUse` hook running `tinstar hook claude-code`).
  It reports errors and warnings in the edited file (and in its folder, for layout laws), and stays
  silent otherwise.

### With your editor's own model (MCP)

`tinstar compile` needs an API key. `tinstar mcp` doesn't: it is an MCP server, and the model you
already use in your editor or agent (Copilot, DeepSeek, Claude, a local one) drafts the laws while
tinstar checks them. Every law still has to pass its examples and the noise check before
`tinstar_save_law` writes it to `.tinstar/laws/pending/`, whichever model wrote it.

| Tool | What it does |
|---|---|
| `tinstar_sections` | How to work, the law format, a picture of the project, the document's sections |
| `tinstar_check_law` | Compiles a drafted law, runs its examples and the noise check; saves nothing |
| `tinstar_save_law` | Checks again and writes it to `pending/` (a noisy law needs `confirmed`) |
| `tinstar_check` | Runs the approved laws on files: agents check their own work |

Then ask the agent: *"Use tinstar to turn AGENTS.md into laws."*

- **Zed:** add [`integrations/zed/settings.json`](integrations/zed/settings.json) to your settings
  (`context_servers`).
- **Claude Code:** `claude mcp add tinstar -- tinstar mcp`, or the project's
  [`.mcp.json`](integrations/claude-code/.mcp.json).
- **VS Code, Cursor and others:** a stdio server running `tinstar mcp`.

Started outside the project, the tools take a `project` argument (an absolute path), or start the
server with `tinstar --root /path/to/project mcp`.

Exit status: `0` nothing broken, `1` a law with `severity: error` is broken, `2` a law file is invalid
or the command failed. Files ignored by `.gitignore` are not checked.

## How it works

One engine (`crates/tinstar-core`), several front-ends that report the same diagnostics. Each file is
parsed once and the tree is shared by every law of its language. See
[`docs/architecture.md`](docs/architecture.md), including the design of the language server.

## License

MIT
