//! `pitchfork proxy doctor` — one line per thing that has to be true for a
//! proxy URL to work in a browser.
//!
//! Each check is independent and non-destructive, so a failure early on does
//! not hide the state of everything after it.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

/// How long any single probe is allowed to take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Transaction ID on the resolver probe, echoed in the reply.
///
/// Checked on the way back so a datagram from another process on this loopback
/// port is not mistaken for the responder's answer.
const QUERY_ID: u16 = 0x7f00;

/// Most of a PAC response doctor will read before giving up on it.
const MAX_PAC_RESPONSE: u64 = 64 * 1024;

/// Result of one check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warn,
    Fail,
}

impl Status {
    fn glyph(self) -> &'static str {
        match self {
            Status::Pass => "ok  ",
            Status::Warn => "warn",
            Status::Fail => "fail",
        }
    }
}

/// One named check and what it found.
#[derive(Clone, Debug)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
}

impl Check {
    fn new(name: impl Into<String>, status: Status, detail: impl Into<String>) -> Self {
        Check {
            name: name.into(),
            status,
            detail: detail.into(),
        }
    }

    /// The single line this check prints.
    pub fn line(&self) -> String {
        format!(
            "[{}] {:<22} {}",
            self.status.glyph(),
            self.name,
            self.detail
        )
    }
}

/// A name under the TLD that nothing could have cached or registered.
///
/// The resolver is meant to answer every name under the TLD, so a random one
/// proves wildcard resolution rather than a leftover `/etc/hosts` entry.
fn probe_name(tld: &str) -> String {
    // Clock nanoseconds mixed with the pid: unique enough for a label that only
    // has to be one nothing has seen before, without pulling in a RNG crate.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nonce = nanos ^ std::process::id().rotate_left(11) ^ seq.rotate_left(23);
    format!("pf-doctor-{nonce:08x}.{tld}")
}

/// Ask the loopback responder directly, bypassing the system resolver.
///
/// Returns the address it answered with.
async fn query_responder(
    name: &str,
    port: u16,
    family: std::net::IpAddr,
) -> Result<std::net::IpAddr, String> {
    // Ask for the record the proxy's bind address means it will answer. With
    // `proxy.host = "::1"` there is deliberately no A record, and asking only
    // for A would read that as the resolver being down.
    let (qtype, rdlen) = match family {
        std::net::IpAddr::V4(_) => (1u16, 4usize),
        std::net::IpAddr::V6(_) => (28u16, 16usize),
    };
    let mut msg: Vec<u8> = Vec::new();
    msg.extend_from_slice(&QUERY_ID.to_be_bytes()); // ID
    msg.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    msg.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    msg.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    for label in name.split('.') {
        msg.push(u8::try_from(label.len()).map_err(|_| "label too long".to_string())?);
        msg.extend_from_slice(label.as_bytes());
    }
    msg.push(0);
    msg.extend_from_slice(&qtype.to_be_bytes());
    msg.extend_from_slice(&1u16.to_be_bytes()); // QCLASS IN

    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    sock.send_to(&msg, addr).await.map_err(|e| e.to_string())?;
    // Read until the responder answers, rather than trusting whatever arrives
    // first. Another local process can write to this ephemeral port, and a
    // datagram from somewhere else, or one carrying a different transaction
    // ID, would otherwise be reported as the resolver's answer — a false pass
    // or a false failure in a command whose whole job is to be believed.
    let deadline = tokio::time::Instant::now() + PROBE_TIMEOUT;
    let mut buf = [0u8; 512];
    let resp = loop {
        let (len, from) = tokio::time::timeout_at(deadline, sock.recv_from(&mut buf))
            .await
            .map_err(|_| "no reply within 3s".to_string())?
            .map_err(|e| e.to_string())?;
        if from != addr {
            continue;
        }
        if len < 12 {
            return Err("reply was too short to be a DNS message".into());
        }
        if u16::from_be_bytes([buf[0], buf[1]]) != QUERY_ID {
            continue;
        }
        break &buf[..len];
    };
    let rcode = u16::from_be_bytes([resp[2], resp[3]]) & 0x000F;
    if rcode != 0 {
        return Err(format!("responder returned rcode {rcode}"));
    }
    // Skip the echoed question, then read the first answer's RDATA.
    let mut pos = 12;
    while let Some(&l) = resp.get(pos) {
        pos += 1 + usize::from(l);
        if l == 0 {
            break;
        }
    }
    pos += 4; // QTYPE + QCLASS
    let rdata = resp
        .get(pos + 12..pos + 12 + rdlen)
        .ok_or_else(|| "reply carried no address record".to_string())?;
    Ok(match family {
        std::net::IpAddr::V4(_) => {
            std::net::IpAddr::V4(Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3]))
        }
        std::net::IpAddr::V6(_) => {
            let octets: [u8; 16] = rdata.try_into().map_err(|_| "short AAAA record")?;
            std::net::IpAddr::V6(octets.into())
        }
    })
}

/// What a probe of a TCP port found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PortProbe {
    /// Nothing accepted a connection.
    Closed,
    /// Something accepted, and answered without pitchfork's header.
    Foreign,
    /// Something accepted but did not answer in time, so who holds the port is
    /// still an open question.
    ///
    /// Kept apart from `Foreign` because the two call for opposite advice. A
    /// foreign listener means the port is taken and URLs would reach the wrong
    /// service; a slow one is just as likely to be pitchfork itself, busy with
    /// tunnels or a handshake. Reporting the first when the truth is the second
    /// hands the reader a confident, wrong diagnosis.
    Silent,
    /// The pitchfork proxy answered.
    Pitchfork,
}

