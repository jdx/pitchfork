use crate::Result;
use crate::ipc::client::IpcClient;
use crate::settings::settings;

/// Starts the internal pitchfork daemon in the background
#[derive(Debug, clap::Args)]
#[clap()]
pub struct Start {
    /// kill existing daemon
    #[clap(short, long)]
    force: bool,
}

impl Start {
    pub async fn run(&self) -> Result<()> {
        IpcClient::connect(true).await?;
        // NOTE: info! routes to stderr (via eprintln! in Logger::log), not stdout.
        // Use println! for user-facing messages that should appear on stdout.
        println!("Pitchfork daemon is running");

        let s = settings();
        if s.proxy.https {
            // Only prompt to trust the cert if it hasn't been generated yet.
            // Once the cert exists the user has already been through the trust
            // flow (or is using a custom cert), so repeating the hint on every
            // `supervisor start` would be noisy.
            let cert_path = if s.proxy.tls_cert.is_empty() {
                crate::env::PITCHFORK_STATE_DIR.join("proxy").join("ca.pem")
            } else {
                std::path::PathBuf::from(&s.proxy.tls_cert)
            };
            if !cert_path.exists() {
                // NOTE: info! routes to stderr (via eprintln! in Logger::log), not stdout.
                println!(
                    "HTTPS proxy is enabled. To trust the self-signed certificate, run: pitchfork proxy trust"
                );
            }
        }

        Ok(())
    }
}
