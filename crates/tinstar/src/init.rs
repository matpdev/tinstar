use std::path::Path;

use anyhow::{Result, bail};
use tinstar_core::LAWS_DIR;

const EXAMPLE: &str = r#"# A law: one rule of this project, checked on every run, with no LLM involved.
# `statement` is the rule as people say it; the rest is how it is checked.
id: no-committed-env
kind: layout
statement: Secrets never go into the repository, so no .env file is committed.
message: .env files must not be committed
hint: Remove the file from git, add it to .gitignore and keep a .env.example with fake values.
severity: error
files: ["**/.env", "**/.env.*"]
ignores: ["**/.env.example"]
forbid: true
examples:
  valid:
    - [".env.example", "src/main.ts"]
  invalid:
    - [".env", "src/main.ts"]
    - ["services/api/.env.production"]
"#;

pub fn run(root: &Path) -> Result<()> {
    let dir = root.join(LAWS_DIR);
    let file = dir.join("no-committed-env.yaml");
    if file.exists() {
        bail!("{} already exists", file.display());
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&file, EXAMPLE)?;
    println!(
        "created {}",
        file.strip_prefix(root).unwrap_or(&file).display()
    );
    println!(
        "next: `tinstar test` checks the law against its examples, `tinstar check` checks the project"
    );
    Ok(())
}
