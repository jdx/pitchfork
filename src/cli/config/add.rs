use crate::pitchfork_toml::{PitchforkToml, PitchforkTomlAuto, PitchforkTomlDaemon};
use crate::Result;

/// Add a new daemon to ./pitchfork.toml
#[derive(Debug, clap::Args)]
#[clap(visible_alias = "a", verbatim_doc_comment)]
pub struct Add {
    /// ID of the daemon to add
    pub id: String,
    /// Arguments to pass to the daemon
    #[clap(allow_hyphen_values = true, trailing_var_arg = true)]
    pub args: Vec<String>,
    /// Autostart the daemon when entering the directory
    #[clap(long)]
    pub autostart: bool,
    /// Autostop the daemon when leaving the directory
    #[clap(long)]
    pub autostop: bool,
}

impl Add {
    pub async fn run(&self) -> Result<()> {
        let mut pt = PitchforkToml::read("pitchfork.toml").unwrap_or_default();
        pt.path = pt.path.or(Some("pitchfork.toml".into()));
        let mut auto = vec![];
        if self.autostart {
            auto.push(PitchforkTomlAuto::Start);
        }
        if self.autostop {
            auto.push(PitchforkTomlAuto::Stop);
        }
        pt.daemons.insert(
            self.id.clone(),
            PitchforkTomlDaemon {
                run: shell_words::join(&self.args),
                auto,
                cron: None,
                path: None,
            },
        );
        pt.write()?;
        println!("added {} to {}", self.id, pt.path.unwrap().display());
        Ok(())
    }
}
