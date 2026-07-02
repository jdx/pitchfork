use crate::Result;

/// Show the companies sponsoring pitchfork and the jdx.dev open source tools
#[derive(Debug, clap::Args)]
pub struct Sponsors;

impl Sponsors {
    pub async fn run() -> Result<()> {
        println!(
            r#"pitchfork and the jdx.dev open source tools are sponsored by:

  entire.io - https://entire.io
  37signals - https://37signals.com

View all sponsors: https://jdx.dev/sponsors.html"#
        );
        Ok(())
    }
}