/// Probe a loopback port and find out whether the pitchfork proxy is behind it.
///
/// A bare TCP connect is not enough: a proxy bind failure leaves the supervisor
/// running, and some unrelated service may hold the port. Every pitchfork
/// response carries the `x-pitchfork` header, including the redirect the HTTPS
/// listener sends for a plain-HTTP request, so one plain request over the port
/// distinguishes the two without needing to complete a TLS handshake.
async fn probe_port(ip: std::net::IpAddr, port: u16) -> PortProbe {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let addr = SocketAddr::from((ip, port));
    let connect = tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect(addr)).await;
    let Ok(Ok(mut stream)) = connect else {
        return PortProbe::Closed;
    };

    const REQUEST: &str = "GET / HTTP/1.1\r\nHost: pf-doctor.invalid\r\nConnection: close\r\n\r\n";
    let exchange = async {
        stream.write_all(REQUEST.as_bytes()).await?;
        let mut buf = vec![0u8; 2048];
        let n = stream.read(&mut buf).await?;
        buf.truncate(n);
        Ok::<_, std::io::Error>(buf)
    };
    match tokio::time::timeout(PROBE_TIMEOUT, exchange).await {
        Ok(Ok(buf)) => {
            let head = String::from_utf8_lossy(&buf).to_ascii_lowercase();
            if head.contains("x-pitchfork") {
                PortProbe::Pitchfork
            } else {
                PortProbe::Foreign
            }
        }
        // Accepted and then refused to talk: a reset, a broken pipe, a
        // protocol that hangs up on an HTTP request. The proxy answers every
        // plain request with its header, including the redirect the HTTPS
        // listener sends, so whatever did this is not it. That is the same
        // conclusion as an unrecognised reply, and a port conflict is what the
        // reader needs to hear.
        Ok(Err(_)) => PortProbe::Foreign,
        // Nothing came back inside the budget, which settles nothing: a
        // pitchfork proxy busy with tunnels or a handshake looks like this.
        Err(_) => PortProbe::Silent,
    }
}

/// Check that a configured `tls_cert` / `tls_key` pair loads.
///
/// Mirrors what the TLS listener does at startup, so a failure here is the same
/// failure the proxy would hit.
fn load_configured_cert(cert_path: &std::path::Path, key: &str) -> Result<(), String> {
    if !cert_path.exists() {
        return Err(format!(
            "proxy.tls_cert {} does not exist, so the HTTPS listener cannot start",
            cert_path.display()
        ));
    }
    if key.is_empty() {
        return Err("proxy.tls_cert is set but proxy.tls_key is empty".to_string());
    }
    let key_path = std::path::Path::new(key);
    if !key_path.exists() {
        return Err(format!(
            "proxy.tls_key {} does not exist, so the HTTPS listener cannot start",
            key_path.display()
        ));
    }
    #[cfg(feature = "proxy-tls")]
    {
        use rustls_pemfile::{certs, private_key};
        let cert_pem = std::fs::read(cert_path)
            .map_err(|e| format!("cannot read {}: {e}", cert_path.display()))?;
        let found: Vec<_> = certs(&mut cert_pem.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("cannot parse {}: {e}", cert_path.display()))?;
        if found.is_empty() {
            return Err(format!("no certificate found in {}", cert_path.display()));
        }
        let key_pem = std::fs::read(key_path)
            .map_err(|e| format!("cannot read {}: {e}", key_path.display()))?;
        let key_der = private_key(&mut key_pem.as_slice())
            .map_err(|e| format!("cannot parse {}: {e}", key_path.display()))?
            .ok_or_else(|| format!("no private key found in {}", key_path.display()))?;
        // The pair also has to belong together, or the listener starts and then
        // fails every handshake.
        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
            .map_err(|e| format!("cannot use the key in {}: {e}", key_path.display()))?;
        rustls::sign::CertifiedKey::new(found, signing_key)
            .keys_match()
            .map_err(|e| {
                format!(
                    "proxy.tls_key {} does not match proxy.tls_cert {}: {e}",
                    key_path.display(),
                    cert_path.display()
                )
            })?;
    }
    Ok(())
}

/// Run a blocking call with a deadline, without letting it outlive the answer.
///
/// `spawn_blocking` is the obvious tool and the wrong one here. Its tasks are
/// not cancellable, and the runtime waits for them when it shuts down, so a
/// probe wedged in `getaddrinfo` or a keychain lookup would still hang `proxy
/// doctor` at exit even with a timeout on the handle. A detached OS thread is
/// not tracked by the runtime: dropping the receiver abandons it and the
/// process exits regardless.
///
/// `None` means the deadline passed, which every caller reports as an unknown
/// rather than a failure, because nothing was learned either way.
async fn bounded_blocking<T, F>(f: F) -> Option<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    bounded_blocking_for(PROBE_TIMEOUT, f).await
}

/// [`bounded_blocking`] with a deadline of its own.
///
/// For work that already gives up after some time internally. Such a probe
/// needs longer here than it allows itself, because its own deadline is what
/// cleans up after it — a child process killed and reaped, say. An outer
/// deadline that fired first would return, let the command exit, and leave
/// that cleanup unrun.
async fn bounded_blocking_for<T, F>(budget: Duration, f: F) -> Option<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    tokio::time::timeout(budget, rx).await.ok()?.ok()
}

/// Whether the CA is in the system trust store, or `None` when the lookup did
/// not finish in time.
///
/// The lookup shells out on Linux and reads the keychain on macOS, so it runs
/// off the runtime rather than stalling it, and under a deadline: `security
/// verify-cert` can block on a keychain prompt or a revocation check.
///
/// The `None` is kept rather than folded into `false`. A lookup that timed out
/// says nothing about whether the CA is trusted, and telling someone to run
/// `proxy trust` on that basis would send them to fix something that may well
/// already be right.
async fn ca_is_trusted(ca_path: &std::path::Path) -> Option<bool> {
    let probe = ca_path.to_path_buf();
    // `ca_trust_state`, not `is_ca_trusted`: the latter reports a trust store
    // that would not answer as "not trusted", which this check would then
    // print as a failure telling the reader to run `proxy trust`.
    // Longer than the probe allows itself, so its own deadline is the one that
    // fires: that is what kills and reaps `security verify-cert`. Waiting the
    // same 3s as everything else here would always win the race, return, and
    // let the command exit with the child still running — the orphan
    // `ca_trust_state`'s kill-and-reap was added to prevent.
    let budget = crate::proxy::trust::TRUST_PROBE_TIMEOUT + Duration::from_secs(1);
    bounded_blocking_for(budget, move || crate::proxy::trust::ca_trust_state(&probe))
        .await
        .flatten()
}

