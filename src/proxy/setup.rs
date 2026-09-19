//! Host integration for the proxy: `pitchfork proxy setup`, `--undo`, `doctor`.
//!
//! The supervisor itself never needs privileges. Everything that does — writing
//! a resolver file, installing the CA, letting an unprivileged process reach
//! ports 80 and 443 — is collected here, printed as a plan before anything runs,
//! and reversed by `--undo`.
//!
//! The plan is built from a [`SetupContext`] that carries every platform fact
//! the steps depend on, so the same code can be exercised in tests for a
//! platform the test is not running on.

use std::path::{Path, PathBuf};

use crate::Result;

/// Marker delimiting a pitchfork-managed block inside a file we do not own.
const MARKER_START: &str = "# pitchfork-start";
const MARKER_END: &str = "# pitchfork-end";

/// Header written at the top of files pitchfork owns outright.
const OWNED_HEADER: &str = "# Managed by pitchfork (pitchfork proxy setup)";

/// Host platform, as far as setup is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Platform {
    MacOs,
    Linux,
    Other,
}

impl Platform {
    /// The platform this binary was built for.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "linux") {
            Platform::Linux
        } else {
            Platform::Other
        }
    }
}

/// Where `proxy setup` generates its certificate authority.
fn default_generated_ca() -> PathBuf {
    crate::env::PITCHFORK_STATE_DIR.join("proxy").join("ca.pem")
}

/// Everything the plan depends on, gathered up front so planning is pure.
///
/// Serialized after a run so `--undo` can reverse what setup actually did
/// rather than what the settings say now. Between the two, `proxy.tld`,
/// `proxy.port`, `proxy.host` or `proxy.tls_cert` may well have changed, and
/// undo built from the new values would probe for a different iptables rule or
/// PAC URL and leave the real ones in place.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SetupContext {
    pub platform: Platform,
    /// TLD proxy URLs live under.
    pub tld: String,
    /// Port the loopback resolver listens on.
    pub dns_port: u16,
    /// Port the proxy listens on.
    pub proxy_port: u16,
    /// Whether the proxy serves HTTPS (decides CA steps and which standard port matters).
    pub https: bool,
    /// Whether the loopback resolver is enabled at all.
    pub dns_enabled: bool,
    /// Configure the system through a PAC file instead of the resolver.
    pub pac: bool,
    /// Whether systemd-resolved is the active stub resolver.
    pub systemd_resolved: bool,
    /// Major version of systemd, when it could be determined.
    pub systemd_version: Option<u32>,
    /// Whether LAN mode is on, in which case `.local` is mDNS territory.
    pub lan: bool,
    /// Address a local client uses to reach the proxy, as it appears in a URL.
    ///
    /// Normally `127.0.0.1`, but `proxy.host = "::1"` means nothing is
    /// listening on IPv4 and the PAC file has to say so.
    pub contact_host: String,
    /// Path to the CA certificate.
    pub ca_path: PathBuf,
    /// Whether the CA is already in the system trust store.
    pub ca_trusted: bool,
    /// Whether a custom `tls_cert` is configured (pitchfork then owns no CA).
    pub custom_cert: bool,
    /// Where the CA pitchfork generates lives, whatever `proxy.tls_cert` says.
    ///
    /// Undo works from this rather than `ca_path`: configuring a custom
    /// certificate later must not hide a CA an earlier setup installed.
    ///
    /// Records written before this field existed default to the current
    /// generated path rather than to an empty one, which would have planned the
    /// removal of nothing.
    #[serde(default = "default_generated_ca")]
    pub generated_ca: PathBuf,
    /// Path to the running pitchfork binary, used for `setcap` and re-invocation.
    pub binary: PathBuf,
    /// Directory holding per-domain macOS resolver files (`/etc/resolver`).
    pub resolver_dir: PathBuf,
    /// Directory holding systemd-resolved drop-ins.
    pub resolved_dropin_dir: PathBuf,
    /// Path to `pf.conf` on macOS.
    pub pf_conf: PathBuf,
    /// Path to the pitchfork pf anchor file on macOS.
    pub pf_anchor: PathBuf,
    /// Active macOS network services to point at the PAC file.
    pub network_services: Vec<String>,
    /// Whether the GNOME proxy settings are available.
    pub gnome: bool,
    /// The automatic-proxy configuration found before `--pac` overwrote it.
    ///
    /// Without this, `--undo` can only switch the proxy off: it has nothing to
    /// put back, so a machine that already had a corporate or hand-written PAC
    /// URL would lose it the first time `setup --pac` ran. Entries pointing at
    /// pitchfork's own PAC file are not recorded, so re-running setup cannot
    /// overwrite a genuine earlier value with our own.
    ///
    /// Records written before this field existed simply have none, which
    /// leaves undo behaving as it did before.
    #[serde(default)]
    pub prior_auto_proxy: Vec<PriorAutoProxy>,
}

/// An automatic-proxy setting as it stood before setup changed it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PriorAutoProxy {
    /// The macOS network service this belongs to, or `gnome`.
    pub target: String,
    /// The URL that was configured.
    pub url: String,
    /// The switch that went with it: `on`/`off` on macOS, the proxy mode on
    /// GNOME. Recorded verbatim because it is passed back to the same tool.
    pub state: String,
}

impl SetupContext {
    /// The standard port proxy URLs would use with no port suffix.
    fn standard_port(&self) -> u16 {
        if self.https { 443 } else { 80 }
    }

    /// Whether traffic has to be redirected from the standard port.
    ///
    /// False when the proxy already listens there, and false when the proxy is
    /// on a privileged port it cannot bind unprivileged — that case is reported
    /// as a problem rather than papered over with a redirect.
    fn needs_port_redirect(&self) -> bool {
        self.proxy_port >= 1024 && self.proxy_port != self.standard_port()
    }

    /// macOS resolver file for the configured TLD.
    fn resolver_file(&self) -> PathBuf {
        self.resolver_dir.join(&self.tld)
    }

    /// systemd-resolved drop-in for the configured TLD.
    fn resolved_dropin(&self) -> PathBuf {
        self.resolved_dropin_dir.join("pitchfork.conf")
    }

    /// URL of the PAC script served by the proxy.
    fn pac_url(&self) -> String {
        super::pac::url(&self.contact_host, self.proxy_port)
    }
}

/// A command run for its result rather than its effect.
///
/// Used to decide whether a step still needs doing, and whether something is
/// ours to undo, by asking the system rather than assuming.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub argv: Vec<String>,
    /// Every one of these must hold for the probe to pass.
    pub expect: Vec<ProbeExpect>,
}

/// A condition on a probe's output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeExpect {
    /// The output contains this text anywhere.
    Contains(String),
}

impl Probe {
    /// A probe that passes on exit status alone.
    fn status(argv: Vec<String>) -> Self {
        Probe {
            argv,
            expect: vec![],
        }
    }

    /// A probe whose output must contain `expect`.
    fn output(argv: Vec<String>, expect: impl Into<String>) -> Self {
        Probe {
            argv,
            expect: vec![ProbeExpect::Contains(expect.into())],
        }
    }
}

/// One action in a plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Replace a file pitchfork owns entirely.
    WriteFile {
        path: PathBuf,
        content: String,
        sudo: bool,
    },
    /// Delete a file pitchfork wrote.
    ///
    /// The file's managed header is checked first: `--undo` must never delete a
    /// root-owned file that somebody else put there under a name we happen to
    /// use.
    RemoveFile {
        path: PathBuf,
        sudo: bool,
        /// A file that must no longer reference this one before it is removed.
        ///
        /// The mirror of `EnsureBlock`'s `requires`. `apply` continues past a
        /// failed step, so a `RemoveBlock` that did not go through would
        /// otherwise be followed by the deletion of the file that block names,
        /// leaving `/etc/pf.conf` pointing at a missing anchor — the same
        /// broken reference, arrived at from the undo side.
        still_referenced_by: Option<PathBuf>,
    },
    /// Insert or replace a marked block inside a file pitchfork shares.
    EnsureBlock {
        path: PathBuf,
        content: String,
        sudo: bool,
        /// Place the block where pf's rule ordering requires, rather than at
        /// the end of the file.
        pf_order: bool,
        /// A file the block's content refers to, which must exist before the
        /// block is written.
        ///
        /// `apply` runs every step, continuing past one that failed, so that a
        /// single refusal does not abandon the independent work either side of
        /// it. That is wrong for a block that names another file: splicing
        /// `load anchor ... from "<path>"` into `/etc/pf.conf` after the write
        /// of `<path>` failed leaves a dangling reference in a shared system
        /// file, which breaks every later `pfctl -f` including the one at
        /// boot. Naming the prerequisite here turns that into a clean failure
        /// of this step instead.
        requires: Option<PathBuf>,
    },
    /// Remove pitchfork's marked block from a shared file.
    RemoveBlock { path: PathBuf, sudo: bool },
    /// Run a command.
    Run {
        argv: Vec<String>,
        sudo: bool,
        /// A probe that succeeds when this action has already taken effect.
        /// Without one the action is assumed safe to repeat.
        skip_if: Option<Probe>,
    },
    /// Run a command only when a probe shows the state is actually ours to
    /// revert.
    ///
    /// `--undo` runs against whatever the machine looks like now, which is not
    /// necessarily what setup left behind. Reverting a proxy URL somebody else
    /// configured, or stripping capabilities pitchfork never granted, would
    /// damage unrelated system state.
    RunIfPresent {
        /// Probe whose success means there is something of ours to revert.
        probe: Probe,
        argv: Vec<String>,
        sudo: bool,
    },
    /// Drop `cap_net_bind_service` from a binary's file capabilities.
    ///
    /// Its own action because `setcap -r` clears the whole set: whether that is
    /// safe depends on what else is on the file, which has to be read first.
    RevokeBindCapability { binary: PathBuf },
    /// Install the CA into the system trust store, in this process.
    TrustCa { path: PathBuf },
    /// Remove the CA from the system trust store.
    ///
    /// Skipped when that certificate is not trusted, which is the probe that
    /// lets undo queue it unconditionally: an earlier setup may have installed
    /// the generated CA even though a custom certificate is configured now.
    UntrustCa { path: PathBuf, sudo: bool },
    /// Nothing to do; the line exists to explain why.
    Note,
}

/// The system resource a step installs or removes.
///
/// Forward and undo steps for the same resource carry the same value, which is
/// what lets a re-run tell "this configuration still wants that" from "that
/// belonged to a configuration being replaced". Comparing descriptions cannot
/// do it: installing and removing the same thing read quite differently.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Resource {
    /// A file pitchfork owns, by path.
    File(PathBuf),
    /// A marked block inside a file pitchfork shares, by path.
    Block(PathBuf),
    /// A redirect from one port to another.
    Redirect { from: u16, to: u16 },
    /// The bind capability on a binary.
    BindCapability(PathBuf),
    /// The CA in the system trust store.
    TrustedCa(PathBuf),
    /// An automatic proxy URL on a named network service.
    AutoProxy { service: String, url: String },
    /// A service reload, which belongs to whatever configuration triggered it.
    ServiceReload(String),
}

/// A single planned step: what it does, and how it is described.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// One line, shown in the plan and echoed while running.
    pub summary: String,
    pub action: Action,
    /// What this step acts on, when it acts on something durable.
    pub resource: Option<Resource>,
}

impl Step {
    fn note(summary: impl Into<String>) -> Self {
        Step {
            summary: summary.into(),
            action: Action::Note,
            resource: None,
        }
    }

    /// Whether running this step needs elevated privileges.
    pub fn needs_sudo(&self) -> bool {
        match &self.action {
            Action::WriteFile { sudo, .. }
            | Action::RemoveFile { sudo, .. }
            | Action::EnsureBlock { sudo, .. }
            | Action::RemoveBlock { sudo, .. }
            | Action::Run { sudo, .. }
            | Action::RunIfPresent { sudo, .. } => *sudo,
            Action::RevokeBindCapability { .. } => true,
            // macOS installs into the login keychain and prompts on its own;
            // on Linux the CA step is planned as a sudo'd re-invocation instead.
            Action::UntrustCa { sudo, .. } => *sudo,
            Action::TrustCa { .. } | Action::Note => false,
        }
    }
}

/// An ordered list of steps plus trailing advice.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Things the user has to do by hand, printed after the plan.
    pub manual: Vec<String>,
}

impl Plan {
    /// One display line per step, `sudo` steps marked.
    pub fn describe(&self) -> Vec<String> {
        self.steps
            .iter()
            .map(|s| {
                if s.needs_sudo() {
                    format!("[sudo] {}", s.summary)
                } else {
                    s.summary.clone()
                }
            })
            .collect()
    }

    /// Whether any step needs elevated privileges.
    pub fn needs_sudo(&self) -> bool {
        self.steps.iter().any(Step::needs_sudo)
    }

    /// Whether the plan would change anything at all.
    pub fn is_empty(&self) -> bool {
        self.steps.iter().all(|s| s.action == Action::Note)
    }
}

// ─── Plan construction ───────────────────────────────────────────────────────

/// Contents of the macOS `/etc/resolver/<tld>` file.
fn macos_resolver_file(dns_port: u16) -> String {
    format!("{OWNED_HEADER}\nnameserver 127.0.0.1\nport {dns_port}\n")
}

/// Contents of the systemd-resolved drop-in routing the TLD at our resolver.
///
/// `Domains=~<tld>` marks the TLD a routing-only domain, so only names under it
/// are sent to the listed server; everything else keeps using the link's own
/// DNS servers.
fn resolved_dropin(tld: &str, dns_port: u16) -> String {
    format!("{OWNED_HEADER}\n[Resolve]\nDNS=127.0.0.1:{dns_port}\nDomains=~{tld}\n")
}

/// pf rules redirecting the standard port to the proxy's unprivileged port.
fn pf_anchor_rules(standard_port: u16, proxy_port: u16) -> String {
    format!(
        "{OWNED_HEADER}\n\
         rdr pass on lo0 inet proto tcp from any to any port {standard_port} -> 127.0.0.1 port {proxy_port}\n"
    )
}

/// Reject a TLD that must not reach a privileged file path or a config file.
///
/// `proxy.tld` is an arbitrary user string that ends up joined onto
/// `/etc/resolver` for a `sudo` write and delete, and interpolated into a
/// systemd-resolved drop-in. A traversal component would aim those at a
/// root-owned file elsewhere; a newline would inject directives into the
/// drop-in. Only a real DNS suffix is allowed through.
pub fn validate_tld(tld: &str) -> Result<()> {
    if !super::pac::is_valid_tld(tld) {
        miette::bail!(
            "proxy.tld {tld:?} is not a valid host name suffix.\n\
             It must be a DNS suffix such as `localhost` or `test`: \
             dot-separated labels of ASCII letters, digits and `-`, each at \
             most 63 bytes and not starting or ending with `-`.\n\
             `proxy setup` writes it into privileged system files, so it is \
             refused rather than escaped."
        );
    }
    Ok(())
}

/// Build the plan for `pitchfork proxy setup`.
pub fn plan(ctx: &SetupContext) -> Plan {
    let mut plan = Plan::default();
    if ctx.pac {
        plan_pac(ctx, &mut plan);
    } else {
        plan_resolver(ctx, &mut plan);
    }
    plan_ca(ctx, &mut plan);
    plan_ports(ctx, &mut plan);
    plan
}

/// Resolver steps: teach the system where `*.<tld>` is answered.
fn plan_resolver(ctx: &SetupContext, plan: &mut Plan) {
    if !ctx.dns_enabled {
        plan.steps.push(Step::note(
            "proxy.dns is false, so no resolver is running — skipping resolver setup",
        ));
        return;
    }
    if ctx.lan {
        // LAN mode serves `.local`, which belongs to mDNS. Routing that suffix
        // at a unicast resolver would take the whole Bonjour namespace with it,
        // so other machines, printers and AirDrop names would stop resolving.
        // mDNS already answers these names; nothing needs installing.
        plan.steps.push(Step::note(
            "LAN mode resolves *.local over mDNS — leaving the .local namespace alone",
        ));
        return;
    }
    match ctx.platform {
        Platform::MacOs => {
            plan.steps.push(Step {
                summary: format!(
                    "write {} pointing *.{} at 127.0.0.1:{}",
                    ctx.resolver_file().display(),
                    ctx.tld,
                    ctx.dns_port
                ),
                action: Action::WriteFile {
                    path: ctx.resolver_file(),
                    content: macos_resolver_file(ctx.dns_port),
                    sudo: true,
                },
                resource: Some(Resource::File(ctx.resolver_file())),
            });
            if ctx.tld.eq_ignore_ascii_case("localhost") {
                // A resolver file takes over DNS for the suffix it names, and
                // the responder only runs inside the supervisor. Worth saying
                // out loud for the default TLD, because it is the one people
                // will not think to check.
                plan.manual.push(
                    concat!(
                        "Note: /etc/resolver/localhost hands *.localhost lookups to ",
                        "pitchfork, so subdomains such as api.localhost resolve only while ",
                        "the supervisor is running. Plain `localhost` keeps resolving from ",
                        "/etc/hosts either way. Undo this with `pitchfork proxy setup --undo`.",
                    )
                    .to_string(),
                );
            }
        }
        Platform::Linux if ctx.systemd_resolved && ctx.tld.eq_ignore_ascii_case("localhost") => {
            plan.steps.push(Step::note(
                "systemd-resolved already answers *.localhost with 127.0.0.1 — no resolver change needed",
            ));
        }
        Platform::Linux if ctx.systemd_resolved => {
            plan.steps.push(Step {
                summary: format!(
                    "write {} routing *.{} to 127.0.0.1:{}",
                    ctx.resolved_dropin().display(),
                    ctx.tld,
                    ctx.dns_port
                ),
                action: Action::WriteFile {
                    path: ctx.resolved_dropin(),
                    content: resolved_dropin(&ctx.tld, ctx.dns_port),
                    sudo: true,
                },
                resource: Some(Resource::File(ctx.resolved_dropin())),
            });
            plan.steps.push(Step {
                summary: "restart systemd-resolved to pick up the route (interrupts DNS briefly)"
                    .to_string(),
                action: Action::Run {
                    argv: vec![
                        "systemctl".into(),
                        "restart".into(),
                        "systemd-resolved".into(),
                    ],
                    sudo: true,
                    // Unconditional, deliberately.
                    //
                    // Four guards have been tried here: whether the file
                    // changed, whether `resolvectl status` shows the server and
                    // domain, whether those appear in the same scope, and
                    // whether a name resolves. Each was wrong in a way that
                    // skipped a restart that was needed, and the symptom every
                    // time was setup reporting success while proxy names did
                    // not resolve.
                    //
                    // What the guard bought was avoiding a brief interruption
                    // to name resolution when setup is re-run with nothing to
                    // change. That is not worth a silent failure, in a command
                    // the user invoked on purpose and which already stops to
                    // ask for a password.
                    skip_if: None,
                },
                resource: Some(Resource::ServiceReload("systemd-resolved".into())),
            });
            // `DNS=<addr>:<port>` needs systemd 246, and routing a domain at it
            // with `Domains=~<tld>` needs 247. On anything older the files are
            // written and the service restarts, but the route never takes
            // effect, so say so here rather than let `doctor` be the first hint.
            if let Some(v) = ctx.systemd_version
                && v < 247
            {
                plan.manual.push(format!(
                    "Warning: systemd {v} is too old to route a domain at a resolver on a \
                     non-standard port. That needs 246 for `DNS=127.0.0.1:{port}` and 247 \
                     for `Domains=~{tld}`.\n\
                     The drop-in will be written but will not take effect. Use \
                     `pitchfork proxy setup --pac`, or point dnsmasq at \
                     127.0.0.1#{port} and make it your system resolver.",
                    port = ctx.dns_port,
                    tld = ctx.tld
                ));
            }
        }
        Platform::Linux => {
            plan.manual.push(format!(
                "systemd-resolved is not active, so pitchfork cannot route *.{tld} for you.\n\
                 Point a local resolver at pitchfork instead, for example with dnsmasq:\n\
                 \x20   # /etc/dnsmasq.d/pitchfork\n\
                 \x20   server=/{tld}/127.0.0.1#{port}\n\
                 then make dnsmasq your system resolver and restart it.\n\
                 Alternatively run `pitchfork proxy setup --pac`, which needs no root access.",
                tld = ctx.tld,
                port = ctx.dns_port
            ));
        }
        Platform::Other => {
            plan.manual.push(format!(
                "pitchfork cannot configure this platform's resolver automatically.\n\
                 Point your system resolver at 127.0.0.1:{port} for *.{tld}, \
                 or run `pitchfork proxy setup --pac`.",
                port = ctx.dns_port,
                tld = ctx.tld
            ));
        }
    }
}

