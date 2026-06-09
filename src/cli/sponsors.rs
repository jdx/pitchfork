use crate::Result;

/// Show the companies sponsoring pitchfork and the jdx project family
#[derive(Debug, clap::Args)]
pub struct Sponsors;

impl Sponsors {
    pub async fn run() -> Result<()> {
        println!(
            r#"pitchfork and the jdx project family are sponsored by:

  37signals - https://37signals.com

View all sponsors: https://github.com/sponsors/jdx"#
        );
        Ok(())
    }
}
