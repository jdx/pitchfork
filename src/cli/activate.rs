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
        let s = match self.shell.as_str() {
            "fish" => {
                format!(
                    r#"
function __pitchfork --on-variable PWD
    {} cd --shell-pid "$fish_pid"
end
__pitchfork
"#,
                    env::BIN_PATH.display()
                )
            }
            _ => unimplemented!(),
        };
        println!("{}", s.trim());
        Ok(())
    }
}
