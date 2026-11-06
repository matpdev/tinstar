//! Zed extension that starts `tinstar lsp` for the project's files.

use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

struct TinstarExtension;

impl zed::Extension for TinstarExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        // `lsp.tinstar.binary` in settings.json overrides the binary found in PATH.
        let binary = LspSettings::for_worktree(language_server_id.as_ref(), worktree)
            .ok()
            .and_then(|settings| settings.binary);
        let command = binary
            .as_ref()
            .and_then(|binary| binary.path.clone())
            .or_else(|| worktree.which("tinstar"))
            .ok_or_else(|| {
                "tinstar not found in PATH: install it with `cargo install --path crates/tinstar`".to_string()
            })?;
        let mut args = vec!["lsp".to_string()];
        args.extend(binary.as_ref().and_then(|b| b.arguments.clone()).unwrap_or_default());
        Ok(zed::Command {
            command,
            args,
            env: binary
                .and_then(|binary| binary.env)
                .map(|env| env.into_iter().collect())
                .unwrap_or_default(),
        })
    }
}

zed::register_extension!(TinstarExtension);
