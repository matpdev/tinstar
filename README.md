# Tinstar

**The law in your repo.** Write your project's rules in plain language; get checks for code, infra and
layout that people and AI agents can't ignore.

`AGENTS.md` and `CLAUDE.md` tell an agent what the rules are, but nothing makes sure they were followed,
and every rule costs tokens in every conversation. Tinstar turns each rule into a **law**: a small check
that runs without an LLM, in your terminal, in CI and (soon) in your editor and in the agent's loop.

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

✗ 8 error(s), 1 other(s), from 9 law(s)
```

> **Status: early.** The engine and the CLI work. Next: the language server (Zed, VS Code), agent hooks,
> and `tinstar compile`, which drafts laws from your `AGENTS.md` with an LLM, once.

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

## Use

```sh
cargo install --path crates/tinstar

tinstar init                    # .tinstar/laws/ with an example law
tinstar test                    # every law against its own examples
tinstar check                   # the whole project
tinstar check src/cart.ts deploy/
tinstar check --changed         # files changed in git (modified, staged, untracked)
tinstar check --format agent    # one line per problem, for AI agents
tinstar check --format json     # for tools
tinstar laws                    # list the laws
```

Exit status: `0` nothing broken, `1` a law with `severity: error` is broken, `2` a law file is invalid
or the command failed. Files ignored by `.gitignore` are not checked.

## How it works

One engine (`crates/tinstar-core`), several front-ends that report the same diagnostics. Each file is
parsed once and the tree is shared by every law of its language. See
[`docs/architecture.md`](docs/architecture.md), including the design of the language server.

## License

MIT