/// PAC steps: the no-sudo path.
fn plan_pac(ctx: &SetupContext, plan: &mut Plan) {
    let url = ctx.pac_url();
    plan.steps.push(Step::note(format!(
        "the supervisor serves the PAC file at {url} while the proxy is running"
    )));
    match ctx.platform {
        Platform::MacOs if !ctx.network_services.is_empty() => {
            for service in &ctx.network_services {
                plan.steps.push(Step {
                    summary: format!("set the automatic proxy URL for \"{service}\" to {url}"),
                    action: Action::Run {
                        argv: vec![
                            "networksetup".into(),
                            "-setautoproxyurl".into(),
                            service.clone(),
                            url.clone(),
                        ],
                        sudo: false,
                        skip_if: None,
                    },
                    resource: Some(Resource::AutoProxy {
                        service: service.clone(),
                        url: url.clone(),
                    }),
                });
            }
        }
        Platform::MacOs => {
            plan.manual.push(format!(
                "No active network services were found, so the automatic proxy URL was not set.\n\
                 Set it by hand in System Settings → Network → Details → Proxies → \
                 Automatic proxy configuration, using {url}."
            ));
        }
        Platform::Linux if ctx.gnome => {
            plan.steps.push(Step {
                summary: format!("set the GNOME automatic proxy URL to {url}"),
                action: Action::Run {
                    argv: vec![
                        "gsettings".into(),
                        "set".into(),
                        "org.gnome.system.proxy".into(),
                        "autoconfig-url".into(),
                        url.clone(),
                    ],
                    sudo: false,
                    skip_if: None,
                },
                resource: Some(Resource::AutoProxy {
                    service: "gnome".into(),
                    url: url.clone(),
                }),
            });
            plan.steps.push(Step {
                summary: "switch the GNOME proxy mode to automatic".to_string(),
                action: Action::Run {
                    argv: vec![
                        "gsettings".into(),
                        "set".into(),
                        "org.gnome.system.proxy".into(),
                        "mode".into(),
                        "auto".into(),
                    ],
                    sudo: false,
                    skip_if: None,
                },
                resource: None,
            });
        }
        _ => {
            plan.manual.push(format!(
                "pitchfork cannot set this system's automatic proxy URL for you.\n\
                 Point your browser or desktop proxy settings at {url}."
            ));
        }
    }
}

/// CA steps: make the leaf certificates the proxy mints verifiable.
fn plan_ca(ctx: &SetupContext, plan: &mut Plan) {
    if !ctx.https {
        return;
    }
    if ctx.custom_cert {
        plan.steps.push(Step::note(
            "proxy.tls_cert is set, so pitchfork serves your certificate and installs no CA",
        ));
        return;
    }
    if ctx.ca_trusted {
        // Claims the resource even though there is nothing to do: this
        // configuration still wants the CA trusted, so a re-run must not let
        // an earlier record's undo step remove it.
        plan.steps.push(Step {
            summary: format!(
                "the pitchfork CA at {} is already trusted",
                ctx.ca_path.display()
            ),
            action: Action::Note,
            resource: Some(Resource::TrustedCa(ctx.ca_path.clone())),
        });
        return;
    }
    let summary = format!(
        "install the pitchfork CA at {} into the system trust store",
        ctx.ca_path.display()
    );
    // On Linux the trust store is root-owned, so the step re-invokes pitchfork
    // under sudo. On macOS the login keychain takes it unprivileged, with the
    // OS prompting for confirmation.
    let action = if ctx.platform == Platform::Linux {
        Action::Run {
            // `--cert` is passed explicitly: sudo resets the environment, so a
            // child left to re-derive the path would read a different
            // PITCHFORK_STATE_DIR and trust a certificate the proxy never
            // serves, while reporting success.
            argv: vec![
                ctx.binary.to_string_lossy().into_owned(),
                "proxy".into(),
                "trust".into(),
                "--cert".into(),
                ctx.ca_path.to_string_lossy().into_owned(),
            ],
            sudo: true,
            skip_if: None,
        }
    } else {
        Action::TrustCa {
            path: ctx.ca_path.clone(),
        }
    };
    plan.steps.push(Step {
        summary,
        action,
        resource: Some(Resource::TrustedCa(ctx.ca_path.clone())),
    });
}

/// Port steps: let the proxy answer on 80/443 without running as root.
fn plan_ports(ctx: &SetupContext, plan: &mut Plan) {
    let standard = ctx.standard_port();
    if ctx.proxy_port < 1024 {
        // The supervisor is configured to bind a privileged port itself, which
        // no redirect can help with — something has to grant it that right.
        match ctx.platform {
            Platform::Linux => plan.steps.push(Step {
                summary: format!(
                    "grant {} permission to bind ports below 1024 (cap_net_bind_service)",
                    ctx.binary.display()
                ),
                action: Action::Run {
                    argv: vec![
                        "setcap".into(),
                        "cap_net_bind_service=+ep".into(),
                        ctx.binary.to_string_lossy().into_owned(),
                    ],
                    sudo: true,
                    skip_if: None,
                },
                resource: Some(Resource::BindCapability(ctx.binary.clone())),
            }),
            _ => plan.manual.push(format!(
                "proxy.port is {port}, which an unprivileged process cannot bind on macOS, \
                 and pitchfork will not run the supervisor as root.\n\
                 Set an unprivileged port and re-run setup, which then redirects \
                 {port} to it through pf:\n\
                 \x20   pitchfork settings set proxy.port {suggested}",
                port = ctx.proxy_port,
                suggested = if ctx.https { 8443 } else { 8080 }
            )),
        }
        return;
    }
    if !ctx.needs_port_redirect() {
        plan.steps.push(Step::note(format!(
            "the proxy listens on port {}, which needs no redirect",
            ctx.proxy_port
        )));
        return;
    }
    if ctx.pac {
        // The PAC file sends the browser straight at the proxy port, so the
        // standard port never enters the picture.
        plan.steps.push(Step::note(format!(
            "the PAC file sends requests directly to port {}, so no port redirect is needed",
            ctx.proxy_port
        )));
        return;
    }
    match ctx.platform {
        Platform::MacOs => {
            plan.steps.push(Step {
                summary: format!(
                    "write {} redirecting port {standard} to {}",
                    ctx.pf_anchor.display(),
                    ctx.proxy_port
                ),
                action: Action::WriteFile {
                    path: ctx.pf_anchor.clone(),
                    content: pf_anchor_rules(standard, ctx.proxy_port),
                    sudo: true,
                },
                resource: Some(Resource::File(ctx.pf_anchor.clone())),
            });
            plan.steps.push(Step {
                summary: format!("load the pitchfork anchor into {}", ctx.pf_conf.display()),
                action: Action::EnsureBlock {
                    path: ctx.pf_conf.clone(),
                    content: format!(
                        "rdr-anchor \"pitchfork\"\nload anchor \"pitchfork\" from \"{}\"",
                        ctx.pf_anchor.display()
                    ),
                    sudo: true,
                    pf_order: true,
                    // The block names the anchor file, so it must not be
                    // written before that file exists.
                    requires: Some(ctx.pf_anchor.clone()),
                },
                resource: Some(Resource::Block(ctx.pf_conf.clone())),
            });
            plan.steps.push(Step {
                summary: "enable pf and load the new rules".to_string(),
                action: Action::Run {
                    argv: vec![
                        "pfctl".into(),
                        "-Ef".into(),
                        ctx.pf_conf.to_string_lossy().into_owned(),
                    ],
                    sudo: true,
                    skip_if: None,
                },
                resource: Some(Resource::ServiceReload("pf".into())),
            });
        }
        Platform::Linux => {
            plan.steps.push(Step {
                summary: format!(
                    "redirect loopback traffic for port {standard} to {} (iptables)",
                    ctx.proxy_port
                ),
                action: Action::Run {
                    argv: iptables_redirect_argv("-A", standard, ctx.proxy_port),
                    sudo: true,
                    // `-C` checks for the identical rule, so re-running setup
                    // does not stack duplicate NAT entries.
                    skip_if: Some(Probe::status(iptables_redirect_argv(
                        "-C",
                        standard,
                        ctx.proxy_port,
                    ))),
                },
                resource: Some(Resource::Redirect {
                    from: standard,
                    to: ctx.proxy_port,
                }),
            });
        }
        Platform::Other => plan.manual.push(format!(
            "Redirect port {standard} to {} yourself, or use the port in the URL.",
            ctx.proxy_port
        )),
    }
}

/// iptables arguments for the loopback redirect, parameterised by `-A`/`-D`.
fn iptables_redirect_argv(op: &str, from: u16, to: u16) -> Vec<String> {
    [
        "iptables",
        "-t",
        "nat",
        op,
        "OUTPUT",
        "-p",
        "tcp",
        "-o",
        "lo",
        "--dport",
        &from.to_string(),
        "-j",
        "REDIRECT",
        "--to-ports",
        &to.to_string(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Resolver files in `dir` that pitchfork wrote, plus the one for `tld`.
///
/// The current TLD's path is always included even when the file is absent, so
/// the plan still lists it and reports "nothing there" rather than staying
/// silent. Scanning picks up files left by a setup run under a different TLD.
fn managed_resolver_files(dir: &Path, tld: &str, include_current: bool) -> Vec<PathBuf> {
    let current = dir.join(tld);
    let mut found = if include_current {
        vec![current.clone()]
    } else {
        vec![]
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    let mut others: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p != &current && p.is_file() && is_managed_file(p, false))
        .collect();
    others.sort();
    found.extend(others);
    found
}

/// Where the record of the last successful setup lives.
fn record_path() -> PathBuf {
    crate::env::PITCHFORK_STATE_DIR
        .join("proxy")
        .join("setup.toml")
}

/// Every setup whose resources may still be installed.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct SetupRecords {
    #[serde(default)]
    setups: Vec<SetupContext>,
}

/// How many past setups are remembered.
///
/// Each entry is one configuration whose resources might still be installed.
/// A machine reaches a handful; the cap only stops a pathological loop from
/// growing the file without limit.
const MAX_RECORDS: usize = 32;

/// What two contexts have to agree on to count as the same installation.
///
/// Compared by the undo plan rather than field by field, because that is
/// exactly the set of resources at stake.
fn undo_key(ctx: &SetupContext) -> String {
    plan_undo(ctx).describe().join("\n")
}

/// Record a setup alongside the ones before it, for a later `--undo`.
///
/// Written before the steps run, so a run that fails halfway still leaves
/// something to reverse. Best-effort: failing to record must not stop setup,
/// because undo falls back to the current settings.
///
/// Earlier setups are kept, not replaced. Running setup again after changing
/// `proxy.port` installs a second redirect, and only the earlier record names
/// the first one.
/// Holds the setup lock for as long as it is alive.
pub struct SetupLock(#[allow(dead_code)] Option<xx::fslock::LockFile>);

/// Take the lock that serialises a whole `proxy setup` run.
///
/// It has to span the entire transaction, not just one step. The CLI reads the
/// records, builds a plan from them, writes the new record, applies the plan
/// and, on undo, clears the record. Locking only the apply would still let a
/// second run read the same records, build a plan against a state the first run
/// is about to change, and then apply it — including splicing a block into
/// `/etc/pf.conf` or a resolver drop-in from a stale pre-image, so one run's
/// edit is silently dropped while both report success.
///
/// A lock that cannot be taken is an error rather than a warning to run on
/// through. Every step past this point edits shared system state — a resolver
/// file, a firewall rule, the trust store — and this repository's rule is that
/// the state file is always locked. Applying those edits unserialised because
/// the lock was unavailable is the one outcome worse than not applying them.
/// The record file is the lock's *name*, not the file that gets locked:
/// `xx::fslock` flocks a file of its own under the temporary directory, keyed
/// on a hash of this path. So `clear_record` unlinking the record after a
/// successful undo does not drop or orphan the lock, and a run waiting on it
/// still holds the same one.
pub fn lock_setup() -> miette::Result<SetupLock> {
    let path = record_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| miette::miette!("Could not create {}: {e}", parent.display()))?;
    }
    // Blocks while another run holds it, so a concurrent `proxy setup` waits
    // its turn rather than failing.
    let lock = xx::fslock::get(&path, false).map_err(|e| {
        miette::miette!(
            "Could not lock {}: {e}\n\
             `pitchfork proxy setup` changes system configuration and will not \
             run without the lock.",
            path.display()
        )
    })?;
    Ok(SetupLock(lock))
}

/// Record what this configuration set up, for a later `--undo`.
///
/// The caller holds the lock from [`lock_setup`]; this does not take it again,
/// because the file lock is not re-entrant.
pub fn save_record(ctx: &SetupContext) {
    let path = record_path();
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let records = SetupRecords {
        setups: merged_records(load_records(), ctx),
    };

    let Ok(text) = toml::to_string_pretty(&records) else {
        return;
    };
    // Through `write_file`, so the journal lands whole or not at all. This is
    // the only thing that knows which resources were really installed, as
    // against what the settings say now; a truncated file reads as "nothing
    // recorded" or parses as a shorter list, and either way `--undo` loses
    // track of a resolver file, a redirect, a capability grant or a trusted CA
    // that nothing else can find again.
    if let Err(e) = write_file(&path, &text, false) {
        log::debug!(
            "Could not record the setup state at {}: {e}",
            path.display()
        );
    }
}

/// `existing` with `ctx` added, oldest first.
///
/// An identical configuration replaces its earlier entry; a different one is
/// appended. Nothing is dropped for failing, because a setup that fails
/// part-way may still have installed something, and the entries it would have
/// discarded name resources an earlier run really did install.
fn merged_records(existing: Vec<SetupContext>, ctx: &SetupContext) -> Vec<SetupContext> {
    let key = undo_key(ctx);
    let mut merged = existing;
    merged.retain(|r| undo_key(r) != key);
    merged.push(ctx.clone());
    if merged.len() > MAX_RECORDS {
        let excess = merged.len() - MAX_RECORDS;
        merged.drain(..excess);
    }
    merged
}

/// A record as read from disk, with everything privileged rebuilt from source.
///
/// The record decides which resources undo removes, and its fields reach `sudo`
/// as file paths. It is an ordinary file in the state directory, so it is
/// treated as input rather than as truth: the TLD is validated exactly as a
/// configured one is, and the fixed system paths are restored from the values
/// this build uses. A record naming something else cannot then steer a
/// privileged write or delete to it.
///
/// `binary` is rebuilt too. Keeping it looked reasonable — revoking from a
/// binary that has since moved is the kind of thing a record is for — but the
/// only guard on that step reads the target's current capabilities, which says
/// what a file has and not who granted it. A record naming an unrelated
/// executable that happens to hold `cap_net_bind_service` would have had undo
/// strip it under `sudo`. Undo now revokes from the running pitchfork only, so
/// a capability granted to a copy that has since been replaced is left behind
/// rather than guessed at.
fn sanitize_record(mut ctx: SetupContext) -> Option<SetupContext> {
    if let Err(e) = validate_tld(&ctx.tld) {
        log::warn!("Ignoring a setup record with an unusable proxy.tld: {e}");
        return None;
    }
    let fixed = fixed_paths();
    ctx.resolver_dir = fixed.resolver_dir;
    ctx.resolved_dropin_dir = fixed.resolved_dropin_dir;
    ctx.pf_conf = fixed.pf_conf;
    ctx.pf_anchor = fixed.pf_anchor;
    ctx.generated_ca = default_generated_ca();
    ctx.binary = current_binary();
    // Every one of these lands in an argv on undo. A value beginning with `-`
    // would be read as a flag by `networksetup` or `gsettings` rather than as
    // data, and a state outside the vocabulary of the tool it is handed to is
    // not something this file wrote. Drop the entry rather than the record: an
    // unusable restore should not cost the rest of the undo.
    ctx.prior_auto_proxy.retain(|p| {
        let plain = |v: &str| !v.is_empty() && !v.starts_with('-');
        let known_state = matches!(p.state.as_str(), "on" | "off" | "none" | "auto" | "manual");
        let usable_url = p.url.starts_with("http://")
            || p.url.starts_with("https://")
            || p.url.starts_with("file://");
        let ok = plain(&p.target) && plain(&p.url) && known_state && usable_url;
        if !ok {
            log::warn!(
                "Ignoring an unusable recorded automatic-proxy value for {:?}",
                p.target
            );
        }
        ok
    });
    Some(ctx)
}

/// The running pitchfork executable, which is the only binary undo will touch.
fn current_binary() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("pitchfork"))
}

