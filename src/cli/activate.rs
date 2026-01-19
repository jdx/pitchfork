use crate::{Result, env};
use miette::bail;

/// Activate pitchfork in your shell session
///
/// Necessary for autostart/stop when entering/exiting projects with pitchfork.toml files
#[derive(Debug, clap::Args)]
#[clap(
    verbatim_doc_comment,
    long_about = "\
Activate pitchfork in your shell session

Generates shell code that enables automatic daemon management when changing
directories. Required for auto-start/stop features in pitchfork.toml.

Supported shells: bash, zsh, fish

Add to your shell config:
  bash (~/.bashrc):
    eval \"$(pitchfork activate bash)\"

  zsh (~/.zshrc):
    eval \"$(pitchfork activate zsh)\"

  fish (~/.config/fish/config.fish):
    pitchfork activate fish | source"
)]
pub struct Activate {
    /// Shell to activate (bash, zsh, fish)
    #[clap()]
    shell: String,
}

impl Activate {
    pub async fn run(&self) -> Result<()> {
        let pitchfork = env::PITCHFORK_BIN.to_string_lossy().to_string();
        let s = match self.shell.as_str() {
            "bash" => format!(
                r#"
__pitchfork() {{
    {pitchfork} cd --shell-pid $$
}}
{}
{}
chpwd_functions+=(__pitchfork)
__pitchfork
"#,
                include_str!("../../assets/bash_zsh_support/chpwd/function.sh"),
                include_str!("../../assets/bash_zsh_support/chpwd/load.sh")
            ),
            "zsh" => format!(
                r#"
__pitchfork() {{
    {pitchfork} cd --shell-pid $$
}}
chpwd_functions+=(__pitchfork)
__pitchfork
"#
            ),
            "fish" => format!(
                r#"
function __pitchfork --on-variable PWD
    {pitchfork} cd --shell-pid "$fish_pid"
end
__pitchfork
"#,
            ),
            shell => bail!("unsupported shell: {shell}. Supported shells: bash, zsh, fish"),
        };
        println!("{}", s.trim());
        Ok(())
    }
}
