use crate::{env, Result};

/// Activate pitchfork in your shell session
///
/// Necessary for autostart/stop when entering/exiting projects with pitchfork.toml files
#[derive(Debug, clap::Args)]
#[clap(verbatim_doc_comment)]
pub struct Activate {
    /// The shell to generate source for
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
            _ => unimplemented!(),
        };
        println!("{}", s.trim());
        Ok(())
    }
}