/// The system paths setup writes to, which are fixed rather than configured.
struct FixedPaths {
    resolver_dir: PathBuf,
    resolved_dropin_dir: PathBuf,
    pf_conf: PathBuf,
    pf_anchor: PathBuf,
}

fn fixed_paths() -> FixedPaths {
    FixedPaths {
        resolver_dir: PathBuf::from("/etc/resolver"),
        resolved_dropin_dir: PathBuf::from("/etc/systemd/resolved.conf.d"),
        pf_conf: PathBuf::from("/etc/pf.conf"),
        pf_anchor: PathBuf::from("/etc/pf.anchors/pitchfork"),
    }
}

/// Every recorded setup, oldest first.
pub fn load_records() -> Vec<SetupContext> {
    let Ok(text) = std::fs::read_to_string(record_path()) else {
        return vec![];
    };
    // A file in the previous single-context format parses as `SetupRecords`
    // with no entries, because TOML ignores keys the struct does not name and
    // `setups` defaults to empty. An empty list therefore means "try the older
    // format" rather than "nothing was recorded".
    if let Ok(records) = toml::from_str::<SetupRecords>(&text)
        && !records.setups.is_empty()
    {
        return records
            .setups
            .into_iter()
            .filter_map(sanitize_record)
            .collect();
    }
    match toml::from_str::<SetupContext>(&text) {
        Ok(ctx) => sanitize_record(ctx).into_iter().collect(),
        // Genuinely empty, or unreadable; either way there is nothing to undo
        // from it.
        Err(e) => {
            log::debug!("No usable setup record: {e}");
            vec![]
        }
    }
}

/// Forget the recorded setup, once it has been undone.
pub fn clear_record() {
    let path = record_path();
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        log::debug!("Could not clear {}: {e}", path.display());
    }
}

/// The undo plan, reversing what setup recorded and what the settings imply.
///
/// The record is authoritative: it names the resources actually installed. The
/// current context is included as well, so a setup performed before records
/// existed, or changed by hand since, is still cleaned up. Steps are
/// deduplicated by their description, which names the path or rule each one
/// acts on.
/// `plan_undo_all`, with the current settings optional.
///
/// They are left out when `proxy.tld` no longer validates: paths built from it
/// are exactly what `validate_tld` refuses to let near a privileged write, and
/// the records were validated when they were written, so undo still works from
/// those alone.
pub fn plan_undo_from(current: Option<&SetupContext>, recorded: &[SetupContext]) -> Plan {
    let mut plan = Plan::default();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Newest first, so the most recent installation is cleaned up before the
    // ones it superseded.
    for ctx in recorded.iter().rev().chain(current) {
        let part = plan_undo(ctx);
        for step in part.steps {
            if seen.insert(step.summary.clone()) {
                plan.steps.push(step);
            }
        }
        for note in part.manual {
            if !plan.manual.contains(&note) {
                plan.manual.push(note);
            }
        }
    }
    plan
}

/// The setup plan, preceded by removal of anything an earlier setup installed
/// that this one supersedes.
///
/// Changing `proxy.port` and running setup again would otherwise leave the old
/// redirect in place beside the new one. That is worse than a leak: with two
/// iptables rules for the same port the first one wins, so traffic keeps going
/// to the port that is no longer in use.
pub fn plan_with_reconcile(ctx: &SetupContext, recorded: &[SetupContext]) -> Plan {
    let forward = plan(ctx);
    // What this configuration wants in place afterwards, including anything it
    // finds already done. Matched by resource rather than by description,
    // because installing and removing the same thing read quite differently:
    // comparing descriptions once let a re-run untrust the CA it still needed.
    let wanted: std::collections::HashSet<&Resource> = forward
        .steps
        .iter()
        .filter_map(|s| s.resource.as_ref())
        .collect();

    let mut reconciled = Plan::default();
    let mut seen: std::collections::HashSet<Resource> = std::collections::HashSet::new();
    for old in recorded {
        for step in plan_undo(old).steps {
            let Some(resource) = step.resource.clone() else {
                // Acts on nothing durable, so there is nothing to reconcile and
                // running it would only be churn.
                continue;
            };
            if wanted.contains(&resource) || !seen.insert(resource) {
                continue;
            }
            reconciled.steps.push(step);
        }
    }
    reconciled.steps.extend(forward.steps);
    reconciled.manual = forward.manual;
    reconciled
}

/// Build the plan for `pitchfork proxy setup --undo`.
///
/// Every step `plan` can take has a counterpart here; steps whose forward
/// version did nothing are simply absent.
pub fn plan_undo(ctx: &SetupContext) -> Plan {
    let mut plan = Plan::default();

    // Mirrors `plan_resolver`, so the undo plan claims a resolver file only
    // where that configuration would have written one. Claiming it otherwise
    // both lists a privileged removal that never applied and, on a re-run,
    // offers the file up as something the new configuration has superseded.
    // `pac` is part of that test because `plan` runs `plan_pac` *instead of*
    // `plan_resolver`: a PAC setup points the system at a proxy URL and never
    // touches the resolver, so it has no resolver file to take back.
    let wrote_resolver = ctx.dns_enabled && !ctx.lan && !ctx.pac;
    let wrote_dropin =
        wrote_resolver && ctx.systemd_resolved && !ctx.tld.eq_ignore_ascii_case("localhost");

    match ctx.platform {
        Platform::MacOs => {
            // Every resolver file we wrote, not just the one for the TLD
            // configured right now: changing `proxy.tld` after a setup would
            // otherwise orphan the earlier file for good. The managed-header
            // check on removal keeps this off anyone else's files.
            // The scan finds files carrying our header, which is evidence they
            // were written; the current TLD's path is added only when this
            // configuration would have written it.
            for path in managed_resolver_files(&ctx.resolver_dir, &ctx.tld, wrote_resolver) {
                plan.steps.push(Step {
                    summary: format!("remove {}", path.display()),
                    resource: Some(Resource::File(path.clone())),
                    action: Action::RemoveFile {
                        path,
                        sudo: true,
                        still_referenced_by: None,
                    },
                });
            }
        }
        Platform::Linux if wrote_dropin => {
            plan.steps.push(Step {
                summary: format!("remove {}", ctx.resolved_dropin().display()),
                action: Action::RemoveFile {
                    path: ctx.resolved_dropin(),
                    sudo: true,
                    still_referenced_by: None,
                },
                resource: Some(Resource::File(ctx.resolved_dropin())),
            });
            if ctx.systemd_resolved {
                plan.steps.push(Step {
                    summary: "restart systemd-resolved to drop the route".to_string(),
                    action: Action::Run {
                        argv: vec![
                            "systemctl".into(),
                            "restart".into(),
                            "systemd-resolved".into(),
                        ],
                        sudo: true,
                        skip_if: None,
                    },
                    resource: Some(Resource::ServiceReload("systemd-resolved".into())),
                });
            }
        }
        Platform::Linux | Platform::Other => {}
    }

    // Ports. Mirrors what `plan_ports` would have installed for this context,
    // so the undo plan is a faithful inverse rather than a superset: a
    // reconciling re-run reads it as "what that configuration installed", and
    // a step for something never installed would be listed and, worse, treated
    // as a resource the new run has to reverse.
    let granted_capability = ctx.proxy_port < 1024;
    let installed_redirect = !granted_capability && ctx.needs_port_redirect() && !ctx.pac;
    match ctx.platform {
        Platform::MacOs if installed_redirect => {
            plan.steps.push(Step {
                summary: format!("remove the pitchfork anchor from {}", ctx.pf_conf.display()),
                action: Action::RemoveBlock {
                    path: ctx.pf_conf.clone(),
                    sudo: true,
                },
                resource: Some(Resource::Block(ctx.pf_conf.clone())),
            });
            plan.steps.push(Step {
                summary: format!("remove {}", ctx.pf_anchor.display()),
                action: Action::RemoveFile {
                    path: ctx.pf_anchor.clone(),
                    sudo: true,
                    // Only once `pf.conf` no longer names it.
                    still_referenced_by: Some(ctx.pf_conf.clone()),
                },
                resource: Some(Resource::File(ctx.pf_anchor.clone())),
            });
            plan.steps.push(Step {
                summary: format!("reload pf rules from {}", ctx.pf_conf.display()),
                action: Action::Run {
                    argv: vec![
                        "pfctl".into(),
                        "-f".into(),
                        ctx.pf_conf.to_string_lossy().into_owned(),
                    ],
                    sudo: true,
                    skip_if: None,
                },
                resource: Some(Resource::ServiceReload("pf".into())),
            });
        }
        Platform::Linux => {
            if installed_redirect {
                plan.steps.push(Step {
                    // Names both ports: the plan should say which rule it will
                    // remove, and two setups that differ only in `proxy.port` must
                    // not look like the same step.
                    summary: format!(
                        "drop the iptables redirect from port {} to {}",
                        ctx.standard_port(),
                        ctx.proxy_port
                    ),
                    action: Action::RunIfPresent {
                        // `-C` succeeds only when that exact rule exists, so undo
                        // neither fails nor deletes a rule shaped like ours but
                        // added by someone else.
                        probe: Probe::status(iptables_redirect_argv(
                            "-C",
                            ctx.standard_port(),
                            ctx.proxy_port,
                        )),
                        argv: iptables_redirect_argv("-D", ctx.standard_port(), ctx.proxy_port),
                        sudo: true,
                    },
                    resource: Some(Resource::Redirect {
                        from: ctx.standard_port(),
                        to: ctx.proxy_port,
                    }),
                });
            }
            if granted_capability {
                plan.steps.push(Step {
                    summary: format!("revoke cap_net_bind_service from {}", ctx.binary.display()),
                    action: Action::RevokeBindCapability {
                        binary: ctx.binary.clone(),
                    },
                    resource: Some(Resource::BindCapability(ctx.binary.clone())),
                });
            }
        }
        Platform::MacOs | Platform::Other => {}
    }

    // PAC.
    match ctx.platform {
        Platform::MacOs => {
            for service in &ctx.network_services {
                plan.steps.push(Step {
                    // Names the URL for the same reason the iptables step
                    // names both ports.
                    summary: format!(
                        "turn off the automatic proxy URL {} for \"{service}\"",
                        ctx.pac_url()
                    ),
                    action: Action::RunIfPresent {
                        // Only if the service still points at pitchfork's PAC
                        // file. A corporate or hand-configured proxy URL is
                        // left exactly as it is.
                        probe: Probe::output(
                            vec![
                                "networksetup".into(),
                                "-getautoproxyurl".into(),
                                service.clone(),
                            ],
                            ctx.pac_url(),
                        ),
                        argv: vec![
                            "networksetup".into(),
                            "-setautoproxystate".into(),
                            service.clone(),
                            "off".into(),
                        ],
                        sudo: false,
                    },
                    resource: Some(Resource::AutoProxy {
                        service: service.clone(),
                        url: ctx.pac_url(),
                    }),
                });
                // Put back what was there before, when setup recorded it.
                //
                // The URL goes first and the switch last, because
                // `-setautoproxyurl` turns the switch on as a side effect:
                // writing the URL after the switch would re-enable a proxy the
                // user had deliberately left off. Setting the state last makes
                // the recorded value the one that survives either way.
                //
                // Each step is guarded by what the one before it leaves
                // behind: the URL step runs only while the service still points
                // at pitchfork's PAC file, and the switch step only once the
                // URL is the recorded one, which is the proof that the URL step
                // ran.
                for prior in ctx.prior_auto_proxy.iter().filter(|p| &p.target == service) {
                    let geturl = vec![
                        "networksetup".into(),
                        "-getautoproxyurl".into(),
                        service.clone(),
                    ];
                    for (summary, expect, argv) in [
                        (
                            format!(
                                "restore the automatic proxy URL for \"{service}\" to {}",
                                prior.url
                            ),
                            ctx.pac_url(),
                            vec![
                                "networksetup".into(),
                                "-setautoproxyurl".into(),
                                service.clone(),
                                prior.url.clone(),
                            ],
                        ),
                        (
                            format!(
                                "restore the automatic proxy switch for \"{service}\" to {}",
                                prior.state
                            ),
                            prior.url.clone(),
                            vec![
                                "networksetup".into(),
                                "-setautoproxystate".into(),
                                service.clone(),
                                prior.state.clone(),
                            ],
                        ),
                    ] {
                        plan.steps.push(Step {
                            summary,
                            action: Action::RunIfPresent {
                                probe: Probe::output(geturl.clone(), expect),
                                argv,
                                sudo: false,
                            },
                            resource: None,
                        });
                    }
                }
            }
        }
        Platform::Linux if ctx.gnome => {
            plan.steps.push(Step {
                summary: "switch the GNOME proxy mode back to none".to_string(),
                action: Action::RunIfPresent {
                    // Same care as on macOS: leave a proxy URL we did not set.
                    probe: Probe::output(
                        vec![
                            "gsettings".into(),
                            "get".into(),
                            "org.gnome.system.proxy".into(),
                            "autoconfig-url".into(),
                        ],
                        ctx.pac_url(),
                    ),
                    argv: vec![
                        "gsettings".into(),
                        "set".into(),
                        "org.gnome.system.proxy".into(),
                        "mode".into(),
                        "none".into(),
                    ],
                    sudo: false,
                },
                resource: Some(Resource::AutoProxy {
                    service: "gnome".into(),
                    url: ctx.pac_url(),
                }),
            });
            // Same ordering and the same guard chain as macOS: the URL while
            // it is still ours, then the mode once the URL proves that ran.
            for prior in ctx.prior_auto_proxy.iter().filter(|p| p.target == "gnome") {
                let geturl = vec![
                    "gsettings".into(),
                    "get".into(),
                    "org.gnome.system.proxy".into(),
                    "autoconfig-url".into(),
                ];
                for (summary, expect, key, value) in [
                    (
                        format!("restore the GNOME automatic proxy URL to {}", prior.url),
                        ctx.pac_url(),
                        "autoconfig-url",
                        prior.url.clone(),
                    ),
                    (
                        format!("restore the GNOME proxy mode to {}", prior.state),
                        prior.url.clone(),
                        "mode",
                        prior.state.clone(),
                    ),
                ] {
                    plan.steps.push(Step {
                        summary,
                        action: Action::RunIfPresent {
                            probe: Probe::output(geturl.clone(), expect),
                            argv: vec![
                                "gsettings".into(),
                                "set".into(),
                                "org.gnome.system.proxy".into(),
                                key.into(),
                                value,
                            ],
                            sudo: false,
                        },
                        resource: None,
                    });
                }
            }
        }
        _ => {}
    }

    // CA, last: the resolver is useless without it but harmless with it.
    //
    // Gated on HTTPS, matching `plan_ca`: a configuration serving plain HTTP
    // installed no CA, so claiming the resource would let a re-run remove one
    // that something else — an earlier HTTPS setup, or the user by hand — put
    // there.
    //
    // Not gated on `proxy.tls_cert`, which is a different question. Setting a
    // custom certificate after a setup does not untrust the CA that setup
    // installed, and gating on it left the root CA and its private key trusted
    // with no command able to remove them. The step probes the trust store
    // instead and skips when there is nothing there.
    // An empty path would name no certificate at all, so there is nothing to
    // plan; records always carry one, defaulted on read when absent.
    if ctx.https && !ctx.generated_ca.as_os_str().is_empty() {
        plan.steps.push(Step {
            summary: format!(
                "remove the pitchfork CA at {} from the system trust store",
                ctx.generated_ca.display()
            ),
            action: Action::UntrustCa {
                path: ctx.generated_ca.clone(),
                sudo: ctx.platform == Platform::Linux,
            },
            resource: Some(Resource::TrustedCa(ctx.generated_ca.clone())),
        });
    }

    plan
}

// ─── Execution ───────────────────────────────────────────────────────────────

/// Whether the current process is already root, in which case `sudo` is
/// unnecessary (and may not even be installed).
fn is_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid is always safe to call and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Run a command, prefixing `sudo` when the step needs privileges we lack.
fn run(argv: &[String], sudo: bool) -> Result<()> {
    let (program, args): (&str, &[String]) = if sudo && !is_root() {
        ("sudo", argv)
    } else {
        (argv[0].as_str(), &argv[1..])
    };
    let status = std::process::Command::new(program)
        .args(args)
        .status()
        .map_err(|e| miette::miette!("Failed to run `{}`: {e}", argv.join(" ")))?;
    if !status.success() {
        miette::bail!(
            "`{}` failed with exit code {}",
            argv.join(" "),
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

/// Write `content` to `path`, elevating if needed.
///
/// The privileged path pipes through `tee` rather than writing directly so that
/// only the write itself runs as root.
/// Write `path` so that it is either wholly the old content or wholly the new.
///
/// Every file this touches is shared system configuration: `/etc/pf.conf`, a
/// `/etc/resolver` entry, a systemd-resolved drop-in. Writing in place would
/// truncate the destination the moment it opened — `tee` does this too — so a
/// declined sudo prompt, a hangup or a full disk part-way through would leave
/// a half-written file that the system still reads. Writing a sibling
/// temporary file and renaming it over the destination makes the swap atomic,
/// because a rename within a directory either happens or does not.
fn write_file(path: &Path, content: &str, sudo: bool) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| miette::miette!("{} has no parent directory", path.display()))?;
    // A sibling, so the rename stays within one filesystem. The pid keeps two
    // processes from sharing one temporary file.
    let tmp = parent.join(format!(
        ".{}.pitchfork-{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string()),
        std::process::id()
    ));

    if !sudo || is_root() {
        std::fs::create_dir_all(parent)
            .map_err(|e| miette::miette!("Failed to create {}: {e}", parent.display()))?;
        std::fs::write(&tmp, content).map_err(|e| {
            // A partial write leaves the temporary behind, and these land in
            // system config directories where a stray file is not inert:
            // anything in `/etc/resolver` is read as a resolver file.
            let _ = std::fs::remove_file(&tmp);
            miette::miette!("Failed to write {}: {e}", tmp.display())
        })?;
        return std::fs::rename(&tmp, path).map_err(|e| {
            // The destination still holds whatever it held before.
            let _ = std::fs::remove_file(&tmp);
            miette::miette!("Failed to replace {}: {e}", path.display())
        });
    }

    run(
        &[
            "mkdir".into(),
            "-p".into(),
            parent.to_string_lossy().into_owned(),
        ],
        true,
    )?;

    let write_tmp = || -> Result<()> {
        use std::io::Write;
        let mut child = std::process::Command::new("sudo")
            .arg("tee")
            .arg(&tmp)
            .stdout(std::process::Stdio::null())
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| miette::miette!("Failed to write {} via sudo: {e}", path.display()))?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| miette::miette!("Failed to open stdin for sudo tee"))?
            .write_all(content.as_bytes())
            .map_err(|e| miette::miette!("Failed to write {}: {e}", path.display()))?;
        let status = child
            .wait()
            .map_err(|e| miette::miette!("Failed to write {}: {e}", path.display()))?;
        if !status.success() {
            miette::bail!(
                "Failed to write {}: sudo tee exited nonzero",
                path.display()
            );
        }
        // `tee` creates the temporary file under root's umask, and the rename
        // carries that mode to the destination. These are files the whole
        // system reads, so say so rather than inherit whatever it happened to
        // be.
        run(
            &[
                "chmod".into(),
                "644".into(),
                tmp.to_string_lossy().into_owned(),
            ],
            true,
        )?;
        // `-f` because the destination exists in the ordinary case; this is
        // the atomic swap.
        run(
            &[
                "mv".into(),
                "-f".into(),
                tmp.to_string_lossy().into_owned(),
                path.to_string_lossy().into_owned(),
            ],
            true,
        )
    };

    write_tmp().inspect_err(|_| {
        // Leave no half-written temporary behind. The destination is untouched
        // either way, which is the point of writing beside it.
        let _ = run(
            &["rm".into(), "-f".into(), tmp.to_string_lossy().into_owned()],
            true,
        );
    })
}