/// Whether the system's automatic proxy URL points at pitchfork's PAC file.
///
/// Read from the same places `proxy setup --pac` writes, so a working PAC
/// configuration is recognised rather than reported as broken DNS.
///
/// Every command gets its own `PROBE_TIMEOUT`, like the other probes here, and
/// for a concrete reason: `gsettings` blocks on the session bus, so on a
/// headless box or over an SSH session with no bus it can hang indefinitely.
/// This runs before any check is printed, so an unbounded call means `proxy
/// doctor` produces no output at all rather than one unknown line.
///
/// The budget is per command rather than shared across the probe, and the
/// per-service queries run concurrently, so the total does not grow with the
/// number of network services. A shared deadline would let a machine with
/// enough active services exhaust it before reaching the configured one and
/// report a working `--pac` setup as absent.
///
/// The commands are spawned asynchronously rather than on a blocking thread so
/// that the timeout actually reaches them: dropping the future kills the child,
/// whereas a `spawn_blocking` thread stuck in `wait` cannot be cancelled and
/// would go on to hold up runtime shutdown.
/// What a settings command had to say.
///
/// Three outcomes, not two, and the middle one is the whole point. A machine
/// with no `gsettings` or no GNOME schema has no such proxy setting to read:
/// that is the ordinary state of a Linux box without GNOME, and calling it
/// unreadable would downgrade every dependent check to a warning, so real
/// breakage would stop being reported. A command that failed for some other
/// reason — no session bus, no authorisation, a service that is down — settles
/// nothing, and neither does one that never returned.
enum Answer {
    Said(String),
    /// There is no such setting on this machine, which is an answer.
    Nothing,
    /// Nothing was learned either way.
    Unknown,
}

/// Whether a failed settings command proves the setting does not exist here.
///
/// Matched on what the tool said rather than on its exit status, because the
/// status is the same for "no such schema" as for "the session bus is not
/// running", and only the first is evidence.
fn means_no_such_setting(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    // `gsettings` with no schemas at all, without this schema, or without the
    // key. The quotes around the name are typographic, so the match stops
    // before them.
    s.contains("no schemas installed") || s.contains("no such schema") || s.contains("no such key")
}

async fn pac_configured(pac_url: &str) -> Option<bool> {
    async fn output(argv: &[&str]) -> Answer {
        let run = tokio::process::Command::new(argv[0])
            .args(&argv[1..])
            // The classifier below reads what the tool printed, so the tool has
            // to print the messages it was written against. Without this a
            // French or Japanese desktop gets translated errors, none of which
            // match, and every one of them is then read as "could not tell" —
            // which is exactly the misreport this classification exists to
            // avoid, just restricted to people who do not work in English.
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C")
            .env("LANG", "C")
            // Captured, not discarded: what the tool says is the only way to
            // tell "there is no such setting" from "I could not reach it".
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output();
        // On a timeout the future is dropped, which kills the child.
        match tokio::time::timeout(PROBE_TIMEOUT, run).await {
            Err(_) => Answer::Unknown,
            // The tool is not installed, so the mechanism it configures is not
            // in use here. Any other spawn failure is not that clear.
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Answer::Nothing,
            Ok(Err(_)) => Answer::Unknown,
            Ok(Ok(out)) if out.status.success() => {
                Answer::Said(String::from_utf8_lossy(&out.stdout).into_owned())
            }
            Ok(Ok(out)) if means_no_such_setting(&String::from_utf8_lossy(&out.stderr)) => {
                Answer::Nothing
            }
            // Ran and failed for a reason that proves nothing: no session bus,
            // no authorisation, a service that is down.
            Ok(Ok(_)) => Answer::Unknown,
        }
    }

    // Whether any command gave up, which is the difference between "no PAC is
    // configured" and "the system would not say".
    let mut unknown = false;
    if cfg!(target_os = "macos") {
        // `networksetup -getautoproxyurl` prints the URL and an `Enabled`
        // line. A leftover URL with automatic proxy switched off routes
        // nothing, so both have to hold.
        let services = match output(&["networksetup", "-listallnetworkservices"]).await {
            Answer::Said(o) => o,
            Answer::Nothing => return Some(false),
            Answer::Unknown => return None,
        };

        let mut probes = tokio::task::JoinSet::new();
        for svc in services
            .lines()
            .skip(1) // header line explaining the asterisk
            // A leading asterisk marks a disabled service.
            .filter(|l| !l.trim().is_empty() && !l.starts_with('*'))
            .map(|l| l.trim().to_string())
        {
            let url = pac_url.to_string();
            probes.spawn(async move {
                match output(&["networksetup", "-getautoproxyurl", &svc]).await {
                    Answer::Said(o) => {
                        let enabled = o.lines().any(|l| {
                            let l = l.trim().to_ascii_lowercase();
                            l.starts_with("enabled:") && l.ends_with("yes")
                        });
                        Some(o.contains(&url) && enabled)
                    }
                    // No such service is a definite "not on this one"; a
                    // failure that proves nothing leaves the whole answer open.
                    Answer::Nothing => Some(false),
                    Answer::Unknown => None,
                }
            });
        }
        while let Some(res) = probes.join_next().await {
            match res.unwrap_or(None) {
                // `JoinSet` aborts the rest of the probes when dropped.
                Some(true) => return Some(true),
                Some(false) => {}
                None => unknown = true,
            }
        }
        // One service that would not answer could have been the configured
        // one, so "none of them" is only true when all of them answered.
        return (!unknown).then_some(false);
    }

    // Under GNOME the URL is only consulted in `auto` mode. No `gsettings`, or
    // no schema for it, means this machine has no GNOME proxy setting at all,
    // which is a definite answer rather than an unreadable one.
    let mode = output(&["gsettings", "get", "org.gnome.system.proxy", "mode"]).await;
    if !matches!(&mode, Answer::Said(m) if m.trim().trim_matches('\'') == "auto") {
        return decide_gnome(mode, None, pac_url);
    }
    let url = output(&[
        "gsettings",
        "get",
        "org.gnome.system.proxy",
        "autoconfig-url",
    ])
    .await;
    decide_gnome(mode, Some(url), pac_url)
}

