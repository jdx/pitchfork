//! mDNS address record publishing for LAN mode.
//!
//! Registers each slug hostname as an mDNS service so that other devices on
//! the LAN can resolve `<slug>.local` to the host's LAN IP address.
//!
//! Because `mdns-sd` does not support registering bare A/AAAA records, we
//! register a virtual service for each slug.  The service type is
//! `_pitchfork._tcp`, and each slug gets its own instance with the hostname
//! set to `<slug>.local.`.  This causes the mDNS responder to include the
//! correct A record in its response, making `<slug>.local` resolvable on
//! the network.

use mdns_sd::ServiceDaemon;
use std::collections::HashMap;
use std::net::Ipv4Addr;

/// The virtual service type used to publish slug hostnames via mDNS.
/// The service name "pitchfork" is exactly 10 chars, well under the RFC 6763
/// limit of 15 chars.
const SERVICE_TYPE: &str = "_pitchfork._tcp.local.";

/// Manages mDNS address records for slug hostnames on the LAN.
pub struct MdnsPublisher {
    daemon: ServiceDaemon,
    /// Registered slug hostname → full instance name (for unregister).
    registrations: HashMap<String, String>,
    /// Current LAN IP (used when re-publishing on IP change).
    lan_ip: Ipv4Addr,
    /// Whether the publisher has been shut down.
    shutdown: bool,
}

impl MdnsPublisher {
    /// Create a new mDNS publisher.
    ///
    /// Returns `None` if the mDNS daemon could not be started (e.g. Avahi
    /// not running on Linux, or Bonjour unavailable on macOS).
    pub fn new(lan_ip: Ipv4Addr) -> Option<Self> {
        let daemon = ServiceDaemon::new().ok()?;
        log::info!("mDNS publisher started with LAN IP {lan_ip}");
        Some(Self {
            daemon,
            registrations: HashMap::new(),
            lan_ip,
            shutdown: false,
        })
    }

    /// Publish an mDNS address record for a slug hostname.
    ///
    /// `hostname` should be the fully qualified name (e.g. `"myapp.local"`).
    /// If the hostname is already registered, this is a no-op.
    pub fn publish(&mut self, hostname: &str, port: u16) {
        if self.shutdown {
            return;
        }
        if self.registrations.contains_key(hostname) {
            return;
        }

        // Derive instance name from hostname: "myapp.local" → "myapp"
        let instance_name = hostname.strip_suffix(".local").unwrap_or(hostname);

        // ServiceInfo requires host_name to end with ".local."
        let host_name = format!("{hostname}.");

        let service = match mdns_sd::ServiceInfo::new(
            SERVICE_TYPE,
            instance_name,
            &host_name,
            self.lan_ip.to_string(),
            port,
            &[] as &[(&str, &str)],
        ) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("mDNS: failed to build ServiceInfo for {hostname}: {e}");
                return;
            }
        };

        let fullname = service.get_fullname().to_string();

        match self.daemon.register(service) {
            Ok(_) => {
                log::info!("mDNS: published {hostname} → {lan}", lan = self.lan_ip);
                self.registrations.insert(hostname.to_string(), fullname);
            }
            Err(e) => {
                log::warn!("mDNS: failed to register {hostname}: {e}");
            }
        }
    }

    /// Unpublish an mDNS address record for a slug hostname.
    pub fn unpublish(&mut self, hostname: &str) {
        if self.shutdown {
            return;
        }
        if let Some(fullname) = self.registrations.remove(hostname) {
            if let Err(e) = self.daemon.unregister(&fullname) {
                log::warn!("mDNS: failed to unregister {hostname}: {e}");
            } else {
                log::info!("mDNS: unpublished {hostname}");
            }
        }
    }

    /// Re-publish all registered hostnames with a new LAN IP.
    ///
    /// Called when the LAN IP changes (detected by polling).  Unregisters all
    /// existing records and re-registers them with the new IP.
    pub fn republish_all(&mut self, new_ip: Ipv4Addr, port: u16) {
        if self.shutdown || new_ip == self.lan_ip {
            return;
        }

        let hostnames: Vec<String> = self.registrations.keys().cloned().collect();

        // Unregister all existing records.
        for hostname in &hostnames {
            if let Some(fullname) = self.registrations.remove(hostname) {
                let _ = self.daemon.unregister(&fullname);
            }
        }

        let old_ip = self.lan_ip;
        self.lan_ip = new_ip;
        log::info!(
            "mDNS: LAN IP changed {old_ip} → {new_ip}, re-publishing {} records",
            hostnames.len()
        );

        // Re-register with new IP.
        for hostname in &hostnames {
            self.publish(hostname, port);
        }
    }

    /// The current LAN IP used for mDNS publishing.
    #[allow(dead_code)]
    pub fn lan_ip(&self) -> Ipv4Addr {
        self.lan_ip
    }

    /// Return the list of currently registered hostnames.
    pub fn registered_hostnames(&self) -> Vec<String> {
        self.registrations.keys().cloned().collect()
    }

    /// Check if a hostname is currently registered.
    pub fn is_published(&self, hostname: &str) -> bool {
        self.registrations.contains_key(hostname)
    }

    /// Shutdown the mDNS daemon gracefully.
    ///
    /// Sends goodbye packets and stops the mDNS responder.
    pub fn shutdown(&mut self) {
        if self.shutdown {
            return;
        }
        self.shutdown = true;
        // Clear registrations so they aren't used after shutdown.
        self.registrations.clear();
        if let Err(e) = self.daemon.shutdown() {
            log::warn!("mDNS: shutdown error: {e}");
        } else {
            log::info!("mDNS: publisher shut down");
        }
    }
}