/// Whether `path` is a file pitchfork wrote, identified by its managed header.
///
/// A file we cannot read counts as not ours: `--undo` declines to delete
/// anything it cannot positively identify.
fn is_managed_file(path: &Path, sudo: bool) -> bool {
    read_maybe_privileged(path, sudo)
        .map(|c| c.contains(OWNED_HEADER))
        .unwrap_or(false)
}

/// Run a probe: succeed, and optionally match `expect` in its output.
fn probe_matches(probe: &Probe, sudo: bool) -> bool {
    let argv = &probe.argv;
    let expect = &probe.expect;
    let (program, args): (&str, &[String]) = if sudo && !is_root() {
        ("sudo", argv)
    } else {
        (argv[0].as_str(), &argv[1..])
    };
    let Ok(out) = std::process::Command::new(program)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    expect.iter().all(|e| match e {
        ProbeExpect::Contains(text) => stdout.contains(text.as_str()),
    })
}

/// Delete `path`, elevating if needed. A missing file is not an error.
///
/// Refuses to delete a file that does not carry pitchfork's managed header, so
/// an `/etc/resolver/<tld>` an administrator wrote by hand survives `--undo`.
fn remove_file(path: &Path, sudo: bool) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if !is_managed_file(path, sudo) {
        // Declining to delete somebody else's file is the right outcome, not an
        // error: `apply` reports it as skipped so an otherwise clean undo still
        // succeeds.
        return Ok(());
    }
    if !sudo || is_root() {
        return std::fs::remove_file(path)
            .map_err(|e| miette::miette!("Failed to remove {}: {e}", path.display()));
    }
    run(
        &[
            "rm".into(),
            "-f".into(),
            path.to_string_lossy().into_owned(),
        ],
        true,
    )
}

/// The leading keyword of a `pf.conf` line, if it has one.
///
/// Compared whole rather than by prefix: custom rulesets routinely define
/// macros such as `nat_if = "en0"` or `pass_hosts = "{ ... }"`, and treating
/// those as rules would put the pitchfork block on the wrong side of the
/// ordering boundary and make `pfctl -f` reject the file.
fn pf_keyword(line: &str) -> Option<&str> {
    let first = line.split_whitespace().next()?;
    // A macro assignment written without spaces, `nat_if="en0"`.
    match first.split_once('=') {
        Some(_) => None,
        None => Some(first),
    }
}

/// Whether a `pf.conf` line is a translation rule, which must precede filters.
fn is_pf_translation(line: &str) -> bool {
    pf_keyword(line).is_some_and(|kw| {
        matches!(
            kw,
            "rdr-anchor" | "nat-anchor" | "binat-anchor" | "rdr" | "nat" | "binat"
        )
    })
}

/// Whether a `pf.conf` line is a filter rule, which must follow translation.
///
/// `anchor` is a filter anchor; the translation anchors have their own
/// keywords and are matched by [`is_pf_translation`] first.
fn is_pf_filter(line: &str) -> bool {
    pf_keyword(line)
        .is_some_and(|kw| matches!(kw, "anchor" | "pass" | "block" | "match" | "antispoof"))
}

/// Insert or replace the pitchfork block in a `pf.conf`, respecting rule order.
///
/// pf requires translation rules (`rdr`) before filter rules, and Apple's stock
/// `/etc/pf.conf` ends with `anchor "com.apple/*"`, a filter anchor. Appending
/// there makes `pfctl -f` fail with "Rules must be in order", so the block goes
/// immediately after the last existing `rdr-anchor` line instead. With no such
/// line to order against, it falls back to appending.
pub(crate) fn splice_pf_block(text: &str, block: &str) -> String {
    // Strip any block we already placed first, so a re-run replaces it in the
    // right position instead of leaving the old one and appending a new one.
    let stripped = splice_block(text, "");
    if block.is_empty() {
        return stripped;
    }
    let mut out: Vec<&str> = stripped.lines().collect();
    // After the last translation rule if there is one. Otherwise before the
    // first filter rule, because appending past it would break the same
    // ordering requirement on a customised file that has filters but no
    // `rdr-anchor`. With neither, the end of the file is fine.
    let at = out
        .iter()
        .rposition(|l| is_pf_translation(l))
        .map(|i| i + 1)
        .or_else(|| out.iter().position(|l| is_pf_filter(l)))
        .unwrap_or(out.len());
    let managed = format!("{MARKER_START}\n{block}\n{MARKER_END}");
    out.insert(at, &managed);
    let mut joined = out.join("\n");
    if stripped.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// Replace the pitchfork-managed block in `text` with `block`, or append it.
///
/// Passing an empty `block` removes the block. Exposed for tests.
pub(crate) fn splice_block(text: &str, block: &str) -> String {
    let stripped = match (text.find(MARKER_START), text.find(MARKER_END)) {
        (Some(start), Some(end)) if end > start => {
            let end = end + MARKER_END.len();
            let mut out = String::with_capacity(text.len());
            out.push_str(&text[..start]);
            out.push_str(text[end..].trim_start_matches('\n'));
            out
        }
        _ => text.to_string(),
    };
    if block.is_empty() {
        let trimmed = stripped.trim_end();
        if trimmed.is_empty() {
            return String::new();
        }
        return format!("{trimmed}\n");
    }
    let body = stripped.trim_end();
    let prefix = if body.is_empty() {
        String::new()
    } else {
        format!("{body}\n\n")
    };
    format!("{prefix}{MARKER_START}\n{block}\n{MARKER_END}\n")
}

/// Read a file that may need privileges to read. Missing files read as empty.
fn read_maybe_privileged(path: &Path, sudo: bool) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(c) => Ok(c),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(_) if sudo && !is_root() => {
            let out = std::process::Command::new("sudo")
                .arg("cat")
                .arg(path)
                .output()
                .map_err(|e| miette::miette!("Failed to read {}: {e}", path.display()))?;
            if !out.status.success() {
                miette::bail!("Failed to read {}", path.display());
            }
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        Err(e) => Err(miette::miette!("Failed to read {}: {e}", path.display())),
    }
}

/// Whether a step's effect is already in place, so running it can be skipped.
fn already_done(action: &Action) -> bool {
    match action {
        Action::WriteFile { path, content, .. } => {
            std::fs::read_to_string(path).is_ok_and(|c| &c == content)
        }
        // Either it is gone, or it is not ours to remove.
        Action::RemoveFile { path, sudo, .. } => !path.exists() || !is_managed_file(path, *sudo),
        // For a pf.conf the block's position matters as much as its presence:
        // one left sitting after the filter anchor by an older version keeps
        // `pfctl -f` failing, so compare against what the splice would produce.
        Action::EnsureBlock {
            path,
            content,
            pf_order: true,
            ..
        } => std::fs::read_to_string(path).is_ok_and(|c| splice_pf_block(&c, content) == c),
        Action::EnsureBlock { path, content, .. } => std::fs::read_to_string(path)
            .is_ok_and(|c| c.contains(MARKER_START) && c.contains(content.as_str())),
        Action::RemoveBlock { path, .. } => {
            std::fs::read_to_string(path).is_ok_and(|c| !c.contains(MARKER_START))
        }
        Action::Note => true,
        // A command with no guard is either idempotent by construction or
        // cheap to repeat; one with a guard is skipped when the guard succeeds.
        Action::Run { skip_if, sudo, .. } => {
            skip_if.as_ref().is_some_and(|p| probe_matches(p, *sudo))
        }
        // Nothing of ours to revert means there is nothing to do.
        Action::RunIfPresent { probe, sudo, .. } => !probe_matches(probe, *sudo),
        Action::RevokeBindCapability { binary } => {
            revoke_already_done(file_capabilities(binary).as_deref())
        }
        Action::TrustCa { path } => crate::proxy::trust::is_ca_trusted(path),
        // Skipped only when the certificate is present and demonstrably not
        // trusted. Two absences are deliberately not enough. A missing PEM is
        // precisely the case `uninstall_cert` exists for: the certificate can
        // still be installed under its own name in the trust store. And a
        // trust store that would not answer says nothing either way, so
        // `ca_trust_state` is used rather than `is_ca_trusted`, which folds
        // that into `false`. Skipping on either would leave a trusted root
        // behind with no command left to remove it.
        Action::UntrustCa { path, .. } => {
            untrust_already_done(path.exists(), crate::proxy::trust::ca_trust_state(path))
        }
    }
}

/// Whether removing the CA from the trust store has nothing left to do.
///
/// Split out so every combination can be checked: the unknown only arises on
/// macOS, where `security verify-cert` can be killed for running too long, and
/// there is no way to provoke it from a test on another platform.
fn untrust_already_done(pem_exists: bool, trusted: Option<bool>) -> bool {
    // Only a positive "not trusted" counts. A missing PEM does not, because
    // that is exactly the case removal exists for: the certificate can still
    // be installed under its own name in the store. Nor does a store that
    // would not answer, which says nothing either way. Skipping on either
    // leaves a trusted root behind with no command left to remove it.
    pem_exists && trusted == Some(false)
}

/// Whether revoking the bind capability has nothing left to do.
///
/// Split out so both mistakes can be checked without depending on what a
/// particular `getcap` build prints. `None` is "could not ask", which settles
/// nothing and must not skip the step; `Some(&[])` is a file that definitely
/// carries no capabilities, which is the ordinary state after an upgrade
/// replaces the binary and must not fail the undo.
fn revoke_already_done(caps: Option<&[String]>) -> bool {
    matches!(caps, Some(caps) if !caps.iter().any(|c| c == BIND_CAPABILITY))
}

/// Why a step was skipped, for the line `apply` prints.
fn skip_reason(action: &Action) -> &'static str {
    match action {
        Action::RemoveFile { path, sudo, .. } if path.exists() && !is_managed_file(path, *sudo) => {
            "not written by pitchfork, left alone"
        }
        Action::RemoveFile { .. } => "nothing there",
        Action::RunIfPresent { .. } => "not configured by pitchfork",
        _ => "already done",
    }
}

/// Execute one step.
fn execute(step: &Step) -> Result<()> {
    match &step.action {
        Action::Note => Ok(()),
        Action::WriteFile {
            path,
            content,
            sudo,
        } => {
            // A file carrying our header is one pitchfork claims outright, so
            // writing it means taking the path over. `--undo` refuses to
            // delete anything without that header; setting up had no matching
            // check, so it would silently replace somebody else's file and
            // then decline to clean up after itself.
            //
            // `/etc/resolver/<tld>` is where this bites: the TLD is chosen by
            // the user, and `test` or `dev` is exactly what other local proxies
            // put there. Overwriting one of those with no copy kept is not
            // pitchfork's call to make.
            if content.contains(OWNED_HEADER) && path.exists() && !is_managed_file(path, *sudo) {
                return Err(miette::miette!(
                    "{} already exists and was not written by pitchfork, so it \
                     was left alone. Move it aside if it is no longer needed, \
                     or choose a different proxy.tld.",
                    path.display()
                ));
            }
            write_file(path, content, *sudo)
        }
        Action::RemoveFile {
            path,
            sudo,
            still_referenced_by,
        } => {
            if let Some(referrer) = still_referenced_by {
                // Fails closed. A referrer we cannot read — a declined sudo
                // prompt is the ordinary way that happens — is not evidence
                // that the reference is gone, and guessing wrong is exactly
                // what this guard exists to prevent: the anchor is deleted
                // while `/etc/pf.conf` still loads it, and every later
                // `pfctl -f` breaks, including the one at boot.
                //
                // A referrer that is not there at all is different: a file
                // that does not exist cannot reference anything.
                let blocked = if referrer.exists() {
                    match read_maybe_privileged(referrer, *sudo) {
                        Ok(contents) => contents
                            .contains(MARKER_START)
                            .then(|| format!("still names {}", path.display())),
                        Err(e) => Some(format!("could not be read ({e})")),
                    }
                } else {
                    None
                };
                if let Some(why) = blocked {
                    return Err(miette::miette!(
                        "{} {why}, so {} was left in place",
                        referrer.display(),
                        path.display()
                    ));
                }
            }
            remove_file(path, *sudo)
        }
        Action::EnsureBlock {
            path,
            content,
            sudo,
            pf_order,
            requires,
        } => {
            // Present *and* ours. Existence alone is not enough: the write
            // of the anchor refuses when something else already holds that
            // path, and the file it declined to replace would otherwise
            // satisfy this check. `/etc/pf.conf` would then carry
            // `load anchor "pitchfork" from "<somebody else's rules>"`, and
            // `pfctl` would load them — a worse outcome than the dangling
            // reference this guard was added for.
            if let Some(required) = requires
                && !is_managed_file(required, *sudo)
            {
                let why = if required.exists() {
                    "was not written by pitchfork"
                } else {
                    "does not exist"
                };
                return Err(miette::miette!(
                    "{} {why}, so the block naming it was not written to {}",
                    required.display(),
                    path.display()
                ));
            }
            let current = read_maybe_privileged(path, *sudo)?;
            let spliced = if *pf_order {
                splice_pf_block(&current, content)
            } else {
                splice_block(&current, content)
            };
            write_file(path, &spliced, *sudo)
        }
        Action::RemoveBlock { path, sudo } => {
            if !path.exists() {
                return Ok(());
            }
            let current = read_maybe_privileged(path, *sudo)?;
            write_file(path, &splice_block(&current, ""), *sudo)
        }
        Action::Run { argv, sudo, .. } | Action::RunIfPresent { argv, sudo, .. } => {
            run(argv, *sudo)
        }
        Action::RevokeBindCapability { binary } => revoke_bind_capability(binary),
        Action::TrustCa { path } => crate::proxy::trust::install_cert(path),
        Action::UntrustCa { path, sudo } => {
            if *sudo && !is_root() {
                // The Linux trust store is root-owned, so this re-invokes
                // pitchfork rather than failing on permissions.
                run(
                    &[
                        std::env::current_exe()
                            .unwrap_or_else(|_| PathBuf::from("pitchfork"))
                            .to_string_lossy()
                            .into_owned(),
                        "proxy".into(),
                        "untrust".into(),
                        "--cert".into(),
                        path.to_string_lossy().into_owned(),
                    ],
                    true,
                )
            } else {
                crate::proxy::trust::uninstall_cert(path)
            }
        }
    }
}

/// The capability `proxy setup` grants so an unprivileged proxy can bind 443.
const BIND_CAPABILITY: &str = "cap_net_bind_service";

/// Capability names currently set on `path`, or `None` if it has none or
/// `getcap` could not be run.
///
/// `getcap` prints `<path> cap_net_bind_service=ep`, or a comma-separated list
/// when a file carries several.
///
/// `Some` is a definite answer, including `Some(vec![])` for a file carrying
/// none. `None` means the question could not be asked: no `getcap` anywhere it
/// was looked for, or one that failed. Callers must not read that as "no
/// capability", because the two call for opposite actions.
fn file_capabilities(path: &Path) -> Option<Vec<String>> {
    // `getcap` lives in `/usr/sbin`, which is not on an ordinary user's PATH on
    // Debian and its derivatives. Granting goes through `sudo setcap`, whose
    // secure_path does include it, so without these fallbacks setup can grant a
    // capability that undo then cannot even see.
    let out = ["getcap", "/usr/sbin/getcap", "/sbin/getcap"]
        .into_iter()
        .find_map(|prog| {
            std::process::Command::new(prog)
                .arg(path)
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
        })?;
    if !out.status.success() {
        return None;
    }
    capabilities_from_getcap(
        path.to_string_lossy().as_ref(),
        &String::from_utf8_lossy(&out.stdout),
    )
}

/// Read `getcap`'s output for `path`.
///
/// `getcap` prints `<path> <capabilities>`. The path is known, so it is
/// stripped rather than guessed at by splitting, which keeps working when the
/// path itself contains spaces.
///
/// Separated from running the command so both readings of "no matching line"
/// can be tested, which is the distinction that matters here and the one a
/// live `getcap` cannot be made to demonstrate.
fn capabilities_from_getcap(path: &str, stdout: &str) -> Option<Vec<String>> {
    if let Some(line) = stdout.lines().find_map(|l| l.trim_end().strip_prefix(path)) {
        return Some(parse_capabilities(line));
    }
    // Two different things end up here.
    //
    // Nothing at all is how `getcap` reports a file that carries no
    // capabilities — the ordinary state after an upgrade replaces the binary,
    // or after someone cleared them by hand. That is a definite empty answer,
    // and reading it as unknown makes `--undo` refuse to finish where there is
    // nothing left to revoke.
    //
    // Output that does not name the path is not that. The path is matched
    // through `to_string_lossy`, so a binary whose path is not valid UTF-8
    // never matches what `getcap` printed back, and a future release could
    // word its output differently. Calling that "no capabilities" would have
    // undo skip the revocation and leave the binary able to bind privileged
    // ports, so it is reported as no answer instead.
    stdout.trim().is_empty().then(Vec::new)
}