/// Read the two GNOME proxy settings into an answer.
///
/// Separated from the commands that produce them so every combination can be
/// checked without a session bus, a desktop or a particular host. `url` is
/// `None` when the mode ruled the question out before it was worth asking.
fn decide_gnome(mode: Answer, url: Option<Answer>, pac_url: &str) -> Option<bool> {
    match mode {
        Answer::Unknown => return None,
        // No schema, no key, no `gsettings`: this machine has no GNOME proxy
        // setting, so nothing is configured through one.
        Answer::Nothing => return Some(false),
        Answer::Said(m) if m.trim().trim_matches('\'') != "auto" => return Some(false),
        Answer::Said(_) => {}
    }
    match url {
        Some(Answer::Said(u)) => Some(u.contains(pac_url)),
        Some(Answer::Nothing) => Some(false),
        Some(Answer::Unknown) => None,
        // `auto` mode with no URL read is not a state the caller produces.
        None => None,
    }
}

/// Fetch the PAC file the system is configured to use.
async fn fetch_pac(url: &str) -> Result<(), String> {
    let addr = url
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let path = super::pac::PAC_PATH;
    let host = addr.clone();
    let exchange = async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(&addr).await?;
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await?;
        // Bounded: whatever answers this port need not be pitchfork, and an
        // endless response would otherwise be read into memory in full. A PAC
        // script is a few hundred bytes.
        let mut body = Vec::new();
        tokio::io::AsyncReadExt::take(&mut stream, MAX_PAC_RESPONSE)
            .read_to_end(&mut body)
            .await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&body).into_owned())
    };
    match tokio::time::timeout(PROBE_TIMEOUT, exchange).await {
        Ok(Ok(body)) if body.contains("FindProxyForURL") => Ok(()),
        Ok(Ok(_)) => Err("the response was not a PAC script".to_string()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("no reply within 3s".to_string()),
    }
}

