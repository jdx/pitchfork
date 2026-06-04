use crate::Result;

/// Show the companies sponsoring pitchfork and the en.dev project family
#[derive(Debug, clap::Args)]
pub struct Sponsors;

impl Sponsors {
    pub async fn run() -> Result<()> {
        println!(
            r#"pitchfork and the en.dev project family are sponsored by:

  37signals - https://37signals.com

View all sponsors: https://en.dev/sponsors.html"#
        );
        Ok(())
    }
}