/// Capability names in a `getcap` capability string.
///
/// The string is one or more whitespace-separated clauses, because capabilities
/// with different flag sets print separately: `cap_chown=ei
/// cap_net_bind_service=ep`. Reading only one clause would miss the rest, and
/// the rest is exactly what decides whether removing ours takes somebody
/// else's with it.
fn parse_capabilities(spec: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for clause in spec.split_whitespace() {
        // A clause is `names=flags`, `names+flags` or `names-flags`; a bare
        // `=` or a flags-only fragment carries no names.
        let Some(list) = clause.split(['=', '+', '-']).next() else {
            continue;
        };
        for name in list.split(',').map(str::trim) {
            if name.starts_with("cap_") && !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Remove `cap_net_bind_service` from `path`, leaving any others alone.
///
/// `setcap -r` clears the whole set, so it is used only when ours is the only
/// capability on the file. A binary carrying others was configured by
/// something else and pitchfork declines to guess at what it may discard.
fn revoke_bind_capability(path: &Path) -> Result<()> {
    // Not `Ok(())`. Reporting a revocation that never happened would leave the
    // binary able to bind privileged ports with `--undo` claiming otherwise.
    // Running `setcap -r` blind is not the answer either: it would clear
    // capabilities pitchfork did not grant, which is what the check below
    // exists to prevent.
    let Some(caps) = file_capabilities(path) else {
        miette::bail!(
            "Could not read the capabilities on {} — `getcap` was not found or failed, \
             so {BIND_CAPABILITY} was left in place.\n\
             Check with: sudo getcap {}\n\
             Remove with: sudo setcap -r {}",
            path.display(),
            path.display(),
            path.display()
        );
    };
    if !caps.iter().any(|c| c == BIND_CAPABILITY) {
        return Ok(());
    }
    let others: Vec<&String> = caps.iter().filter(|c| *c != BIND_CAPABILITY).collect();
    if !others.is_empty() {
        miette::bail!(
            "{} also carries {}, which pitchfork did not grant. \
             Removing {BIND_CAPABILITY} here would clear those too, so it has been left alone.\n\
             Remove it by hand with: sudo setcap {}=ep {}",
            path.display(),
            others
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            others
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(","),
            path.display()
        );
    }
    run(
        &[
            "setcap".into(),
            "-r".into(),
            path.to_string_lossy().into_owned(),
        ],
        true,
    )
}

/// Outcome of running a plan, for reporting.
#[derive(Debug, Default)]
pub struct RunReport {
    pub applied: usize,
    pub skipped: usize,
    pub failed: Vec<(String, String)>,
}

/// Run every step, skipping those already in effect.
///
/// A failing step is recorded and the rest still run: a resolver file that
/// cannot be written should not stop the CA from being installed.
pub fn apply(plan: &Plan) -> RunReport {
    let mut report = RunReport::default();
    for step in &plan.steps {
        if step.action == Action::Note {
            continue;
        }
        if already_done(&step.action) {
            println!("  · {} ({})", step.summary, skip_reason(&step.action));
            report.skipped += 1;
            continue;
        }
        print!("  → {} ... ", step.summary);
        use std::io::Write;
        let _ = std::io::stdout().flush();
        match execute(step) {
            Ok(()) => {
                println!("ok");
                report.applied += 1;
            }
            Err(e) => {
                println!("failed");
                report.failed.push((step.summary.clone(), e.to_string()));
            }
        }
    }
    report
}

// ─── Environment detection ───────────────────────────────────────────────────

/// Whether systemd-resolved is the active stub resolver.
pub fn systemd_resolved_active() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    std::process::Command::new("systemctl")
        .args(["is-active", "--quiet", "systemd-resolved"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Major version of systemd, parsed from `systemctl --version`.
///
/// `None` when systemd is absent or the output cannot be read, in which case no
/// version-specific warning is given rather than a wrong one.
pub fn systemd_version() -> Option<u32> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let out = std::process::Command::new("systemctl")
        .arg("--version")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    // First line looks like `systemd 255 (255.4-1)`.
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Whether the GNOME proxy schema is present.
pub fn gnome_proxy_available() -> bool {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.system.proxy", "mode"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Active macOS network services, as `networksetup` names them.
/// Read the automatic-proxy configuration that is in place right now.
///
/// Called before `setup --pac` overwrites it, so `--undo` has something to put
/// back. A target already pointing at `our_url` is skipped: re-running setup
/// must not record pitchfork's own PAC file as the value to restore.
///
/// Anything unreadable is skipped rather than guessed at. A missing entry
/// means undo switches the proxy off, which is what it did before this was
/// recorded at all.
pub fn read_prior_auto_proxy(
    platform: Platform,
    services: &[String],
    gnome: bool,
    our_url: &str,
) -> Vec<PriorAutoProxy> {
    let run = |argv: &[&str]| -> Option<String> {
        let out = std::process::Command::new(argv[0])
            .args(&argv[1..])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let mut prior = vec![];
    match platform {
        Platform::MacOs => {
            for service in services {
                let Some(out) = run(&["networksetup", "-getautoproxyurl", service]) else {
                    continue;
                };
                let field = |name: &str| {
                    out.lines()
                        .filter_map(|l| l.split_once(':'))
                        .find(|(k, _)| k.trim().eq_ignore_ascii_case(name))
                        .map(|(_, v)| v.trim().to_string())
                };
                let url = field("URL").unwrap_or_default();
                // `(null)` is what `networksetup` prints for an unset URL.
                if url.is_empty() || url == "(null)" || url == our_url {
                    continue;
                }
                let enabled = field("Enabled").is_some_and(|v| v.eq_ignore_ascii_case("yes"));
                prior.push(PriorAutoProxy {
                    target: service.clone(),
                    url,
                    state: if enabled { "on" } else { "off" }.to_string(),
                });
            }
        }
        Platform::Linux if gnome => {
            let unquote = |v: String| v.trim().trim_matches('\'').to_string();
            let url = run(&[
                "gsettings",
                "get",
                "org.gnome.system.proxy",
                "autoconfig-url",
            ])
            .map(unquote)
            .unwrap_or_default();
            if !url.is_empty() && url != our_url {
                let mode = run(&["gsettings", "get", "org.gnome.system.proxy", "mode"])
                    .map(unquote)
                    .unwrap_or_else(|| "none".to_string());
                prior.push(PriorAutoProxy {
                    target: "gnome".to_string(),
                    url,
                    state: mode,
                });
            }
        }
        Platform::Linux | Platform::Other => {}
    }
    prior
}

pub fn macos_network_services() -> Vec<String> {
    if !cfg!(target_os = "macos") {
        return vec![];
    }
    let Ok(out) = std::process::Command::new("networksetup")
        .arg("-listallnetworkservices")
        .output()
    else {
        return vec![];
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip(1) // header line explaining the asterisk
        // A leading asterisk marks a disabled service.
        .filter(|l| !l.trim().is_empty() && !l.starts_with('*'))
        .map(|l| l.trim().to_string())
        .collect()
}

/// The address a local client uses to reach a proxy bound to `proxy_host`,
/// formatted for a URL.
///
/// A wildcard bind is reachable on the loopback address of its own family. An
/// IPv6 literal is bracketed, as a URL requires.
fn contact_host(proxy_host: &str) -> String {
    match proxy_host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(ip)) => {
            let ip = if ip.is_unspecified() {
                std::net::Ipv6Addr::LOCALHOST
            } else {
                ip
            };
            format!("[{ip}]")
        }
        Ok(std::net::IpAddr::V4(ip)) if ip.is_unspecified() => "127.0.0.1".to_string(),
        Ok(std::net::IpAddr::V4(ip)) => ip.to_string(),
        // Not an address at all; the proxy falls back to IPv4 loopback too.
        Err(_) => "127.0.0.1".to_string(),
    }
}

/// Build the context for the current machine and settings.
pub fn context_from_settings(s: &crate::settings::Settings, pac: bool) -> SetupContext {
    // The same source the records are rebuilt from, so a plan and an undo
    // of that plan always name the same files.
    let fixed = fixed_paths();
    let platform = Platform::current();
    let lan_enabled = s.proxy.lan || !s.proxy.lan_ip.is_empty();
    let tld = crate::proxy::effective_tld(s).to_string();
    let custom_cert = !s.proxy.tls_cert.is_empty();
    let ca_path = if custom_cert {
        PathBuf::from(&s.proxy.tls_cert)
    } else {
        crate::env::PITCHFORK_STATE_DIR.join("proxy").join("ca.pem")
    };
    let mut ctx = SetupContext {
        platform,
        tld,
        dns_port: super::dns::dns_port(s),
        proxy_port: u16::try_from(s.proxy.port).unwrap_or(443),
        https: s.proxy.https,
        dns_enabled: s.proxy.dns,
        pac,
        systemd_resolved: systemd_resolved_active(),
        systemd_version: systemd_version(),
        lan: lan_enabled,
        contact_host: contact_host(&s.proxy.host),
        ca_trusted: crate::proxy::trust::is_ca_trusted(&ca_path),
        ca_path,
        generated_ca: default_generated_ca(),
        custom_cert,
        binary: current_binary(),
        resolver_dir: fixed.resolver_dir,
        resolved_dropin_dir: fixed.resolved_dropin_dir,
        pf_conf: fixed.pf_conf,
        pf_anchor: fixed.pf_anchor,
        // Probed regardless of `pac`, because `--undo` has to be able to turn
        // off an automatic proxy URL that an earlier `--pac` run switched on.
        network_services: if platform == Platform::MacOs {
            macos_network_services()
        } else {
            vec![]
        },
        gnome: platform == Platform::Linux && gnome_proxy_available(),
        prior_auto_proxy: vec![],
    };
    // Only on the way in. `--undo` takes no `--pac`, and reads the value to
    // restore from the record rather than from the system it is about to
    // change.
    if pac {
        ctx.prior_auto_proxy =
            read_prior_auto_proxy(platform, &ctx.network_services, ctx.gnome, &ctx.pac_url());
    }
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(platform: Platform) -> SetupContext {
        SetupContext {
            platform,
            tld: "test".into(),
            dns_port: 15353,
            proxy_port: 8443,
            https: true,
            dns_enabled: true,
            pac: false,
            systemd_resolved: true,
            systemd_version: Some(255),
            lan: false,
            contact_host: "127.0.0.1".to_string(),
            ca_path: PathBuf::from("/state/proxy/ca.pem"),
            generated_ca: PathBuf::from("/state/proxy/ca.pem"),
            ca_trusted: false,
            custom_cert: false,
            binary: PathBuf::from("/usr/local/bin/pitchfork"),
            resolver_dir: PathBuf::from("/etc/resolver"),
            resolved_dropin_dir: PathBuf::from("/etc/systemd/resolved.conf.d"),
            pf_conf: PathBuf::from("/etc/pf.conf"),
            pf_anchor: PathBuf::from("/etc/pf.anchors/pitchfork"),
            network_services: vec![],
            gnome: false,
            prior_auto_proxy: vec![],
        }
    }

    #[test]
    fn a_tld_that_could_escape_the_resolver_directory_is_refused() {
        // These reach `/etc/resolver/<tld>` for a privileged write and delete,
        // and the systemd-resolved drop-in, so none may be planned at all.
        for bad in [
            "../../etc/passwd",
            "..",
            "a/b",
            "test\nDNS=8.8.8.8",
            "",
            ".test",
            "te st",
        ] {
            assert!(validate_tld(bad).is_err(), "expected {bad:?} to be refused");
        }
        for good in ["localhost", "test", "dev.internal", "my-tld"] {
            assert!(
                validate_tld(good).is_ok(),
                "expected {good:?} to be allowed"
            );
        }
    }

    #[test]
    fn lan_mode_leaves_the_mdns_local_namespace_alone() {
        // Routing `.local` at a unicast resolver would swallow every Bonjour
        // name on the network, not just pitchfork's.
        let mut c = ctx(Platform::MacOs);
        c.lan = true;
        c.tld = "local".into();
        let lines = plan(&c).describe();
        assert!(lines.iter().any(|l| l.contains("mDNS")));
        assert!(!lines.iter().any(|l| l.contains("/etc/resolver")));

        let mut c = ctx(Platform::Linux);
        c.lan = true;
        c.tld = "local".into();
        let lines = plan(&c).describe();
        assert!(!lines.iter().any(|l| l.contains("resolved.conf.d")));
    }

    #[test]
    fn the_sudo_ca_step_names_the_certificate_it_should_trust() {
        // sudo resets the environment, so a child that re-derived the path
        // could trust a different certificate than the plan promised.
        let c = ctx(Platform::Linux);
        let step = plan(&c)
            .steps
            .into_iter()
            .find(|s| s.summary.contains("trust store"))
            .expect("linux plans a CA install");
        let Action::Run { argv, .. } = step.action else {
            panic!("expected a command");
        };
        assert!(argv.contains(&"--cert".to_string()));
        assert!(argv.contains(&"/state/proxy/ca.pem".to_string()));
    }

    #[test]
    fn undo_only_reverts_proxy_settings_that_point_at_pitchfork() {
        let mut c = ctx(Platform::MacOs);
        c.network_services = vec!["Wi-Fi".into()];
        let step = plan_undo(&c)
            .steps
            .into_iter()
            .find(|s| s.summary.contains("automatic proxy URL"))
            .expect("undo turns the PAC URL off");
        let Action::RunIfPresent { probe, .. } = step.action else {
            panic!("expected a guarded command");
        };
        assert!(probe.argv.contains(&"-getautoproxyurl".to_string()));
        // Guarded on our own URL, so a corporate PAC survives undo.
        assert_eq!(
            probe.expect,
            vec![ProbeExpect::Contains(
                "http://127.0.0.1:8443/proxy.pac".to_string()
            )]
        );
    }

    #[test]
    fn undo_revokes_the_capability_through_a_checked_action() {
        // `setcap -r` clears every capability on the file, so the decision
        // needs the current set, not a yes/no probe.
        // Only a setup that granted it plans to revoke it: the undo plan is a
        // faithful inverse, not a list of everything that might be present.
        let mut privileged = ctx(Platform::Linux);
        privileged.proxy_port = 443;
        let step = plan_undo(&privileged)
            .steps
            .into_iter()
            .find(|s| s.summary.contains("cap_net_bind_service"))
            .expect("undo revokes the capability it granted");
        assert!(matches!(step.action, Action::RevokeBindCapability { .. }));

        // An unprivileged port never granted it, so undo leaves it alone.
        assert!(
            !plan_undo(&ctx(Platform::Linux))
                .describe()
                .iter()
                .any(|l| l.contains("cap_net_bind_service"))
        );
    }

    #[test]
    fn every_capability_clause_is_read_not_just_the_last() {
        // Capabilities with different flag sets print as separate clauses.
        // Reading one clause would either miss another capability and let
        // `setcap -r` clear it, or miss ours and leave it installed.
        assert_eq!(
            parse_capabilities("cap_net_bind_service=ep"),
            vec!["cap_net_bind_service"]
        );
        // Ours last: the earlier capability must still be seen.
        assert_eq!(
            parse_capabilities("cap_sys_admin=ei cap_net_bind_service=ep"),
            vec!["cap_sys_admin", "cap_net_bind_service"]
        );
        // Ours first: it must still be found.
        assert_eq!(
            parse_capabilities("cap_net_bind_service=ep cap_sys_admin=ei"),
            vec!["cap_net_bind_service", "cap_sys_admin"]
        );
        // Comma-separated within one clause, and the older `+ep` spelling.
        assert_eq!(
            parse_capabilities("cap_net_bind_service,cap_sys_admin=ep"),
            vec!["cap_net_bind_service", "cap_sys_admin"]
        );
        assert_eq!(
            parse_capabilities("cap_net_bind_service+ep"),
            vec!["cap_net_bind_service"]
        );
        // A leading empty base set, as `cap_to_text` can emit.
        assert_eq!(
            parse_capabilities("= cap_net_bind_service+ep"),
            vec!["cap_net_bind_service"]
        );
        assert!(parse_capabilities("").is_empty());

        // Only ours: safe to clear the whole set. Shared: undo must refuse.
        assert!(
            parse_capabilities("cap_net_bind_service=ep")
                .iter()
                .all(|c| c == BIND_CAPABILITY)
        );
        let shared = parse_capabilities("cap_sys_admin=ei cap_net_bind_service=ep");
        assert!(shared.iter().any(|c| c == BIND_CAPABILITY));
        assert!(shared.iter().any(|c| c != BIND_CAPABILITY));
    }

    #[test]
    fn macos_plan_covers_resolver_ca_and_port_redirect() {
        let c = ctx(Platform::MacOs);
        // Rendered with the host's path separator, so the expectation is built
        // the same way rather than spelled with a forward slash.
        let resolver = c.resolver_file().display().to_string();
        assert_eq!(
            plan(&c).describe(),
            vec![
                format!("[sudo] write {resolver} pointing *.test at 127.0.0.1:15353"),
                "install the pitchfork CA at /state/proxy/ca.pem into the system trust store"
                    .to_string(),
                "[sudo] write /etc/pf.anchors/pitchfork redirecting port 443 to 8443".to_string(),
                "[sudo] load the pitchfork anchor into /etc/pf.conf".to_string(),
                "[sudo] enable pf and load the new rules".to_string(),
            ]
        );
    }

    #[test]
    fn linux_plan_writes_a_resolved_dropin_and_sudoes_the_ca() {
        let c = ctx(Platform::Linux);
        // Rendered with the host's path separator, so the expectation is built
        // the same way rather than spelled with a forward slash.
        let dropin = c.resolved_dropin().display().to_string();
        let ca = c.ca_path.display().to_string();
        assert_eq!(
            plan(&c).describe(),
            vec![
                format!("[sudo] write {dropin} routing *.test to 127.0.0.1:15353"),
                "[sudo] restart systemd-resolved to pick up the route (interrupts DNS briefly)"
                    .to_string(),
                format!("[sudo] install the pitchfork CA at {ca} into the system trust store"),
                "[sudo] redirect loopback traffic for port 443 to 8443 (iptables)".to_string(),
            ]
        );
    }

    #[test]
    fn macos_says_what_taking_over_localhost_costs() {
        let mut c = ctx(Platform::MacOs);
        c.tld = "localhost".into();
        let p = plan(&c);
        assert!(
            p.manual
                .iter()
                .any(|m| m.contains("only while the supervisor is running")),
            "expected a note about the dependency: {:?}",
            p.manual
        );
        // A TLD that is not the machine's own keeps quiet.
        assert!(plan(&ctx(Platform::MacOs)).manual.is_empty());
    }

    #[test]
    fn linux_with_localhost_tld_needs_no_resolver_change() {
        let mut c = ctx(Platform::Linux);
        c.tld = "localhost".into();
        let lines = plan(&c).describe();
        assert!(lines[0].contains("systemd-resolved already answers *.localhost"));
        assert!(!lines[0].starts_with("[sudo]"));
    }

    #[test]
    fn linux_without_systemd_resolved_falls_back_to_dnsmasq_advice() {
        let mut c = ctx(Platform::Linux);
        c.systemd_resolved = false;
        let p = plan(&c);
        assert!(!p.describe().iter().any(|l| l.contains("resolved.conf.d")));
        assert!(p.manual[0].contains("dnsmasq"));
        assert!(p.manual[0].contains("server=/test/127.0.0.1#15353"));
    }

    #[test]
    fn pac_plan_needs_no_sudo_for_resolution_or_ports() {
        let mut c = ctx(Platform::MacOs);
        c.pac = true;
        c.ca_trusted = true;
        c.network_services = vec!["Wi-Fi".into()];
        let p = plan(&c);
        assert!(
            !p.needs_sudo(),
            "PAC setup must not require sudo: {:?}",
            p.describe()
        );
        assert!(p.describe().iter().any(|l| l.contains(
            "set the automatic proxy URL for \"Wi-Fi\" to http://127.0.0.1:8443/proxy.pac"
        )));
        // No port redirect: the PAC file names the proxy port directly.
        assert!(
            p.describe()
                .iter()
                .any(|l| l.contains("no port redirect is needed"))
        );
    }

    #[test]
    fn pac_does_not_excuse_a_privileged_port() {
        // The browser connects to `proxy.port` directly under PAC, so no
        // redirect is installed — but binding that port is a separate problem,
        // and the default is 443. Claiming `--pac` removes port-related sudo
        // would be wrong for the configuration most people start from.
        let mut c = ctx(Platform::Linux);
        c.pac = true;
        c.proxy_port = 443;
        c.ca_trusted = true;
        let lines = plan(&c).describe();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("[sudo]") && l.contains("cap_net_bind_service")),
            "expected the capability grant: {lines:?}"
        );

        // macOS cannot grant it at all, so setup says so rather than planning.
        let mut c = ctx(Platform::MacOs);
        c.pac = true;
        c.proxy_port = 443;
        c.ca_trusted = true;
        let p = plan(&c);
        assert!(!p.needs_sudo());
        assert!(
            p.manual
                .iter()
                .any(|m| m.contains("will not run the supervisor as root"))
        );
    }

    #[test]
    fn pac_on_linux_still_needs_sudo_only_for_the_ca() {
        // Being straight about the one exception: the Linux trust store is
        // root-owned, so HTTPS through the PAC file needs that one sudo step.
        // Nothing about resolution or ports does.
        let mut c = ctx(Platform::Linux);
        c.pac = true;
        c.gnome = true;
        let sudo_steps: Vec<String> = plan(&c)
            .describe()
            .into_iter()
            .filter(|l| l.starts_with("[sudo]"))
            .collect();
        assert_eq!(sudo_steps.len(), 1, "unexpected sudo steps: {sudo_steps:?}");
        assert!(sudo_steps[0].contains("trust store"));

        // With the CA already trusted, or without HTTPS, nothing needs sudo.
        let mut trusted = c.clone();
        trusted.ca_trusted = true;
        assert!(!plan(&trusted).needs_sudo());
        let mut plain = c.clone();
        plain.https = false;
        assert!(!plan(&plain).needs_sudo());
    }

    #[test]
    fn an_ipv6_proxy_host_is_advertised_as_an_ipv6_url() {
        // With `proxy.host = "::1"` nothing listens on IPv4, so a PAC file
        // naming 127.0.0.1 would hand the browser a dead address.
        assert_eq!(contact_host("::1"), "[::1]");
        assert_eq!(contact_host("::"), "[::1]");
        assert_eq!(contact_host("0.0.0.0"), "127.0.0.1");
        assert_eq!(contact_host("127.0.0.1"), "127.0.0.1");
        assert_eq!(contact_host("192.168.1.5"), "192.168.1.5");
        assert_eq!(contact_host("not-an-address"), "127.0.0.1");

        let mut c = ctx(Platform::MacOs);
        c.pac = true;
        c.contact_host = "[::1]".to_string();
        c.network_services = vec!["Wi-Fi".into()];
        assert!(
            plan(&c)
                .describe()
                .iter()
                .any(|l| l.contains("http://[::1]:8443/proxy.pac"))
        );
    }

    #[test]
    fn gnome_pac_plan_sets_the_autoconfig_url() {
        let mut c = ctx(Platform::Linux);
        c.pac = true;
        c.gnome = true;
        c.https = false;
        let lines = plan(&c).describe();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("GNOME automatic proxy URL"))
        );
        assert!(lines.iter().any(|l| l.contains("proxy mode to automatic")));
    }

    #[test]
    fn privileged_proxy_port_uses_setcap_on_linux_and_advice_on_macos() {
        let mut c = ctx(Platform::Linux);
        c.proxy_port = 443;
        assert!(
            plan(&c)
                .describe()
                .iter()
                .any(|l| l.contains("cap_net_bind_service"))
        );

        let mut c = ctx(Platform::MacOs);
        c.proxy_port = 443;
        let p = plan(&c);
        assert!(!p.describe().iter().any(|l| l.contains("pf.anchors")));
        assert!(p.manual[0].contains("will not run the supervisor as root"));
    }

    #[test]
    fn a_trusted_ca_and_a_custom_cert_both_skip_the_trust_step() {
        let mut c = ctx(Platform::MacOs);
        c.ca_trusted = true;
        assert!(
            plan(&c)
                .describe()
                .iter()
                .any(|l| l.contains("is already trusted"))
        );

        let mut c = ctx(Platform::MacOs);
        c.custom_cert = true;
        assert!(
            plan(&c)
                .describe()
                .iter()
                .any(|l| l.contains("installs no CA"))
        );
    }

    #[test]
    fn a_privileged_port_is_never_papered_over_with_a_redirect() {
        // Plain HTTP on 443: privileged, and not the standard port either. A
        // redirect cannot help — the supervisor still has to bind it.
        let mut c = ctx(Platform::Linux);
        c.https = false;
        c.proxy_port = 443;
        let lines = plan(&c).describe();
        assert!(lines.iter().any(|l| l.contains("cap_net_bind_service")));
        assert!(!lines.iter().any(|l| l.contains("iptables")));
    }

    #[test]
    fn plain_http_plans_no_ca_step_and_redirects_port_80() {
        let mut c = ctx(Platform::Linux);
        c.https = false;
        let lines = plan(&c).describe();
        assert!(!lines.iter().any(|l| l.contains("trust store")));
        assert!(lines.iter().any(|l| l.contains("port 80 to 8443")));
    }

    #[test]
    fn disabled_resolver_skips_resolver_steps() {
        let mut c = ctx(Platform::MacOs);
        c.dns_enabled = false;
        let lines = plan(&c).describe();
        assert!(lines[0].contains("proxy.dns is false"));
        assert!(!lines.iter().any(|l| l.contains("/etc/resolver")));
    }

    #[test]
    fn the_iptables_redirect_is_guarded_against_stacking_duplicates() {
        let c = ctx(Platform::Linux);
        let step = plan(&c)
            .steps
            .into_iter()
            .find(|s| s.summary.contains("iptables"))
            .expect("linux plans an iptables redirect");
        let Action::Run { skip_if, argv, .. } = step.action else {
            panic!("expected a command");
        };
        assert!(argv.contains(&"-A".to_string()));
        let guard = skip_if.expect("the append must carry a guard").argv;
        assert!(guard.contains(&"-C".to_string()));
        // The guard has to describe the same rule, or it would not match.
        assert_eq!(guard.len(), argv.len());
        assert_eq!(guard[guard.len() - 1], argv[argv.len() - 1]);
    }

    #[test]
    fn undo_reverses_what_setup_recorded_not_what_settings_now_say() {
        // Settings can change between setup and undo. Rebuilding the plan from
        // the new values would probe for an iptables rule and a PAC URL that
        // were never installed, and leave the real ones running.
        let mut recorded = ctx(Platform::Linux);
        recorded.proxy_port = 8443;
        recorded.contact_host = "127.0.0.1".into();

        let mut current = ctx(Platform::Linux);
        current.proxy_port = 9999;
        current.contact_host = "127.0.0.1".into();

        let lines = plan_undo_from(Some(&current), std::slice::from_ref(&recorded)).describe();
        assert!(
            lines.iter().any(|l| l.contains("port 443 to 8443")),
            "the installed redirect must be reversed: {lines:?}"
        );
        // The current settings are cleaned up too, in case they were applied
        // by a setup that predates records or was changed by hand.
        assert!(lines.iter().any(|l| l.contains("port 443 to 9999")));
        // And nothing is planned twice.
        let mut sorted = lines.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), lines.len(), "duplicate steps in {lines:?}");
    }

    #[test]
    fn a_failed_setup_does_not_erase_what_an_earlier_one_installed() {
        // The record is written before the steps run, so a setup that fails
        // part-way still leaves a trace. It must not take the previous
        // successful run's record with it: those resources are still there.
        let mut first = ctx(Platform::Linux);
        first.proxy_port = 8443;
        let mut second = ctx(Platform::Linux);
        second.proxy_port = 9999;

        let after_first = merged_records(vec![], &first);
        let after_failed_second = merged_records(after_first, &second);
        assert_eq!(after_failed_second.len(), 2);
        assert_eq!(after_failed_second[0].proxy_port, 8443);
        assert_eq!(after_failed_second[1].proxy_port, 9999);

        // And undo cleans up both, including the port the failed run replaced.
        let lines = plan_undo_from(Some(&second), &after_failed_second).describe();
        assert!(lines.iter().any(|l| l.contains("port 443 to 8443")));
        assert!(lines.iter().any(|l| l.contains("port 443 to 9999")));
    }

    #[test]
    fn re_recording_the_same_setup_does_not_grow_the_list() {
        let c = ctx(Platform::Linux);
        let once = merged_records(vec![], &c);
        let twice = merged_records(once.clone(), &c);
        assert_eq!(twice.len(), 1, "an identical setup replaces its entry");
        assert_eq!(undo_key(&twice[0]), undo_key(&c));
    }

    #[test]
    fn the_record_list_is_capped() {
        let mut records = vec![];
        for port in 0..(MAX_RECORDS as u16 + 5) {
            let mut c = ctx(Platform::Linux);
            c.proxy_port = 2000 + port;
            records = merged_records(records, &c);
        }
        assert_eq!(records.len(), MAX_RECORDS);
        // The oldest go first, so the newest configuration is always kept.
        assert_eq!(
            records.last().unwrap().proxy_port,
            2000 + MAX_RECORDS as u16 + 4
        );
    }

    #[test]
    fn a_second_setup_removes_the_redirect_it_supersedes() {
        // Changing `proxy.port` and re-running setup would otherwise leave two
        // iptables rules for port 443. The first one wins, so traffic would
        // keep going to the port that is no longer in use.
        let mut first = ctx(Platform::Linux);
        first.proxy_port = 8443;
        let mut second = ctx(Platform::Linux);
        second.proxy_port = 9999;

        let lines = plan_with_reconcile(&second, std::slice::from_ref(&first)).describe();
        let drop_old = lines
            .iter()
            .position(|l| l.contains("drop the iptables redirect from port 443 to 8443"))
            .expect("the superseded redirect is removed");
        let add_new = lines
            .iter()
            .position(|l| l.contains("redirect loopback traffic for port 443 to 9999"))
            .expect("the new redirect is installed");
        assert!(drop_old < add_new, "remove before install: {lines:?}");
    }

    #[test]
    fn re_running_setup_never_untrusts_the_ca_it_still_needs() {
        // Once the CA is trusted the forward plan only notes it, so matching
        // the earlier record's removal against descriptions let the removal
        // through and broke HTTPS on every re-run.
        let mut first = ctx(Platform::Linux);
        first.proxy_port = 8443;
        first.ca_trusted = true;
        let mut second = ctx(Platform::Linux);
        second.proxy_port = 9999;
        second.ca_trusted = true;

        let lines = plan_with_reconcile(&second, std::slice::from_ref(&first)).describe();
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("from the system trust store")),
            "the CA must survive a re-run: {lines:?}"
        );
        // The superseded redirect is still removed.
        assert!(lines.iter().any(|l| l.contains("port 443 to 8443")));
    }

    #[test]
    fn switching_to_pac_removes_the_resolver_it_no_longer_uses() {
        // Two configurations can have identical undo plans while installing
        // different things. PAC does not install the resolver drop-in, so the
        // earlier one has to be removed or the stale DNS routing stays live.
        let dns = ctx(Platform::Linux);
        let mut pac = ctx(Platform::Linux);
        pac.pac = true;
        pac.ca_trusted = true;

        let lines = plan_with_reconcile(&pac, std::slice::from_ref(&dns)).describe();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("remove") && l.contains("resolved.conf.d")),
            "the superseded resolver drop-in must be removed: {lines:?}"
        );
    }

    #[test]
    fn re_running_the_same_setup_removes_nothing() {
        // Identical configuration: nothing is superseded, so the plan must not
        // churn through removing and reinstalling the same resources.
        let c = ctx(Platform::Linux);
        assert_eq!(
            plan_with_reconcile(&c, std::slice::from_ref(&c)).describe(),
            plan(&c).describe()
        );
    }

    #[test]
    fn undo_reverses_every_recorded_setup_not_just_the_last() {
        // Two setups at different ports leave two redirects installed. Undo has
        // to know about both.
        let mut first = ctx(Platform::Linux);
        first.proxy_port = 8443;
        let mut second = ctx(Platform::Linux);
        second.proxy_port = 9999;
        let current = second.clone();

        let lines = plan_undo_from(Some(&current), &[first, second]).describe();
        assert!(lines.iter().any(|l| l.contains("port 443 to 8443")));
        assert!(lines.iter().any(|l| l.contains("port 443 to 9999")));
        let mut sorted = lines.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), lines.len(), "duplicate steps in {lines:?}");
    }

    #[test]
    fn undo_works_from_records_alone_when_the_tld_no_longer_validates() {
        // Changing `proxy.tld` to something invalid after a setup must not
        // strand what that setup installed. The records were validated when
        // written, so undo runs from those with the current settings dropped.
        let recorded = ctx(Platform::Linux);
        let lines = plan_undo_from(None, std::slice::from_ref(&recorded)).describe();
        assert_eq!(lines, plan_undo(&recorded).describe());
        assert!(lines.iter().any(|l| l.contains("resolved.conf.d")));

        // And with nothing recorded there is simply nothing to do.
        assert!(plan_undo_from(None, &[]).steps.is_empty());
    }

    #[test]
    fn undo_without_a_record_still_uses_the_current_settings() {
        let current = ctx(Platform::Linux);
        assert_eq!(
            plan_undo_from(Some(&current), &[]).describe(),
            plan_undo(&current).describe()
        );
    }

    #[test]
    fn a_tampered_record_cannot_steer_a_privileged_write() {
        // The record lives in the state directory and its fields reach `sudo`
        // as paths, so it is input, not truth. A TLD that would escape the
        // resolver directory is refused outright, and the fixed system paths
        // are rebuilt rather than believed.
        let mut evil = ctx(Platform::MacOs);
        evil.tld = "../../etc/passwd".into();
        assert!(
            sanitize_record(evil).is_none(),
            "an unusable TLD is dropped"
        );

        let mut redirected = ctx(Platform::MacOs);
        redirected.resolver_dir = PathBuf::from("/tmp/attacker");
        redirected.pf_conf = PathBuf::from("/etc/shadow");
        redirected.pf_anchor = PathBuf::from("/tmp/anchor");
        redirected.resolved_dropin_dir = PathBuf::from("/tmp/dropins");
        redirected.generated_ca = PathBuf::from("/tmp/ca.pem");

        redirected.binary = PathBuf::from("/usr/bin/some-other-daemon");

        let clean = sanitize_record(redirected).expect("a valid TLD is kept");
        // The binary is rebuilt too: the capability check reads what a file
        // has, not who granted it, so a record naming an unrelated executable
        // that happens to hold `cap_net_bind_service` would have had undo
        // strip it.
        assert_eq!(clean.binary, current_binary());
        assert_ne!(clean.binary, PathBuf::from("/usr/bin/some-other-daemon"));
        let fixed = fixed_paths();
        assert_eq!(clean.resolver_dir, fixed.resolver_dir);
        assert_eq!(clean.pf_conf, fixed.pf_conf);
        assert_eq!(clean.pf_anchor, fixed.pf_anchor);
        assert_eq!(clean.resolved_dropin_dir, fixed.resolved_dropin_dir);
        assert_eq!(clean.generated_ca, default_generated_ca());

        // Nothing in the resulting plan names the attacker's paths.
        let lines = plan_undo(&clean).describe().join("\n");
        assert!(!lines.contains("/usr/bin/some-other-daemon"));
        assert!(!lines.contains("/tmp/attacker"));
        assert!(!lines.contains("/etc/shadow"));
        assert!(!lines.contains("/tmp/ca.pem"));
    }

    #[test]
    fn a_record_in_the_previous_single_context_format_still_loads() {
        // TOML ignores keys a struct does not name, so a pre-list file parses
        // as `SetupRecords` with an empty list. Treating that as "nothing
        // recorded" would forget the installation it describes.
        let ctx = ctx(Platform::Linux);
        let legacy = toml::to_string_pretty(&ctx).expect("serializes");
        assert!(
            toml::from_str::<SetupRecords>(&legacy)
                .expect("parses as records")
                .setups
                .is_empty(),
            "the empty parse is what makes the fallback necessary"
        );
        let back: SetupContext = toml::from_str(&legacy).expect("parses as one context");
        assert_eq!(undo_key(&back), undo_key(&ctx));

        // And the list format round-trips as itself.
        let records = SetupRecords {
            setups: vec![ctx.clone()],
        };
        let text = toml::to_string_pretty(&records).expect("serializes");
        let back = toml::from_str::<SetupRecords>(&text).expect("parses");
        assert_eq!(back.setups.len(), 1);
        assert_eq!(undo_key(&back.setups[0]), undo_key(&ctx));
    }

    #[test]
    fn a_record_with_no_systemd_version_survives_a_round_trip() {
        // macOS records `systemd_version: None`, which TOML omits entirely.
        // If that failed to read back, `--undo` would silently fall back to
        // current settings on the platform where the pf anchor lives.
        let mut ctx = ctx(Platform::MacOs);
        ctx.systemd_version = None;
        ctx.network_services = vec![];
        let text = toml::to_string_pretty(&ctx).expect("serializes");
        let back: SetupContext = toml::from_str(&text).expect("deserializes without the field");
        assert_eq!(back.systemd_version, None);
        assert_eq!(plan_undo(&back).describe(), plan_undo(&ctx).describe());
    }

    #[test]
    fn a_setup_record_survives_a_round_trip() {
        let ctx = ctx(Platform::MacOs);
        let text = toml::to_string_pretty(&ctx).expect("serializes");
        let back: SetupContext = toml::from_str(&text).expect("deserializes");
        assert_eq!(back.tld, ctx.tld);
        assert_eq!(back.proxy_port, ctx.proxy_port);
        assert_eq!(back.ca_path, ctx.ca_path);
        assert_eq!(back.platform, ctx.platform);
        assert_eq!(plan_undo(&back).describe(), plan_undo(&ctx).describe());
    }

    #[test]
    fn a_record_from_before_the_generated_ca_field_still_names_one() {
        // The field defaults to the current generated path, not to an empty
        // one, or undo would plan the removal of nothing and leave the CA that
        // setup installed trusted.
        let ctx = ctx(Platform::Linux);
        let mut table = toml::Table::try_from(&ctx).expect("serializes");
        table.remove("generated_ca");
        let legacy = toml::to_string_pretty(&table).expect("re-serializes");
        assert!(!legacy.contains("generated_ca"));

        let back: SetupContext = toml::from_str(&legacy).expect("parses without the field");
        assert_eq!(back.generated_ca, default_generated_ca());
        assert!(!back.generated_ca.as_os_str().is_empty());
        assert!(
            plan_undo(&back)
                .describe()
                .iter()
                .any(|l| l.contains("trust store")),
            "a legacy record still plans the CA removal"
        );
    }

    #[test]
    fn an_untrust_step_runs_when_the_certificate_file_is_gone() {
        // `is_ca_trusted` answers false for a missing PEM, which is exactly
        // when `uninstall_cert` is needed: the certificate can still be
        // installed under its own name. Skipping there leaves a trusted root.
        let missing = PathBuf::from("/nonexistent/pitchfork-ca.pem");
        assert!(!missing.exists());
        assert!(!already_done(&Action::UntrustCa {
            path: missing,
            sudo: false,
        }));
    }

    #[test]
    fn a_setup_that_wrote_no_resolver_file_never_claims_one() {
        // Found by running setup twice for real on a machine without
        // systemd-resolved: the second run planned a privileged removal of a
        // drop-in the first had never written.
        let mut no_resolved = ctx(Platform::Linux);
        no_resolved.systemd_resolved = false;
        assert!(
            !plan_undo(&no_resolved)
                .describe()
                .iter()
                .any(|l| l.contains("resolved.conf.d")),
            "nothing was installed, so nothing is claimed"
        );

        // The same for the other configurations that install no resolver file.
        let mut dns_off = ctx(Platform::Linux);
        dns_off.dns_enabled = false;
        assert!(
            !plan_undo(&dns_off)
                .describe()
                .iter()
                .any(|l| l.contains("resolved.conf.d"))
        );
        let mut localhost = ctx(Platform::Linux);
        localhost.tld = "localhost".into();
        assert!(
            !plan_undo(&localhost)
                .describe()
                .iter()
                .any(|l| l.contains("resolved.conf.d"))
        );
        let mut lan = ctx(Platform::Linux);
        lan.lan = true;
        assert!(
            !plan_undo(&lan)
                .describe()
                .iter()
                .any(|l| l.contains("resolved.conf.d"))
        );

        // And a configuration that does write one still reverses it.
        assert!(
            plan_undo(&ctx(Platform::Linux))
                .describe()
                .iter()
                .any(|l| l.contains("resolved.conf.d"))
        );

        // Re-running on a machine without systemd-resolved removes nothing.
        let mut second = no_resolved.clone();
        second.proxy_port = 9999;
        let lines = plan_with_reconcile(&second, std::slice::from_ref(&no_resolved)).describe();
        assert!(!lines.iter().any(|l| l.contains("resolved.conf.d")));
        // The superseded redirect is still reversed.
        assert!(lines.iter().any(|l| l.contains("port 443 to 8443")));
    }

    #[test]
    fn an_http_only_setup_never_claims_the_ca() {
        // Plain HTTP installs no CA, so its undo must not claim one. Otherwise
        // a later HTTP setup reconciles against it and removes a CA that an
        // earlier HTTPS run, or the user, put in the trust store.
        let mut http = ctx(Platform::Linux);
        http.https = false;
        assert!(
            !plan_undo(&http)
                .describe()
                .iter()
                .any(|l| l.contains("trust store")),
            "an HTTP-only setup has no CA to remove"
        );

        // And reconciling two HTTP setups leaves the trust store alone.
        let mut second = http.clone();
        second.proxy_port = 9999;
        let lines = plan_with_reconcile(&second, std::slice::from_ref(&http)).describe();
        assert!(!lines.iter().any(|l| l.contains("trust store")));
        // The superseded redirect is still reversed.
        assert!(lines.iter().any(|l| l.contains("port 80 to 8443")));
    }

    #[test]
    fn undo_always_queues_the_ca_removal_and_probes_for_it() {
        // Setting `proxy.tls_cert` after a setup does not untrust the CA that
        // setup installed. Gating the removal on it left the root CA and its
        // private key trusted with no command able to remove them, so the step
        // is queued regardless and skipped by probing the trust store.
        let mut custom = ctx(Platform::MacOs);
        custom.custom_cert = true;
        custom.ca_path = PathBuf::from("/etc/mycert.pem");

        let step = plan_undo(&custom)
            .steps
            .into_iter()
            .find(|s| s.summary.contains("trust store"))
            .expect("the CA removal is queued even with a custom certificate");
        let Action::UntrustCa { path, .. } = &step.action else {
            panic!("expected the probing action");
        };
        // Always the generated CA, never the configured certificate: pitchfork
        // installed the former and has no business removing the latter.
        assert_eq!(path, &custom.generated_ca);
        assert_ne!(path, &custom.ca_path);

        // Linux needs elevation for its trust store; macOS does not.
        assert!(!step.needs_sudo());
        let mut linux = ctx(Platform::Linux);
        linux.custom_cert = true;
        assert!(
            plan_undo(&linux)
                .steps
                .iter()
                .find(|s| s.summary.contains("trust store"))
                .expect("queued on linux too")
                .needs_sudo()
        );
    }

    #[test]
    fn undo_reverses_each_platform_step() {
        let mac = ctx(Platform::MacOs);
        let resolver = mac.resolver_file().display().to_string();
        assert_eq!(
            plan_undo(&mac).describe(),
            vec![
                format!("[sudo] remove {resolver}"),
                "[sudo] remove the pitchfork anchor from /etc/pf.conf".to_string(),
                "[sudo] remove /etc/pf.anchors/pitchfork".to_string(),
                "[sudo] reload pf rules from /etc/pf.conf".to_string(),
                "remove the pitchfork CA at /state/proxy/ca.pem from the system trust store"
                    .to_string(),
            ]
        );

        let linux = ctx(Platform::Linux);
        let dropin = linux.resolved_dropin().display().to_string();
        let lines = plan_undo(&linux).describe();
        assert!(lines.contains(&format!("[sudo] remove {dropin}")));
        // 8443 installs a redirect and grants no capability, so undo reverses
        // exactly that.
        assert!(
            lines
                .iter()
                .any(|l| l.contains("drop the iptables redirect"))
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("revoke cap_net_bind_service"))
        );
    }

    #[test]
    fn undo_turns_off_a_pac_url_even_without_the_pac_flag() {
        // `--undo` takes no `--pac`, so it has to reverse the PAC steps anyway.
        let mut c = ctx(Platform::MacOs);
        c.pac = false;
        c.network_services = vec!["Wi-Fi".into()];
        assert!(
            plan_undo(&c)
                .describe()
                .iter()
                .any(|l| l.contains("turn off the automatic proxy URL") && l.contains("Wi-Fi"))
        );
    }

    /// Apple's stock `/etc/pf.conf`, verbatim.
    const APPLE_PF_CONF: &str = r#"#