/// Run every check against the current settings.
/// Turn the system resolver's answer into a check.
///
/// `resolved` is `None` when the lookup did not finish in time, which is not
/// the same as a name that does not resolve and must not be reported as one.
fn resolution_check(
    name: &str,
    tld: &str,
    pac_ready: Option<bool>,
    lan: bool,
    resolved: Option<Result<Vec<std::net::IpAddr>, String>>,
) -> Check {
    match resolved {
        Some(Ok(ips)) if !ips.is_empty() => Check::new(
            "system resolution",
            Status::Pass,
            format!(
                "{name} resolves to {}",
                ips.iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        // `proxy setup --pac` deliberately leaves system DNS alone: the browser
        // sends these names to the proxy instead of resolving them. Reporting a
        // failure there would be telling the user to fix a setup that works.
        _ if pac_ready == Some(true) => Check::new(
            "system resolution",
            Status::Pass,
            format!("not needed: the system proxy sends *.{tld} to pitchfork (PAC)"),
        ),
        // A lookup that never came back is not the same as a name that does
        // not resolve. Saying it failed would send someone to re-run setup
        // over a wedged resolver that setup cannot fix. Ahead of the LAN and
        // unknown-PAC arms, which both explain a name that did not resolve.
        None => Check::new(
            "system resolution",
            Status::Warn,
            format!("the system resolver did not answer in time, so {name} could not be checked"),
        ),
        // LAN mode serves `.local`, which belongs to mDNS, and setup leaves
        // that namespace alone on purpose. So a name that does not resolve is
        // not evidence that setup failed, and telling the reader to run it
        // again would be wrong. It is still evidence that a browser on this
        // machine cannot reach the advertised URLs — the publisher may not
        // have started, or the host may have no mDNS resolver — and passing it
        // silently would hide exactly the broken path this command is for.
        // Warn, and say where to look.
        _ if lan => Check::new(
            "system resolution",
            Status::Warn,
            format!(
                "{name} did not resolve. In LAN mode *.{tld} is answered over \
                 mDNS, which `proxy setup` does not configure: check that the \
                 supervisor is publishing and that this host resolves mDNS names"
            ),
        ),
        // The system would not say whether a PAC file is in use, and under one
        // this check does not apply at all. Calling it a failure would be a
        // guess in the direction that sends someone to re-run setup.
        _ if pac_ready.is_none() => Check::new(
            "system resolution",
            Status::Warn,
            format!(
                "{name} does not resolve, but the system would not say whether \
                 a proxy auto-config file is in use, which would explain it"
            ),
        ),
        // The resolver's own words, rather than a guess at what went wrong.
        Some(Err(e)) => Check::new(
            "system resolution",
            Status::Fail,
            format!("{name} does not resolve ({e}) — run `pitchfork proxy setup`"),
        ),
        Some(Ok(_)) => Check::new(
            "system resolution",
            Status::Fail,
            format!("{name} does not resolve — run `pitchfork proxy setup`"),
        ),
    }
}

/// Which name the system-resolution check should look up.
enum SystemName {
    /// Look this one up.
    Published(String),
    /// LAN mode with an empty registry: nothing is advertised to resolve.
    NonePublished,
    /// The registry could not be read, so there is no name and no verdict.
    Unreadable,
}

/// A hostname LAN mode actually advertises, or `None` when none is registered.
///
/// mDNS publishes `<slug>.<tld>` for each registered slug and nothing else, so
/// this is the only name whose resolution says anything about whether the LAN
/// path works. An ambiguous slug is skipped because the proxy refuses to route
/// it, so it is not advertised either.
///
/// Reads the global config, which takes a file lock and can therefore wait on
/// another process, so callers run it under a deadline like every other probe
/// here rather than on the async task.
fn published_slug_name(tld: &str) -> Option<String> {
    let slugs = crate::pitchfork_toml::PitchforkToml::read_global_slugs();
    slugs
        .keys()
        .find(|slug| !crate::pitchfork_toml::PitchforkToml::slug_is_ambiguous(slug, &slugs))
        .map(|slug| format!("{}.{tld}", slug.to_ascii_lowercase()))
}

pub async fn run(s: &crate::settings::Settings) -> Vec<Check> {
    let mut checks = Vec::new();

    if !s.proxy.enable {
        checks.push(Check::new(
            "proxy",
            Status::Fail,
            "disabled — set proxy.enable = true",
        ));
        return checks;
    }

    let tld = crate::proxy::effective_tld(s).to_string();
    let dns_port = super::dns::dns_port(s);
    // Not coerced to 443. The proxy itself refuses to start on an out-of-range
    // `proxy.port`, so silently probing 443 instead would report either a
    // listener that is nothing to do with pitchfork or a missing one, and in
    // both cases hide the misconfiguration this command exists to surface.
    let Some(proxy_port) = u16::try_from(s.proxy.port).ok().filter(|&p| p > 0) else {
        checks.push(Check::new(
            "proxy port",
            Status::Fail,
            format!(
                "proxy.port is {}, which is not a usable port — the proxy \
                 cannot start until it is set between 1 and 65535",
                s.proxy.port
            ),
        ));
        return checks;
    };
    let standard_port = if s.proxy.https { 443 } else { 80 };
    // Probe the address the proxy actually listens on. `proxy.host = "::1"`
    // means nothing is on IPv4 loopback, and probing there would report a
    // failure that is not real.
    let proxy_ip: std::net::IpAddr = match s.proxy.host.parse() {
        Ok(std::net::IpAddr::V4(ip)) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
        Ok(std::net::IpAddr::V6(ip)) if ip.is_unspecified() => std::net::Ipv6Addr::LOCALHOST.into(),
        Ok(ip) => ip,
        Err(_) => Ipv4Addr::LOCALHOST.into(),
    };
    let proxy_at = |port: u16| match proxy_ip {
        std::net::IpAddr::V6(ip) => format!("[{ip}]:{port}"),
        std::net::IpAddr::V4(ip) => format!("{ip}:{port}"),
    };
    let pac_url = format!("http://{}{}", proxy_at(proxy_port), super::pac::PAC_PATH);
    // Which record the resolver serves comes from the resolver's own
    // configuration, not from the proxy's bind address. The two differ in LAN
    // mode, where the resolver hands out the detected IPv4 interface address
    // whatever `proxy.host` is, and asking for the wrong record would read a
    // working resolver as down.
    //
    // `None` for the LAN address is deliberate: detecting it is not needed to
    // know the family, and LAN mode is IPv4 either way.
    let resolver_family = {
        let cfg = super::dns::config_from_settings(s, None);
        match (cfg.ipv4, cfg.ipv6) {
            (Some(ip), _) => std::net::IpAddr::V4(ip),
            (None, Some(ip)) => std::net::IpAddr::V6(ip),
            // Nothing served at all; ask for A so the failure is reported
            // against the record a caller would expect.
            (None, None) => std::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
        }
    };
    // Probed once: each call spawns a blocking task that runs `networksetup`
    // per active macOS service, or two `gsettings` invocations.
    let pac_ready = pac_configured(&pac_url).await;

    // 1. The proxy itself.
    checks.push(match probe_port(proxy_ip, proxy_port).await {
        PortProbe::Pitchfork => Check::new(
            "proxy listener",
            Status::Pass,
            format!("the pitchfork proxy answers on {}", proxy_at(proxy_port)),
        ),
        PortProbe::Foreign => Check::new(
            "proxy listener",
            Status::Fail,
            format!(
                "something other than pitchfork holds {} — proxy URLs would reach it instead",
                proxy_at(proxy_port)
            ),
        ),
        PortProbe::Silent => Check::new(
            "proxy listener",
            Status::Warn,
            format!(
                "something holds {} but did not answer in time, so it could not \
                 be identified — it may be the proxy under load",
                proxy_at(proxy_port)
            ),
        ),
        PortProbe::Closed => Check::new(
            "proxy listener",
            Status::Fail,
            format!(
                "nothing is listening on {} — start it with `pitchfork supervisor start`",
                proxy_at(proxy_port)
            ),
        ),
    });

    // 2. The resolver, queried directly.
    let name = probe_name(&tld);
    if !s.proxy.dns {
        checks.push(Check::new(
            "dns resolver",
            Status::Warn,
            "proxy.dns is false, so pitchfork answers no DNS queries",
        ));
    } else {
        match query_responder(&name, dns_port, resolver_family).await {
            Ok(ip) => checks.push(Check::new(
                "dns resolver",
                Status::Pass,
                format!("127.0.0.1:{dns_port} answers *.{tld} with {ip}"),
            )),
            Err(e) => checks.push(Check::new(
                "dns resolver",
                Status::Fail,
                format!("127.0.0.1:{dns_port} did not answer: {e}"),
            )),
        }
    }

    // 3. The system resolver, which is what a browser actually uses.
    // Under a deadline like every other probe: `getaddrinfo` consults
    // `resolv.conf`, the configured nameservers and whatever NSS modules are
    // installed, any of which can stall indefinitely. Nothing is printed until
    // this function returns, so an unbounded lookup means no output at all.
    let lan = s.proxy.lan || !s.proxy.lan_ip.is_empty();
    // Which name to ask the *system* resolver about. Not the same question as
    // the one above: the loopback responder answers anything under the TLD, so
    // a name nothing could have cached is the right probe for it. mDNS does
    // not. LAN mode publishes one record per registered slug and no wildcard,
    // so a random name never resolves there however healthy the setup is, and
    // asking for one would make this check warn on every LAN machine.
    //
    // The same holds with `proxy.dns = false`: nothing answers wildcards then,
    // and only names written to /etc/hosts (by `proxy.sync_hosts`, or by hand)
    // resolve, which are the published slugs.
    //
    // Not under a working PAC file, though: names never reach the system
    // resolver there, and the regular check already passes it as such.
    let published_only = lan || (!s.proxy.dns && pac_ready != Some(true));
    let system_name = if published_only {
        // Off the async task and under the same budget as the rest: this reads
        // the global config behind a file lock, so a concurrent `proxy add` or
        // a wedged process holding it would otherwise hang the whole command
        // before a single line had been printed.
        let for_tld = tld.clone();
        // Not flattened: the outer `None` is the deadline elapsing, which is
        // not the same as the registry being empty, and reporting one as the
        // other would claim nothing is published when the truth is unknown.
        match bounded_blocking(move || published_slug_name(&for_tld)).await {
            Some(found) => found
                .map(SystemName::Published)
                .unwrap_or(SystemName::NonePublished),
            None => SystemName::Unreadable,
        }
    } else {
        SystemName::Published(name.clone())
    };

    match system_name {
        // LAN mode with nothing registered yet. There is no published name to
        // look up, so there is nothing this check can find out.
        SystemName::NonePublished if lan => checks.push(Check::new(
            "system resolution",
            Status::Pass,
            "nothing is published yet — add one with `pitchfork proxy add <slug>`",
        )),
        // The resolver is off and no slug is written anywhere, so automatic
        // project hostnames have nothing to resolve them. A warning rather
        // than a failure: browsers resolve `*.localhost` on their own.
        SystemName::NonePublished => checks.push(Check::new(
            "system resolution",
            Status::Warn,
            format!(
                "proxy.dns is false and no slug is published, so *.{tld} names resolve \
                 only where the system or browser does so itself — enable proxy.dns and \
                 run `pitchfork proxy setup`, or register a slug with proxy.sync_hosts on"
            ),
        )),
        SystemName::Unreadable => checks.push(Check::new(
            "system resolution",
            Status::Warn,
            "the slug registry did not open in time, so there was no name to check",
        )),
        SystemName::Published(lookup) => {
            let lookup_name = lookup.clone();
            let resolved = bounded_blocking(move || {
                use std::net::ToSocketAddrs;
                (lookup_name.as_str(), 80u16)
                    .to_socket_addrs()
                    .map(|addrs| addrs.map(|a| a.ip()).collect::<Vec<_>>())
                    .map_err(|e| e.to_string())
            })
            .await;
            let mut check = resolution_check(&lookup, &tld, pac_ready, lan, resolved);
            // Setup installs no resolver route when the resolver is off, so
            // "run setup" would not help; say what would.
            if !s.proxy.dns && !lan && check.status == Status::Fail {
                check.detail = format!(
                    "{lookup} does not resolve, and proxy.dns is false — enable proxy.dns \
                     and run `pitchfork proxy setup`, or keep proxy.sync_hosts on"
                );
            }
            checks.push(check);
        }
    }

    // 4. The PAC file, when the system is pointed at it.
    if pac_ready == Some(true) {
        checks.push(match fetch_pac(&pac_url).await {
            Ok(()) => Check::new(
                "pac file",
                Status::Pass,
                format!("the system proxy uses {pac_url}, and it is being served"),
            ),
            Err(e) => Check::new(
                "pac file",
                Status::Fail,
                format!("the system proxy uses {pac_url}, but it could not be fetched: {e}"),
            ),
        });
    }

    // 5. Certificate trust.
    if s.proxy.https
        && let Some(problem) =
            crate::proxy::server::tls_pair_problem(&s.proxy.tls_cert, &s.proxy.tls_key)
    {
        checks.push(Check::new("certificate", Status::Fail, problem));
    } else if s.proxy.https {
        let custom = !s.proxy.tls_cert.is_empty();
        let ca_path = if custom {
            std::path::PathBuf::from(&s.proxy.tls_cert)
        } else {
            crate::env::PITCHFORK_STATE_DIR.join("proxy").join("ca.pem")
        };
        checks.push(if custom {
            // A configured certificate is served as-is, so the thing worth
            // checking is that it loads at all: a missing or unreadable pair
            // stops the HTTPS listener from starting.
            // Reading and parsing the pair is blocking work, so it goes off
            // the runtime rather than stalling it mid-check.
            // Under the same deadline as the other probes: these paths are
            // configured, so they can point at a stalled network mount, and an
            // unbounded read would hang the command with nothing printed.
            let (cert_arg, key_arg) = (ca_path.clone(), s.proxy.tls_key.clone());
            match bounded_blocking(move || load_configured_cert(&cert_arg, &key_arg)).await {
                Some(Ok(())) => Check::new(
                    "certificate",
                    Status::Pass,
                    format!("serving your certificate from {}", ca_path.display()),
                ),
                Some(Err(e)) => Check::new("certificate", Status::Fail, e),
                None => Check::new(
                    "certificate",
                    Status::Warn,
                    format!(
                        "reading {} did not finish in time, so the certificate \
                         could not be checked",
                        ca_path.display()
                    ),
                ),
            }
        } else if !ca_path.exists() {
            Check::new(
                "certificate",
                Status::Fail,
                format!(
                    "no CA at {} — start the supervisor once to generate it",
                    ca_path.display()
                ),
            )
        } else {
            match ca_is_trusted(&ca_path).await {
                Some(true) => {
                    Check::new("certificate", Status::Pass, "the pitchfork CA is trusted")
                }
                Some(false) => Check::new(
                    "certificate",
                    Status::Fail,
                    "the pitchfork CA is not trusted — run `pitchfork proxy trust`",
                ),
                None => Check::new(
                    "certificate",
                    Status::Warn,
                    "could not tell whether the pitchfork CA is trusted: \
                     the trust store did not answer in time",
                ),
            }
        });
    }

    // 6. Reaching the proxy on the port the URL implies.
    //
    // Not under PAC: the browser is told to connect to `proxy.port` directly,
    // so setup installs no redirect and reporting a missing one would send the
    // user to fix a configuration that works.
    if proxy_port != standard_port && pac_ready == Some(true) {
        checks.push(Check::new(
            "standard port",
            Status::Pass,
            format!("not needed: the PAC file sends requests to port {proxy_port}"),
        ));
    } else if proxy_port != standard_port && pac_ready.is_none() {
        // A PAC setup installs no redirect, so whether a missing one is a
        // problem depends on an answer the system declined to give.
        checks.push(Check::new(
            "standard port",
            Status::Warn,
            format!(
                "the system would not say whether a proxy auto-config file is \
                 in use, so port {standard_port} was not checked"
            ),
        ));
    } else if proxy_port != standard_port {
        checks.push(match probe_port(proxy_ip, standard_port).await {
            PortProbe::Pitchfork => Check::new(
                "standard port",
                Status::Pass,
                format!("port {standard_port} reaches the proxy on {proxy_port}"),
            ),
            PortProbe::Foreign => Check::new(
                "standard port",
                Status::Fail,
                format!(
                    "port {standard_port} is held by something other than pitchfork, \
                     so proxy URLs without a port would reach it instead"
                ),
            ),
            PortProbe::Silent => Check::new(
                "standard port",
                Status::Warn,
                format!(
                    "something holds port {standard_port} but did not answer in \
                     time, so it could not be identified"
                ),
            ),
            PortProbe::Closed => Check::new(
                "standard port",
                Status::Warn,
                format!(
                    "port {standard_port} is not redirected, so URLs need :{proxy_port} — \
                     run `pitchfork proxy setup`"
                ),
            ),
        });
    }

    checks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two findings on this branch pulled in opposite directions: reading every
    /// failure as "unknown" hides real breakage on a machine without GNOME,
    /// and reading every failure as "not configured" misreports a PAC setup
    /// whose settings service is simply unreachable. The exit status is the
    /// same either way, so the tool's own words are what decide.
    #[test]
    fn only_a_missing_setting_counts_as_an_answer() {
        // Verbatim from `gsettings` on a host with no GNOME schemas, including
        // the typographic quotes it puts around the schema name.
        for absent in [
            "No schemas installed\n",
            "No such schema \u{201c}org.gnome.system.proxy\u{201d}\n",
            "No such key \u{201c}autoconfig-url\u{201d}\n",
        ] {
            assert!(
                means_no_such_setting(absent),
                "not recognised as an absent setting: {absent:?}"
            );
        }

        // These establish nothing about whether a PAC file is in use.
        for unclear in [
            "Failed to connect to the session bus: No such file or directory\n",
            "Error spawning command line \u{201c}dbus-launch\u{201d}\n",
            "** Error: The parameters were not valid.\n",
            "Operation not permitted\n",
            "",
        ] {
            assert!(
                !means_no_such_setting(unclear),
                "treated as proof that nothing is configured: {unclear:?}"
            );
        }
    }

    /// A machine with no proxy auto-config set up has to give a definite "no",
    /// not an "I could not tell". Returning the latter downgrades every
    /// dependent check to a warning, so an ordinary Linux box without GNOME —
    /// where `gsettings` is missing or has no schema — would stop reporting
    /// broken DNS and a missing port redirect at all.
    ///
    /// Driven from constructed answers rather than the host's own `gsettings`,
    /// so it says the same thing on a developer's GNOME desktop, in a
    /// container with no session bus, and on a macOS runner.
    #[test]
    fn a_machine_with_no_pac_gives_a_definite_answer() {
        let ours = "http://127.0.0.1:8443/proxy.pac";
        let said = |s: &str| Answer::Said(s.to_string());

        // No schema, no key, no `gsettings` at all: nothing is configured.
        assert_eq!(decide_gnome(Answer::Nothing, None, ours), Some(false));

        // Configured, but not at automatic: the URL is not consulted.
        assert_eq!(decide_gnome(said("'manual'\n"), None, ours), Some(false));

        // Automatic, pointing at us.
        assert_eq!(
            decide_gnome(said("'auto'\n"), Some(said(&format!("'{ours}'\n"))), ours),
            Some(true)
        );

        // Automatic, pointing at somebody else's proxy auto-config.
        assert_eq!(
            decide_gnome(
                said("'auto'\n"),
                Some(said("'https://corp.example/proxy.pac'\n")),
                ours
            ),
            Some(false)
        );

        // Only a probe that settled nothing leaves the answer open.
        assert_eq!(decide_gnome(Answer::Unknown, None, ours), None);
        assert_eq!(
            decide_gnome(said("'auto'\n"), Some(Answer::Unknown), ours),
            None
        );
    }

    /// A listener that takes the connection and then refuses to talk is not
    /// pitchfork, which answers every plain request with its header. Reporting
    /// that as indeterminate would bury a port conflict behind a warning
    /// suggesting the proxy is merely busy.
    #[tokio::test]
    async fn a_listener_that_hangs_up_is_a_port_conflict_not_an_unknown() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepting = tokio::spawn(async move {
            // Accept and drop, so the probe's read ends without an answer.
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });

        assert!(
            matches!(probe_port(addr.ip(), addr.port()).await, PortProbe::Foreign),
            "a listener that said nothing usable was not reported as a conflict"
        );
        accepting.abort();
    }

    /// The trust probe cleans up after itself when its own deadline fires:
    /// it kills and reaps the child. An outer deadline of the same length
    /// always wins that race, so the command would return and exit with the
    /// child still running. The outer one has to be the longer of the two.
    #[test]
    fn the_trust_probe_is_given_longer_than_it_gives_itself() {
        assert!(
            crate::proxy::trust::TRUST_PROBE_TIMEOUT >= PROBE_TIMEOUT,
            "the shared probe budget would cut the trust probe short"
        );
        let budget = crate::proxy::trust::TRUST_PROBE_TIMEOUT + Duration::from_secs(1);
        assert!(
            budget > crate::proxy::trust::TRUST_PROBE_TIMEOUT,
            "the outer deadline would fire before the probe could reap its child"
        );
    }

    /// A probe that gave up is not a probe that found a problem. Only `Fail`
    /// prints the "run `pitchfork proxy setup`" summary, so mapping a timeout
    /// to it would send someone to re-run setup over a wedged resolver that
    /// setup cannot fix.
    #[test]
    fn a_resolver_that_timed_out_is_not_reported_as_a_broken_name() {
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        // No PAC file in use, and the system said so.
        let no_pac = Some(false);

        let timed_out = resolution_check("app.test", "test", no_pac, false, None);
        assert_eq!(timed_out.status, Status::Warn);
        assert!(
            timed_out.detail.contains("did not answer in time"),
            "the timeout was not explained: {}",
            timed_out.detail
        );

        // A resolver that answered "no such name" is a real failure, and says
        // what the resolver said rather than guessing.
        let refused = resolution_check(
            "app.test",
            "test",
            no_pac,
            false,
            Some(Err("Name or service not known".into())),
        );
        assert_eq!(refused.status, Status::Fail);
        assert!(refused.detail.contains("Name or service not known"));

        // An empty answer is a failure too, with nothing to quote.
        assert_eq!(
            resolution_check("app.test", "test", no_pac, false, Some(Ok(vec![]))).status,
            Status::Fail
        );

        let found = resolution_check(
            "app.test",
            "test",
            no_pac,
            false,
            Some(Ok(vec![ip("127.0.0.1")])),
        );
        assert_eq!(found.status, Status::Pass);
        assert!(found.detail.contains("127.0.0.1"));

        // Under PAC the system resolver is not consulted at all, so none of
        // the above is a problem worth reporting.
        for answer in [None, Some(Err("boom".to_string())), Some(Ok(vec![]))] {
            assert_eq!(
                resolution_check("app.test", "test", Some(true), false, answer).status,
                Status::Pass,
                "a PAC setup was told to fix its DNS"
            );
        }

        // And when the system would not say whether a PAC file is in use, a
        // name that does not resolve is not yet evidence of anything: under a
        // PAC file it would be expected. Warn rather than send the user to
        // re-run setup.
        for answer in [None, Some(Err("boom".to_string())), Some(Ok(vec![]))] {
            assert_eq!(
                resolution_check("app.test", "test", None, false, answer).status,
                Status::Warn,
                "an unreadable proxy configuration was reported as broken DNS"
            );
        }
        // LAN mode answers over mDNS, which setup does not configure, so a
        // name that will not resolve is not grounds to re-run setup. It is
        // still a browser on this machine unable to reach the advertised URL,
        // so it must not pass silently either.
        for answer in [Some(Err("boom".to_string())), Some(Ok(vec![]))] {
            let c = resolution_check("app.local", "local", no_pac, true, answer);
            assert_eq!(c.status, Status::Warn, "a broken mDNS path was hidden");
            assert!(
                c.detail.contains("mDNS"),
                "the warning did not say where to look: {}",
                c.detail
            );
            // It names `proxy setup` only to say setup is not the fix here,
            // so the check is against the instruction the failure branch
            // gives, not against the words appearing at all.
            assert!(
                !c.detail.contains("run `pitchfork proxy setup`"),
                "LAN mode was told to re-run setup: {}",
                c.detail
            );
        }
        // A lookup that timed out says so, in LAN mode or with the PAC state
        // unknown, rather than being reported as a name that did not resolve.
        for (pac, lan) in [(no_pac, true), (None, false)] {
            let c = resolution_check("app.local", "local", pac, lan, None);
            assert_eq!(c.status, Status::Warn);
            assert!(c.detail.contains("did not answer in time"), "{}", c.detail);
        }
        // A name that does resolve in LAN mode passes as usual.
        assert_eq!(
            resolution_check(
                "app.local",
                "local",
                no_pac,
                true,
                Some(Ok(vec![ip("192.168.1.10")]))
            )
            .status,
            Status::Pass
        );

        // A name that does resolve still passes, whatever the PAC state.
        assert_eq!(
            resolution_check(
                "app.test",
                "test",
                None,
                false,
                Some(Ok(vec![ip("127.0.0.1")]))
            )
            .status,
            Status::Pass
        );
    }

    /// A probe that never finishes must not take `proxy doctor` with it. The
    /// assertion is on the result, not the elapsed time, so this cannot go
    /// flaky under load: if the deadline did not hold, the test would hang
    /// instead of failing intermittently. Time is paused, so it does not
    /// sleep through the deadline either.
    #[tokio::test(start_paused = true)]
    async fn a_blocking_probe_that_never_answers_gives_up() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let stuck = bounded_blocking(move || {
            // Blocks until the sender is dropped at the end of the test, which
            // paused time places well past the deadline.
            let _ = wait.recv();
            "answered"
        });
        assert_eq!(stuck.await, None, "the deadline did not hold");
        drop(release);
    }

    /// The companion case, on real time: paused time would fire the deadline
    /// before the thread could answer, so this cannot share the test above.
    #[tokio::test]
    async fn a_blocking_probe_that_answers_returns_its_value() {
        assert_eq!(bounded_blocking(|| "answered").await, Some("answered"));
    }

    #[test]
    fn probe_names_are_unique_and_under_the_tld() {
        let a = probe_name("test");
        let b = probe_name("test");
        assert!(a.ends_with(".test"));
        assert_ne!(a, b, "each run must use a name nothing could have cached");
    }

    #[test]
    fn the_standard_port_is_not_expected_under_pac() {
        // PAC sends the browser straight to `proxy.port`, so setup installs no
        // redirect. Reporting a missing one would point the user at a fix for
        // a configuration that already works.
        let pac = Check::new(
            "standard port",
            Status::Pass,
            "not needed: the PAC file sends requests to port 8443",
        );
        assert_eq!(pac.status, Status::Pass);
        assert!(pac.line().contains("not needed"));
    }

    #[test]
    fn a_pac_url_only_counts_when_automatic_proxy_is_on() {
        // A leftover URL with automatic proxy switched off routes nothing, so
        // treating it as active would report a broken setup as healthy.
        let enabled_yes = |o: &str| {
            o.lines().any(|l| {
                let l = l.trim().to_ascii_lowercase();
                l.starts_with("enabled:") && l.ends_with("yes")
            })
        };
        assert!(enabled_yes(
            "URL: http://127.0.0.1:8443/proxy.pac\nEnabled: Yes\n"
        ));
        assert!(!enabled_yes(
            "URL: http://127.0.0.1:8443/proxy.pac\nEnabled: No\n"
        ));

        // GNOME consults the URL only in `auto` mode, and gsettings quotes it.
        let is_auto = |o: &str| o.trim().trim_matches('\'') == "auto";
        assert!(is_auto("'auto'\n"));
        assert!(!is_auto("'none'\n"));
        assert!(!is_auto("'manual'\n"));
    }

    #[test]
    fn check_lines_are_single_line_and_labelled() {
        let line = Check::new("dns resolver", Status::Fail, "no reply").line();
        assert!(!line.contains('\n'));
        assert!(line.starts_with("[fail] dns resolver"));
        assert!(line.ends_with("no reply"));
    }

    #[tokio::test]
    async fn responder_probe_reads_back_the_answer() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let task = tokio::spawn({
            let cancel = cancel.clone();
            async move {
                super::super::dns::serve(
                    super::super::dns::ResolverConfig::loopback("test"),
                    addr,
                    tx,
                    cancel,
                )
                .await
            }
        });
        rx.await.unwrap().unwrap();

        let v4 = std::net::IpAddr::V4(Ipv4Addr::LOCALHOST);
        let ip = query_responder(&probe_name("test"), addr.port(), v4)
            .await
            .expect("responder answers");
        assert_eq!(ip, v4);

        // An IPv6-only proxy serves AAAA and no A, so the probe has to ask for
        // the record the bind address implies or it would read a working
        // resolver as down.
        let v6 = std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST);
        let dual = super::super::dns::ResolverConfig::for_bind("test", v6);
        assert_eq!(dual.ipv4, None);

        // A name outside the TLD comes back REFUSED, which the probe reports as
        // a failure rather than a bogus address.
        let err = query_responder("example.com", addr.port(), v4)
            .await
            .unwrap_err();
        assert!(err.contains("rcode 5"), "unexpected error: {err}");

        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
    }
}