# Default PF configuration file.
#
# This file contains the main ruleset, which gets automatically loaded
# at startup.  PF will not be automatically enabled, however.  Instead,
# each component which utilizes PF is responsible for enabling and disabling
# PF via -E and -X as documented in pfctl(8).
#

#
# com.apple anchor point
#
scrub-anchor "com.apple/*"
nat-anchor "com.apple/*"
rdr-anchor "com.apple/*"
dummynet-anchor "com.apple/*"
anchor "com.apple/*"
load anchor "com.apple" from "/etc/pf.anchors/com.apple"
"#;

    #[test]
    fn the_pf_block_lands_before_the_filter_anchor() {
        // pf requires translation rules before filter rules, and Apple's stock
        // file ends with a filter anchor. Appending there makes `pfctl -f` fail
        // with "Rules must be in order", so the block goes after the last
        // `rdr-anchor` instead.
        let block = "rdr-anchor \"pitchfork\"\nload anchor \"pitchfork\" from \"/etc/pf.anchors/pitchfork\"";
        let out = splice_pf_block(APPLE_PF_CONF, block);

        let lines: Vec<&str> = out.lines().collect();
        let ours = lines
            .iter()
            .position(|l| l.contains("rdr-anchor \"pitchfork\""))
            .expect("our rdr-anchor is present");
        let apple_rdr = lines
            .iter()
            .position(|l| l.contains("rdr-anchor \"com.apple/*\""))
            .unwrap();
        let apple_filter = lines
            .iter()
            .position(|l| l.trim() == "anchor \"com.apple/*\"")
            .unwrap();
        assert!(apple_rdr < ours, "must follow the existing rdr-anchor");
        assert!(ours < apple_filter, "must precede the filter anchor");
        assert_eq!(out.matches(MARKER_START).count(), 1);
        assert!(out.ends_with('\n'));

        // Re-running replaces in place rather than adding a second block.
        let again = splice_pf_block(&out, block);
        assert_eq!(again, out);

        // And undo restores Apple's file byte for byte.
        assert_eq!(splice_pf_block(&again, ""), APPLE_PF_CONF);
    }

    #[test]
    fn a_pf_conf_with_no_rdr_anchor_still_lands_before_the_filters() {
        // A customised file with filter rules but no translation anchor to
        // follow: appending would put our rdr after them and `pfctl -f` would
        // reject the file, so the block goes in front of the first filter.
        let custom = "set skip on lo0\nblock in all\npass out all\n";
        let out = splice_pf_block(custom, "rdr-anchor \"pitchfork\"");
        let lines: Vec<&str> = out.lines().collect();
        let ours = lines
            .iter()
            .position(|l| l.contains("rdr-anchor \"pitchfork\""))
            .unwrap();
        let first_filter = lines
            .iter()
            .position(|l| l.trim() == "block in all")
            .unwrap();
        assert!(ours < first_filter, "translation must precede filtering");
        assert_eq!(splice_pf_block(&out, ""), custom);

        // With neither translation nor filter rules, the end is fine.
        let out = splice_pf_block("# empty ruleset\n", "rdr-anchor \"pitchfork\"");
        assert!(out.contains("rdr-anchor \"pitchfork\""));
        assert_eq!(out.matches(MARKER_START).count(), 1);
    }

    #[test]
    fn a_pf_block_in_the_wrong_place_is_repositioned_not_left_alone() {
        // What an older pitchfork left behind: the block appended after the
        // filter anchor, where pf rejects it.
        let stale = format!(
            "{}{MARKER_START}\nrdr-anchor \"pitchfork\"\n{MARKER_END}\n",
            APPLE_PF_CONF
        );
        let action = Action::EnsureBlock {
            path: PathBuf::from("/nonexistent"),
            content: "rdr-anchor \"pitchfork\"".to_string(),
            sudo: false,
            pf_order: true,
            requires: None,
        };
        // Not "already done" just because the markers are present somewhere.
        assert_ne!(splice_pf_block(&stale, "rdr-anchor \"pitchfork\""), stale);
        assert!(!already_done(&action), "a missing file is not already done");

        let fixed = splice_pf_block(&stale, "rdr-anchor \"pitchfork\"");
        let lines: Vec<&str> = fixed.lines().collect();
        let ours = lines
            .iter()
            .position(|l| l.contains("rdr-anchor \"pitchfork\""))
            .unwrap();
        let filter = lines
            .iter()
            .position(|l| l.trim() == "anchor \"com.apple/*\"")
            .unwrap();
        assert!(ours < filter, "the stale block should have moved up");
        assert_eq!(fixed.matches(MARKER_START).count(), 1);
    }

    #[test]
    fn a_pac_setup_claims_no_resolver_file_on_undo() {
        // `plan` runs `plan_pac` instead of `plan_resolver`, so a PAC setup
        // never writes `/etc/resolver/<tld>` or a systemd-resolved drop-in.
        // Undo must not offer them up: listing them plans a privileged removal
        // that never applied, and a reconciling re-run reads the undo plan as
        // "what this configuration installed" and acts on it.
        for platform in [Platform::MacOs, Platform::Linux] {
            // An empty directory, so the macOS sweep for files carrying our
            // header cannot pick up whatever a developer's `/etc/resolver`
            // happens to hold.
            let dir = tempfile::tempdir().unwrap();
            let mut c = ctx(platform);
            c.pac = true;
            c.resolver_dir = dir.path().to_path_buf();

            let resolver = c.resolver_file().display().to_string();
            let dropin = c.resolved_dropin().display().to_string();
            let names_resolver = |l: &String| l.contains(&resolver) || l.contains(&dropin);

            let forward = plan(&c).describe();
            assert!(
                !forward.iter().any(names_resolver),
                "{platform:?}: pac setup wrote resolver configuration: {forward:?}"
            );

            let undo = plan_undo(&c).describe();
            assert!(
                !undo.iter().any(names_resolver),
                "{platform:?}: pac undo claimed a resolver file it never wrote: {undo:?}"
            );
            assert!(
                !undo.iter().any(|l| l.contains("systemd-resolved")),
                "{platform:?}: pac undo restarted the resolver for nothing: {undo:?}"
            );
        }
    }

    #[test]
    fn undo_restores_an_automatic_proxy_url_that_setup_replaced() {
        // Without the recorded value undo can only switch the proxy off, so a
        // machine with a corporate PAC URL would lose it permanently.
        let mut mac = ctx(Platform::MacOs);
        mac.pac = true;
        mac.network_services = vec!["Wi-Fi".into()];
        mac.prior_auto_proxy = vec![PriorAutoProxy {
            target: "Wi-Fi".into(),
            url: "https://corp.example/proxy.pac".into(),
            state: "on".into(),
        }];
        let lines = plan_undo(&mac).describe();
        let at = |needle: &str| lines.iter().position(|l| l.contains(needle));

        let off = at("turn off the automatic proxy URL").expect("no disable step");
        let url = at("restore the automatic proxy URL").expect("no url restore");
        let state = at("restore the automatic proxy switch").expect("no state restore");
        assert!(
            lines[url].contains("https://corp.example/proxy.pac"),
            "the restore step did not name the recorded URL: {:?}",
            lines[url]
        );
        // The URL is restored before the switch. `-setautoproxyurl` turns the
        // switch on as a side effect, so writing it last would re-enable a
        // proxy the recorded state says was off.
        assert!(off < url && url < state, "restore steps are out of order");

        // And a recorded `off` really does end up off: the last word on the
        // switch is the recorded value, not the side effect of the URL write.
        let mut disabled = mac.clone();
        disabled.prior_auto_proxy[0].state = "off".into();
        let lines = plan_undo(&disabled).describe();
        let last_switch = lines
            .iter()
            .rposition(|l| l.contains("automatic proxy switch"))
            .expect("no state restore");
        assert!(
            lines[last_switch].ends_with("off"),
            "the last word on the switch was not the recorded state: {:?}",
            lines[last_switch]
        );

        // GNOME records the mode rather than on/off, and restores it the same
        // way round.
        let mut linux = ctx(Platform::Linux);
        linux.pac = true;
        linux.gnome = true;
        linux.prior_auto_proxy = vec![PriorAutoProxy {
            target: "gnome".into(),
            url: "http://wpad.corp/proxy.pac".into(),
            state: "manual".into(),
        }];
        let lines = plan_undo(&linux).describe();
        let mode = lines
            .iter()
            .position(|l| l.contains("restore the GNOME proxy mode to manual"))
            .expect("no mode restore");
        let url = lines
            .iter()
            .position(|l| l.contains("http://wpad.corp/proxy.pac"))
            .expect("no url restore");
        assert!(url < mode, "the GNOME restore steps are out of order");

        // With nothing recorded, undo is exactly what it was before.
        let mut bare = ctx(Platform::MacOs);
        bare.pac = true;
        bare.network_services = vec!["Wi-Fi".into()];
        assert!(
            !plan_undo(&bare)
                .describe()
                .iter()
                .any(|l| l.contains("restore the automatic proxy")),
            "a restore was planned with nothing recorded to restore"
        );
    }

    #[test]
    fn a_recorded_automatic_proxy_value_that_could_steer_a_command_is_dropped() {
        // These are handed back to `networksetup` and `gsettings` as argv. A
        // value starting with `-` would be read as a flag, and a state outside
        // the tool's vocabulary is not something this file wrote.
        let mut c = ctx(Platform::MacOs);
        c.prior_auto_proxy = vec![
            PriorAutoProxy {
                target: "-setairportpower".into(),
                url: "https://corp.example/proxy.pac".into(),
                state: "on".into(),
            },
            PriorAutoProxy {
                target: "Wi-Fi".into(),
                url: "-setautoproxystate".into(),
                state: "on".into(),
            },
            PriorAutoProxy {
                target: "Wi-Fi".into(),
                url: "https://corp.example/proxy.pac".into(),
                state: "; rm -rf /".into(),
            },
            PriorAutoProxy {
                target: "Wi-Fi".into(),
                url: "/etc/passwd".into(),
                state: "on".into(),
            },
            PriorAutoProxy {
                target: "Wi-Fi".into(),
                url: "https://corp.example/proxy.pac".into(),
                state: "on".into(),
            },
        ];
        let kept = sanitize_record(c)
            .expect("the record itself is usable")
            .prior_auto_proxy;
        assert_eq!(
            kept,
            vec![PriorAutoProxy {
                target: "Wi-Fi".into(),
                url: "https://corp.example/proxy.pac".into(),
                state: "on".into(),
            }],
            "an unusable recorded value survived"
        );
    }

    #[test]
    fn a_write_replaces_a_file_whole_and_leaves_no_temporary_behind() {
        // These are shared system files, so a partial write is worse than no
        // write: the system goes on reading whatever is there.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pf.conf");
        std::fs::write(&target, "original\n").unwrap();

        write_file(&target, "replacement\n", false).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "replacement\n");

        // Nothing is left beside it. A stray temporary in `/etc/resolver`
        // would be read as another resolver file.
        let strays: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "pf.conf")
            .collect();
        assert!(strays.is_empty(), "left behind: {strays:?}");

        // A directory that does not exist yet is created, not an error.
        let nested = dir.path().join("resolver").join("test");
        write_file(&nested, "nameserver 127.0.0.1\n", false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&nested).unwrap(),
            "nameserver 127.0.0.1\n"
        );
    }

    #[test]
    fn setting_up_does_not_replace_a_resolver_file_somebody_else_wrote() {
        // `--undo` already refuses to delete a file without our header. Until
        // now setting up had no matching check, so it would take over another
        // tool's `/etc/resolver/test` and then decline to clean up after
        // itself — protection in the removal direction only.
        let dir = tempfile::tempdir().unwrap();
        let theirs = dir.path().join("test");
        let original = "nameserver 127.0.0.1\nport 20560\n";
        std::fs::write(&theirs, original).unwrap();

        let step = Step {
            summary: "write the resolver file".into(),
            action: Action::WriteFile {
                path: theirs.clone(),
                content: macos_resolver_file(15353),
                sudo: false,
            },
            resource: None,
        };

        let err = execute(&step).expect_err("somebody else's resolver file was replaced");
        assert!(
            err.to_string().contains(&theirs.display().to_string()),
            "the refusal did not name the file: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&theirs).unwrap(),
            original,
            "the file was modified despite the refusal"
        );

        // One pitchfork wrote is replaced as usual, so a re-run still works.
        std::fs::write(&theirs, macos_resolver_file(20000)).unwrap();
        execute(&step).expect("pitchfork refused to update its own file");
        assert_eq!(
            std::fs::read_to_string(&theirs).unwrap(),
            macos_resolver_file(15353)
        );

        // And a path with nothing at it is written without argument.
        let fresh = dir.path().join("fresh");
        let step = Step {
            summary: "write a new resolver file".into(),
            action: Action::WriteFile {
                path: fresh.clone(),
                content: macos_resolver_file(15353),
                sudo: false,
            },
            resource: None,
        };
        execute(&step).expect("a new file was refused");
        assert!(fresh.exists());
    }

    #[test]
    fn a_trust_store_that_will_not_answer_does_not_cancel_the_ca_removal() {
        // `is_ca_trusted` reports an unreadable store as "not trusted", which
        // is the safe reading when deciding whether to *add* trust but the
        // wrong one here: it would mark the removal already done and leave the
        // CA trusted with nothing left to take it out.
        assert!(
            !untrust_already_done(true, None),
            "an unreadable trust store cancelled the removal"
        );
        // A missing PEM does not cancel it either: the certificate can still be
        // installed in the store under its own name.
        assert!(!untrust_already_done(false, Some(false)));
        assert!(!untrust_already_done(false, None));
        assert!(!untrust_already_done(false, Some(true)));
        // Still trusted, so there is work to do.
        assert!(!untrust_already_done(true, Some(true)));
        // The one case that is genuinely done.
        assert!(untrust_already_done(true, Some(false)));
    }

    #[test]
    fn a_capability_that_cannot_be_read_is_not_taken_for_absent() {
        // Two opposite mistakes, one decision.
        //
        // Granting goes through `sudo setcap`, whose secure_path includes
        // `/usr/sbin`; an ordinary user's PATH on Debian does not. So the
        // probe failing outright is likely on exactly the machines where the
        // capability was granted, and reading that as "nothing there" would
        // have `--undo` report success while the binary keeps the right to
        // bind privileged ports.
        //
        // And `getcap` exits 0 printing nothing for a file that carries no
        // capabilities, which is the ordinary state after an upgrade replaces
        // the binary. Reading *that* as "could not ask" makes `--undo` refuse
        // to finish when there is nothing left to do.
        let bind = BIND_CAPABILITY.to_string();
        let other = "cap_net_raw".to_string();

        // Could not ask: not done, whatever else is true.
        assert!(
            !revoke_already_done(None),
            "an unreadable probe was taken for absent"
        );

        // Definitely nothing of ours there: done.
        assert!(revoke_already_done(Some(&[])));
        assert!(revoke_already_done(Some(std::slice::from_ref(&other))));

        // Definitely there: not done.
        assert!(!revoke_already_done(Some(std::slice::from_ref(&bind))));
        assert!(!revoke_already_done(Some(&[bind, other])));
    }

    #[test]
    fn getcap_output_is_read_the_way_getcap_writes_it() {
        // A granted capability, and the empty output that means the file
        // carries none.
        assert_eq!(
            parse_capabilities(&format!(" {BIND_CAPABILITY}=ep")),
            vec![BIND_CAPABILITY.to_string()]
        );
        assert!(parse_capabilities("").is_empty());
    }

    #[test]
    fn output_that_does_not_name_the_binary_is_no_answer() {
        // Only genuinely empty output means "no capabilities". Output that
        // says something we cannot attribute to this path — a binary whose
        // path is not valid UTF-8, or a `getcap` that words things
        // differently — must not be read as an empty list, because that would
        // have `--undo` skip the revocation and leave the capability in place.
        let matched = |stdout: &str| capabilities_from_getcap("/usr/local/bin/pitchfork", stdout);

        assert_eq!(matched(""), Some(vec![]), "empty output is no capabilities");
        assert_eq!(matched("   \n"), Some(vec![]));
        assert_eq!(
            matched(&format!("/usr/local/bin/pitchfork {BIND_CAPABILITY}=ep\n")),
            Some(vec![BIND_CAPABILITY.to_string()])
        );
        assert_eq!(
            matched("/usr/local/bin/pitchfor\u{fffd}k cap_net_bind_service=ep\n"),
            None,
            "output naming a different path was read as an empty list"
        );
        assert_eq!(matched("something unexpected\n"), None);
    }

    #[test]
    fn undo_keeps_the_anchor_while_pf_conf_still_names_it() {
        // The mirror of the forward guard. `apply` continues past a failed
        // step, so a `RemoveBlock` that did not go through would be followed
        // by the deletion of the file that block names, leaving `/etc/pf.conf`
        // pointing at a missing anchor and breaking every later `pfctl -f`.
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("pf.conf");
        let anchor = dir.path().join("pitchfork-anchor");
        std::fs::write(&anchor, pf_anchor_rules(443, 8443)).unwrap();
        std::fs::write(
            &conf,
            format!("{APPLE_PF_CONF}{MARKER_START}\nload anchor \"pitchfork\"\n{MARKER_END}\n"),
        )
        .unwrap();

        let step = Step {
            summary: "remove the anchor".into(),
            action: Action::RemoveFile {
                path: anchor.clone(),
                sudo: false,
                still_referenced_by: Some(conf.clone()),
            },
            resource: None,
        };

        let err = execute(&step).expect_err("the anchor was removed while still referenced");
        assert!(
            err.to_string().contains(&conf.display().to_string()),
            "the refusal did not name the file holding the reference: {err}"
        );
        assert!(anchor.exists(), "the anchor was deleted anyway");

        // A referrer that cannot be read is not evidence that the reference is
        // gone. Guessing wrong here is the whole failure this guard prevents,
        // so an unreadable file keeps the anchor.
        std::fs::create_dir(dir.path().join("unreadable")).unwrap();
        let unreadable = Step {
            summary: "remove the anchor".into(),
            action: Action::RemoveFile {
                path: anchor.clone(),
                sudo: false,
                // A directory, so reading it as a file fails.
                still_referenced_by: Some(dir.path().join("unreadable")),
            },
            resource: None,
        };
        let err = unreadable_err(&unreadable);
        assert!(
            err.contains("could not be read"),
            "an unreadable referrer did not stop the removal: {err}"
        );
        assert!(anchor.exists(), "the anchor was deleted on a failed read");

        // A referrer that is not there at all cannot reference anything.
        let gone = Step {
            summary: "remove the anchor".into(),
            action: Action::RemoveFile {
                path: anchor.clone(),
                sudo: false,
                still_referenced_by: Some(dir.path().join("no-such-file")),
            },
            resource: None,
        };
        execute(&gone).expect("a missing referrer blocked the removal");
        assert!(!anchor.exists());

        // And with the block gone the ordinary removal goes through.
        std::fs::write(&anchor, pf_anchor_rules(443, 8443)).unwrap();
        std::fs::write(&conf, APPLE_PF_CONF).unwrap();
        execute(&step).expect("the anchor was kept with nothing referencing it");
        assert!(!anchor.exists());
    }

    /// `execute`'s error as a string, for asserting on the reason.
    fn unreadable_err(step: &Step) -> String {
        execute(step)
            .expect_err("the step was allowed to run")
            .to_string()
    }

    #[test]
    fn a_block_naming_a_file_pitchfork_does_not_own_is_not_written() {
        // `apply` continues past a failed step, so a refused or failed write of
        // the pf anchor would otherwise be followed by a splice that points
        // `/etc/pf.conf` at a file pitchfork does not control — either missing,
        // which breaks every later `pfctl -f` including at boot, or somebody
        // else's, which makes pf load their rules under our name. Both survive
        // the run that caused them.
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("pf.conf");
        std::fs::write(&conf, APPLE_PF_CONF).unwrap();
        let anchor = dir.path().join("pitchfork-anchor");

        let step = Step {
            summary: "load the pitchfork anchor".into(),
            action: Action::EnsureBlock {
                path: conf.clone(),
                content: format!("load anchor \"pitchfork\" from \"{}\"", anchor.display()),
                sudo: false,
                pf_order: true,
                requires: Some(anchor.clone()),
            },
            resource: None,
        };

        let err = execute(&step).expect_err("the block was written anyway");
        assert!(
            err.to_string().contains(&anchor.display().to_string()),
            "the failure did not name the missing file: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&conf).unwrap(),
            APPLE_PF_CONF,
            "pf.conf was modified despite the missing anchor"
        );

        // A file at that path that pitchfork did not write is not good enough
        // either: the write of the anchor refuses to replace it, so splicing a
        // reference would point pf at somebody else's rules.
        std::fs::write(&anchor, "rdr pass on lo0\n").unwrap();
        let err = execute(&step).expect_err("pf.conf was pointed at a foreign anchor");
        assert!(
            err.to_string().contains("was not written by pitchfork"),
            "the refusal did not say why: {err}"
        );
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), APPLE_PF_CONF);

        // Once our own anchor is in place the same step goes through.
        std::fs::write(&anchor, pf_anchor_rules(443, 8443)).unwrap();
        execute(&step).expect("the block was refused with the anchor in place");
        assert!(
            std::fs::read_to_string(&conf)
                .unwrap()
                .contains(MARKER_START)
        );
    }

    #[test]
    fn undo_leaves_a_resolver_file_it_did_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let theirs = dir.path().join("test");
        std::fs::write(&theirs, "nameserver 10.0.0.1\n").unwrap();

        // Removing it is reported as already done, and the file survives.
        let action = Action::RemoveFile {
            path: theirs.clone(),
            sudo: false,
            still_referenced_by: None,
        };
        assert!(already_done(&action));
        assert_eq!(skip_reason(&action), "not written by pitchfork, left alone");
        assert!(remove_file(&theirs, false).is_ok());
        assert!(theirs.exists(), "somebody else's resolver file was deleted");

        // One we wrote is removed.
        let ours = dir.path().join("ours");
        std::fs::write(&ours, macos_resolver_file(15353)).unwrap();
        assert!(!already_done(&Action::RemoveFile {
            path: ours.clone(),
            sudo: false,
            still_referenced_by: None,
        }));
        remove_file(&ours, false).unwrap();
        assert!(!ours.exists());
    }

    #[test]
    fn undo_sweeps_resolver_files_left_under_an_older_tld() {
        let dir = tempfile::tempdir().unwrap();
        // Written by an earlier setup, before `proxy.tld` changed.
        let old = dir.path().join("oldtld");
        std::fs::write(&old, macos_resolver_file(15353)).unwrap();
        // Not ours.
        let theirs = dir.path().join("corp");
        std::fs::write(&theirs, "nameserver 10.0.0.1\n").unwrap();

        let found = managed_resolver_files(dir.path(), "test", true);
        assert!(found.contains(&dir.path().join("test")), "the current TLD");
        assert!(found.contains(&old), "the orphaned file");
        assert!(!found.contains(&theirs), "somebody else's file");
    }

    #[test]
    fn an_old_systemd_is_warned_about_rather_than_silently_ineffective() {
        let mut c = ctx(Platform::Linux);
        c.systemd_version = Some(245);
        let p = plan(&c);
        assert!(
            p.manual.iter().any(|m| m.contains("too old")),
            "expected a version warning: {:?}",
            p.manual
        );
        // A current systemd says nothing.
        assert!(
            !plan(&ctx(Platform::Linux))
                .manual
                .iter()
                .any(|m| m.contains("too old"))
        );
    }

    #[test]
    fn the_resolved_restart_is_unconditional() {
        // Every guard tried here has at some point skipped a restart that was
        // needed, and the symptom is setup reporting success while names do not
        // resolve. Restarting always is the behaviour worth pinning.
        let steps = plan(&ctx(Platform::Linux)).steps;
        let restart = steps
            .iter()
            .find(|s| s.summary.contains("restart systemd-resolved"))
            .expect("linux plans a restart");
        let Action::Run { skip_if, .. } = &restart.action else {
            panic!("expected a command");
        };
        assert!(skip_if.is_none(), "the restart must not be guarded");
        // And the cost is stated in the plan the user approves.
        assert!(restart.summary.contains("interrupts DNS"));
    }

    #[test]
    fn pf_keywords_are_whole_words_not_prefixes() {
        // Macros are common in custom rulesets and are not rules.
        assert!(!is_pf_translation("nat_if = \"en0\""));
        assert!(!is_pf_translation("nat_if=\"en0\""));
        assert!(!is_pf_filter("pass_hosts = \"{ 10.0.0.1 }\""));
        assert!(!is_pf_filter("blocklist = \"{ 1.2.3.4 }\""));
        // Real rules still classify.
        assert!(is_pf_translation("rdr-anchor \"com.apple/*\""));
        assert!(is_pf_translation("  nat on en0 from any to any"));
        assert!(is_pf_filter("anchor \"com.apple/*\""));
        assert!(is_pf_filter("pass out all"));
        assert!(is_pf_filter("block in all"));
        // Options are neither, so the block is never placed before them.
        assert!(!is_pf_translation("set skip on lo0"));
        assert!(!is_pf_filter("set skip on lo0"));
    }

    #[test]
    fn a_macro_named_like_a_rule_does_not_move_the_block() {
        // `nat_if` must not count as a translation rule, or the block would
        // land before the `set` options and after nothing useful.
        let custom = "nat_if = \"en0\"\nset skip on lo0\nblock in all\n";
        let out = splice_pf_block(custom, "rdr-anchor \"pitchfork\"");
        let lines: Vec<&str> = out.lines().collect();
        let ours = lines
            .iter()
            .position(|l| l.contains("rdr-anchor \"pitchfork\""))
            .unwrap();
        let macro_line = lines.iter().position(|l| l.starts_with("nat_if")).unwrap();
        let filter = lines
            .iter()
            .position(|l| l.trim() == "block in all")
            .unwrap();
        assert!(
            ours > macro_line,
            "must not be treated as a translation rule"
        );
        assert!(ours < filter, "must still precede the filter");
    }

    #[test]
    fn splice_block_inserts_replaces_and_removes() {
        let original = "scrub-anchor \"com.apple/*\"\n";
        let added = splice_block(original, "rdr-anchor \"pitchfork\"");
        assert_eq!(
            added,
            "scrub-anchor \"com.apple/*\"\n\n# pitchfork-start\nrdr-anchor \"pitchfork\"\n# pitchfork-end\n"
        );

        let replaced = splice_block(&added, "rdr-anchor \"other\"");
        assert!(replaced.contains("rdr-anchor \"other\""));
        assert!(!replaced.contains("rdr-anchor \"pitchfork\""));
        assert_eq!(replaced.matches(MARKER_START).count(), 1);

        assert_eq!(splice_block(&added, ""), original);
    }

    #[test]
    fn generated_files_carry_the_expected_contents() {
        assert_eq!(
            macos_resolver_file(15353),
            "# Managed by pitchfork (pitchfork proxy setup)\nnameserver 127.0.0.1\nport 15353\n"
        );
        let dropin = resolved_dropin("test", 15353);
        assert!(dropin.contains("[Resolve]"));
        assert!(dropin.contains("DNS=127.0.0.1:15353"));
        // A routing-only domain, so other lookups keep using the link's servers.
        assert!(dropin.contains("Domains=~test"));
        assert!(pf_anchor_rules(443, 8443).contains("port 443 -> 127.0.0.1 port 8443"));
    }
}
