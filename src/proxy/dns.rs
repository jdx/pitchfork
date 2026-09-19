//! Minimal loopback DNS responder for the proxy TLD.
//!
//! The query shape pitchfork has to answer is fixed: "is this name under the
//! configured TLD?".  Everything under the TLD resolves to the loopback
//! address (or the LAN IP in LAN mode), and everything else is NXDOMAIN.  That
//! is narrow enough that a hand-rolled responder is smaller and cheaper than
//! pulling in a full DNS server stack, so this module parses the question
//! section directly and writes the answer back by hand.
//!
//! The responder binds `127.0.0.1:<proxy.dns_port>` on both UDP and TCP.  It is
//! never authoritative for anything but the configured TLD, and it never
//! forwards: a name outside the TLD gets REFUSED, which is what lets the system
//! resolver move on to its next server.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};

/// Default port for the loopback resolver.
///
/// 5353 (the obvious choice) is taken by mDNSResponder on macOS, so the default
/// is moved well clear of it.
pub const DEFAULT_DNS_PORT: u16 = 15353;

/// Maximum size of a DNS message carried over UDP without EDNS0.
const MAX_UDP_PAYLOAD: usize = 512;

/// Maximum size of a DNS message carried over TCP, per RFC 1035 §4.2.2.
const MAX_TCP_MESSAGE: usize = 65535;

/// TTL advertised on answers, in seconds.
///
/// Short because the answer can change under a client: in LAN mode the monitor
/// re-detects the interface address and updates the responder in place via
/// [`update_lan_ip`], and a client holding the old address for long would keep
/// missing the proxy.
const TTL: u32 = 60;

/// How long the TCP accept loop waits after an error before trying again.
///
/// Long enough that a descriptor shortage cannot spin the task, short enough
/// that recovery is not noticeable.
const ACCEPT_ERROR_BACKOFF: std::time::Duration = std::time::Duration::from_millis(100);

/// How long a TCP client may leave a connection idle before it is dropped.
///
/// A DNS query arrives immediately or not at all; anything longer is a client
/// holding a socket open for no reason.
const TCP_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How often a refusal may be logged. See [`crate::proxy::LogThrottle`].
const REFUSAL_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Throttle for the "too many connections" warning.
static REFUSED_TCP: crate::proxy::LogThrottle = crate::proxy::LogThrottle::new();

/// Maximum number of TCP connections served at once.
///
/// The responder is a loopback service, so this only has to be larger than any
/// plausible burst of real queries. It bounds what a local process can pin by
/// opening sockets and never writing to them.
const MAX_TCP_CONNECTIONS: usize = 64;

const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const CLASS_IN: u16 = 1;

const RCODE_NOERROR: u16 = 0;
const RCODE_FORMERR: u16 = 1;
const RCODE_NOTIMP: u16 = 4;
const RCODE_REFUSED: u16 = 5;

const FLAG_QR: u16 = 0x8000;
const FLAG_AA: u16 = 0x0400;
const FLAG_TC: u16 = 0x0200;
const FLAG_RD: u16 = 0x0100;

/// What the responder answers with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolverConfig {
    /// TLD the responder is authoritative for, without a leading dot.
    pub tld: String,
    /// Address returned for `A` queries, or `None` to answer NODATA.
    ///
    /// `None` when the proxy listens on IPv6 only: there is no IPv4 address
    /// that would reach it, and NODATA sends the client to the `AAAA` record
    /// instead of to a closed port.
    pub ipv4: Option<Ipv4Addr>,
    /// Address returned for `AAAA` queries, or `None` to answer NODATA.
    ///
    /// Only set when the proxy actually accepts IPv6. Handing out an address
    /// nothing listens on is worse than handing out none: a client that prefers
    /// IPv6 would get connection refused, and the 80/443 redirects `proxy setup`
    /// installs are IPv4-only (`inet` in pf, `iptables` rather than `ip6tables`).
    pub ipv6: Option<Ipv6Addr>,
}

impl ResolverConfig {
    /// Loopback configuration for `tld`, IPv4 only.
    pub fn loopback(tld: impl Into<String>) -> Self {
        Self {
            tld: tld.into(),
            ipv4: Some(Ipv4Addr::LOCALHOST),
            ipv6: None,
        }
    }

    /// LAN configuration for `tld`: the detected interface address, IPv4 only.
    pub fn lan(tld: impl Into<String>, ip: Ipv4Addr) -> Self {
        Self {
            tld: tld.into(),
            ipv4: Some(ip),
            ipv6: None,
        }
    }

    /// Configuration for a proxy bound to `bind_ip`.
    ///
    /// The answers name addresses that actually reach the listener, so a client
    /// is never sent to a closed port:
    ///
    /// - A specific address is served for its own family, and the other family
    ///   gets NODATA, because nothing is listening there.
    /// - `0.0.0.0` becomes the IPv4 loopback.
    /// - `::` becomes both loopbacks: a wildcard IPv6 socket accepts IPv4 too
    ///   on a dual-stack host, which is the default on Linux and macOS.
    pub fn for_bind(tld: impl Into<String>, bind_ip: std::net::IpAddr) -> Self {
        let tld = tld.into();
        match bind_ip {
            std::net::IpAddr::V4(ip) if ip.is_unspecified() => Self {
                tld,
                ipv4: Some(Ipv4Addr::LOCALHOST),
                ipv6: None,
            },
            std::net::IpAddr::V4(ip) => Self {
                tld,
                ipv4: Some(ip),
                ipv6: None,
            },
            std::net::IpAddr::V6(ip) if ip.is_unspecified() => Self {
                tld,
                ipv4: Some(Ipv4Addr::LOCALHOST),
                ipv6: Some(Ipv6Addr::LOCALHOST),
            },
            std::net::IpAddr::V6(ip) => Self {
                tld,
                ipv4: None,
                ipv6: Some(ip),
            },
        }
    }

    /// Whether `name` falls under the configured TLD.
    ///
    /// The apex (`localhost`) matches as well as anything beneath it
    /// (`core.fix-refs.entiredb.localhost`). Comparison is ASCII
    /// case-insensitive, per RFC 4343.
    fn owns(&self, name: &str) -> bool {
        // Shared with the certificate resolver: what the proxy will resolve and
        // what its CA will sign for have to be the same set of names.
        super::owns_name(&self.tld, name)
    }
}

/// A parsed question section.
#[derive(Debug, PartialEq, Eq)]
struct Question {
    name: String,
    qtype: u16,
    qclass: u16,
    /// Offset just past the question, where the answer section starts.
    end: usize,
}

/// Parse the single question following the 12-byte header.
///
/// Queries never use name compression, so pointers are rejected rather than
/// followed.
fn parse_question(msg: &[u8]) -> Option<Question> {
    let mut pos = 12;
    let mut name = String::new();
    loop {
        let len = *msg.get(pos)? as usize;
        pos += 1;
        if len == 0 {
            break;
        }
        // Top two bits set marks a compression pointer, which is not legal in
        // a question we originated the parse from.
        if len & 0xC0 != 0 {
            return None;
        }
        let label = msg.get(pos..pos + len)?;
        pos += len;
        if !name.is_empty() {
            name.push('.');
        }
        name.push_str(&String::from_utf8_lossy(label));
        if name.len() > 255 {
            return None;
        }
    }
    let qtype = u16::from_be_bytes([*msg.get(pos)?, *msg.get(pos + 1)?]);
    let qclass = u16::from_be_bytes([*msg.get(pos + 2)?, *msg.get(pos + 3)?]);
    Some(Question {
        name,
        qtype,
        qclass,
        end: pos + 4,
    })
}

/// Build a response carrying only a header.
fn header_only(id: u16, flags: u16, rcode: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&(flags | rcode).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    out
}

/// Answer a DNS query.
///
/// Returns `None` when the datagram is not a query this responder should reply
/// to at all (truncated below the header, or itself a response).
pub fn handle_query(query: &[u8], cfg: &ResolverConfig) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([query[0], query[1]]);
    let req_flags = u16::from_be_bytes([query[2], query[3]]);
    if req_flags & FLAG_QR != 0 {
        // A response, not a query. Dropping it avoids packet ping-pong.
        return None;
    }
    let opcode = req_flags & 0x7800;
    let qdcount = u16::from_be_bytes([query[4], query[5]]);

    // Echo the opcode and the recursion-desired bit; the responder is
    // authoritative and never recurses, so RA stays clear.
    let base_flags = FLAG_QR | opcode | (req_flags & FLAG_RD);

    // Only standard queries (opcode 0) are implemented.
    if opcode != 0 {
        return Some(header_only(id, base_flags, RCODE_NOTIMP));
    }
    if qdcount != 1 {
        return Some(header_only(id, base_flags, RCODE_FORMERR));
    }
    let Some(q) = parse_question(query) else {
        return Some(header_only(id, base_flags, RCODE_FORMERR));
    };

    let owned = q.qclass == CLASS_IN && cfg.owns(&q.name);
    let answer = if !owned {
        None
    } else {
        match q.qtype {
            TYPE_A => cfg.ipv4.map(|ip| ip.octets().to_vec()),
            TYPE_AAAA => cfg.ipv6.map(|ip| ip.octets().to_vec()),
            _ => None,
        }
    };

    // Outside the TLD: REFUSED, not NXDOMAIN.  NXDOMAIN is an authoritative
    // "this name does not exist", which a stub resolver caches and acts on
    // without consulting its other servers — so answering it here would break
    // every lookup that reaches this responder by mistake.  REFUSED says "not
    // mine", which is what makes the stub move on to the next server.
    // Inside the TLD but no record of that type: NODATA (NOERROR with an empty
    // answer section), which is what stops a resolver from retrying.
    let rcode = if owned { RCODE_NOERROR } else { RCODE_REFUSED };
    let ancount: u16 = u16::from(answer.is_some());

    let mut out = Vec::with_capacity(query.len() + 32);
    out.extend_from_slice(&id.to_be_bytes());
    // The AA bit claims authority, so it is set only for the zone we serve.
    let aa = if owned { FLAG_AA } else { 0 };
    out.extend_from_slice(&(base_flags | aa | rcode).to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&ancount.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    out.extend_from_slice(&query[12..q.end]); // question, verbatim

    if let Some(rdata) = answer {
        // The question name always starts at offset 12, so the answer's owner
        // name is a compression pointer to it.
        out.extend_from_slice(&[0xC0, 0x0C]);
        out.extend_from_slice(&q.qtype.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out.extend_from_slice(&TTL.to_be_bytes());
        out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        out.extend_from_slice(&rdata);
    }

    Some(out)
}

/// Truncate a response to fit a UDP datagram, setting the TC bit so the client
/// retries over TCP.
fn truncate_for_udp(mut resp: Vec<u8>) -> Vec<u8> {
    if resp.len() <= MAX_UDP_PAYLOAD {
        return resp;
    }
    let flags = u16::from_be_bytes([resp[2], resp[3]]) | FLAG_TC;
    resp[2..4].copy_from_slice(&flags.to_be_bytes());
    // Drop the answer section; the header still describes the question.
    resp[6..8].copy_from_slice(&0u16.to_be_bytes());
    resp.truncate(MAX_UDP_PAYLOAD);
    resp
}

/// The configuration the running responder is answering from.
///
/// Shared so the LAN IP monitor can update the address without restarting the
/// responder: in LAN mode the interface address can change under us, and an
/// answer pointing at the old one is worse than no answer.
/// Replaced on each `serve`, not set once: a second responder in the same
/// process must be the one that address updates reach, or the first, dead
/// config would keep absorbing them.
static ACTIVE_CONFIG: std::sync::RwLock<Option<Arc<std::sync::RwLock<ResolverConfig>>>> =
    std::sync::RwLock::new(None);

/// Point the running responder at a new LAN address.
///
/// A no-op when the resolver is not running, or when it is not in LAN mode.
pub fn update_lan_ip(ip: Ipv4Addr) {
    let cfg = match ACTIVE_CONFIG.read() {
        Ok(active) => active.clone(),
        Err(e) => {
            log::warn!("Could not read the active DNS resolver config: {e}");
            return;
        }
    };
    let Some(cfg) = cfg else {
        return;
    };
    match cfg.write() {
        Ok(mut cfg) if cfg.ipv4 != Some(ip) => {
            log::info!("DNS resolver now answering *.{} with {ip}", cfg.tld);
            cfg.ipv4 = Some(ip);
        }
        Ok(_) => {}
        Err(e) => log::warn!("Could not update the DNS resolver address: {e}"),
    }
}

/// Run the responder on `addr` over both UDP and TCP until `cancel` fires.
///
/// `bind_tx` reports the bind result so the supervisor can surface a port
/// conflict immediately instead of discovering it from a log line.
pub async fn serve(
    cfg: ResolverConfig,
    addr: SocketAddr,
    bind_tx: tokio::sync::oneshot::Sender<std::result::Result<(), String>>,
    cancel: tokio_util::sync::CancellationToken,
) -> crate::Result<()> {
    let udp = match UdpSocket::bind(addr).await {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("DNS resolver failed to bind UDP {addr}: {e}");
            let _ = bind_tx.send(Err(msg.clone()));
            miette::bail!("{msg}");
        }
    };
    let tcp = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            let msg = format!("DNS resolver failed to bind TCP {addr}: {e}");
            let _ = bind_tx.send(Err(msg.clone()));
            miette::bail!("{msg}");
        }
    };
    let _ = bind_tx.send(Ok(()));
    {
        let answers = [
            cfg.ipv4.map(|ip| ip.to_string()),
            cfg.ipv6.map(|ip| ip.to_string()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        log::info!(
            "DNS resolver listening on {addr} (udp+tcp), answering *.{} with {answers}",
            cfg.tld,
        );
    }

    // Publish the config so the LAN monitor can update the address in place,
    // replacing whatever a previous `serve` left behind.
    let cfg = Arc::new(std::sync::RwLock::new(cfg));
    match ACTIVE_CONFIG.write() {
        Ok(mut active) => *active = Some(Arc::clone(&cfg)),
        Err(e) => log::warn!("Could not publish the DNS resolver config: {e}"),
    }

    /// Answer from the shared config, holding the read lock only for the call.
    fn answer(cfg: &std::sync::RwLock<ResolverConfig>, query: &[u8]) -> Option<Vec<u8>> {
        match cfg.read() {
            Ok(cfg) => handle_query(query, &cfg),
            Err(e) => {
                log::warn!("DNS resolver config lock poisoned: {e}");
                None
            }
        }
    }

    let mut buf = vec![0u8; MAX_UDP_PAYLOAD];
    let mut conns: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
    loop {
        while conns.try_join_next().is_some() {}
        tokio::select! {
            recv = udp.recv_from(&mut buf) => {
                let (len, peer) = match recv {
                    Ok(v) => v,
                    Err(e) => {
                        log::debug!("DNS UDP receive error: {e}");
                        // Same backoff as the accept arm below, for the same
                        // reason: a resource shortage makes the syscall fail
                        // at once and keep failing, so retrying eagerly pins a
                        // core and floods the log instead of waiting for a
                        // datagram. Raced against cancellation so the wait
                        // cannot hold shutdown up.
                        tokio::select! {
                            _ = tokio::time::sleep(ACCEPT_ERROR_BACKOFF) => continue,
                            _ = cancel.cancelled() => {
                                log::info!("DNS resolver shutting down");
                                break;
                            }
                        }
                    }
                };
                if let Some(resp) = answer(&cfg, &buf[..len])
                    && let Err(e) = udp.send_to(&truncate_for_udp(resp), peer).await
                {
                    log::debug!("DNS UDP send error to {peer}: {e}");
                }
            }
            accept = tcp.accept() => {
                let (stream, peer) = match accept {
                    Ok(v) => v,
                    Err(e) => {
                        log::debug!("DNS TCP accept error: {e}");
                        // Back off rather than retry straight away. A
                        // process-wide descriptor shortage — this process also
                        // runs the HTTP proxy, mDNS and the IPC listener —
                        // makes `accept` fail immediately and repeatedly
                        // instead of waiting for a connection, so an eager
                        // retry would spin a core and flood the log until a
                        // descriptor frees up.
                        //
                        // Raced against cancellation so the wait cannot hold
                        // shutdown up: this arm runs after the outer `select!`
                        // has already resolved, so without this the token would
                        // go unobserved for the length of the backoff.
                        tokio::select! {
                            _ = tokio::time::sleep(ACCEPT_ERROR_BACKOFF) => continue,
                            _ = cancel.cancelled() => {
                                log::info!("DNS resolver shutting down");
                                break;
                            }
                        }
                    }
                };
                // Drop the connection rather than queue it without bound: a
                // local process could otherwise pin a task and a socket per
                // connection by never sending the query it promised.
                if conns.len() >= MAX_TCP_CONNECTIONS {
                    // Throttled: a client can provoke this as fast as it can
                    // open sockets, and one line each would let it fill the
                    // disk while it is already being refused service.
                    if let Some(suppressed) = REFUSED_TCP.allow(REFUSAL_LOG_INTERVAL) {
                        log::warn!(
                            "DNS resolver refused a TCP connection from {peer}: \
                             {MAX_TCP_CONNECTIONS} already in flight \
                             ({suppressed} similar refusals since the last message)"
                        );
                    }
                    drop(stream);
                    continue;
                }
                let cfg = Arc::clone(&cfg);
                conns.spawn(async move {
                    if let Err(e) = serve_tcp_conn(stream, &cfg, TCP_IDLE_TIMEOUT).await {
                        log::debug!("DNS TCP connection from {peer} ended: {e}");
                    }
                });
            }
            _ = cancel.cancelled() => {
                log::info!("DNS resolver shutting down");
                break;
            }
        }
    }
    conns.abort_all();
    // Stop absorbing address updates: this responder is no longer answering.
    if let Ok(mut active) = ACTIVE_CONFIG.write()
        && active.as_ref().is_some_and(|c| Arc::ptr_eq(c, &cfg))
    {
        *active = None;
    }
    Ok(())
}

/// Serve queries on one TCP connection until the peer closes it.
///
/// DNS over TCP frames each message with a two-byte big-endian length, and a
/// connection may carry more than one query.
async fn serve_tcp_conn(
    mut stream: tokio::net::TcpStream,
    cfg: &std::sync::RwLock<ResolverConfig>,
    idle: std::time::Duration,
) -> std::io::Result<()> {
    /// A read that gives up rather than waiting on a client forever.
    async fn read_exact_timeout(
        stream: &mut tokio::net::TcpStream,
        buf: &mut [u8],
        idle: std::time::Duration,
    ) -> std::io::Result<()> {
        tokio::time::timeout(idle, stream.read_exact(buf))
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "idle DNS connection")
            })??;
        Ok(())
    }

    loop {
        let mut len_buf = [0u8; 2];
        match read_exact_timeout(&mut stream, &mut len_buf, idle).await {
            Ok(()) => {}
            // A clean close between messages is the normal end of a connection.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
        let len = usize::from(u16::from_be_bytes(len_buf));
        if len == 0 || len > MAX_TCP_MESSAGE {
            return Ok(());
        }
        let mut msg = vec![0u8; len];
        // The length prefix is a promise the client may not keep.
        read_exact_timeout(&mut stream, &mut msg, idle).await?;
        let Some(resp) = (match cfg.read() {
            Ok(cfg) => handle_query(&msg, &cfg),
            Err(_) => None,
        }) else {
            continue;
        };
        // DNS over TCP frames each message with its length, so a reply that
        // will not fit in that field cannot be sent at all. Clamping the
        // prefix and writing the whole body anyway — which is what
        // `unwrap_or(u16::MAX)` did — leaves the client reading the tail of
        // this reply as the length of the next one, so every message after it
        // on the connection is garbage. A query long enough to provoke this is
        // already at the 65535-byte limit before the header and answers are
        // added to it, so closing the connection is the honest response.
        let Ok(resp_len) = u16::try_from(resp.len()) else {
            log::debug!(
                "DNS reply of {} bytes cannot be framed over TCP; closing the connection",
                resp.len()
            );
            return Ok(());
        };
        // The write is bounded too: a client that sends queries and never reads
        // the answers fills the socket buffer, and an unbounded `write_all`
        // would then hold this connection slot open indefinitely.
        tokio::time::timeout(idle, async {
            stream.write_all(&resp_len.to_be_bytes()).await?;
            stream.write_all(&resp).await?;
            stream.flush().await
        })
        .await
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "DNS client not reading replies",
            )
        })??;
    }
}

/// Resolver config implied by the current settings.
///
/// `lan_ip` is the address detected for LAN mode, if any; without it the
/// responder stays on loopback even when `proxy.lan` is set, because handing
/// out an address the proxy is not reachable on would be worse than loopback.
pub fn config_from_settings(
    s: &crate::settings::Settings,
    lan_ip: Option<Ipv4Addr>,
) -> ResolverConfig {
    let lan_enabled = s.proxy.lan || !s.proxy.lan_ip.is_empty();
    // One definition of which TLD is in force, shared with the router and the
    // hostname builder: computing it separately is how the resolver and the
    // PAC file drifted apart before.
    let tld = crate::proxy::effective_tld(s).to_string();
    if lan_enabled {
        // LAN mode hands out the interface address, which is IPv4.
        return match lan_ip {
            Some(ip) => ResolverConfig::lan(tld, ip),
            None => ResolverConfig::loopback(tld),
        };
    }
    // Derived from the bind address, so every answer names something the proxy
    // is actually listening on. A configured `proxy.host` that is not an
    // address falls back to IPv4 loopback, matching what `serve` binds.
    let bind_ip = s
        .proxy
        .host
        .parse()
        .unwrap_or(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST));
    ResolverConfig::for_bind(tld, bind_ip)
}

/// The port the resolver should listen on, clamped into range.
pub fn dns_port(s: &crate::settings::Settings) -> u16 {
    u16::try_from(s.proxy.dns_port)
        .ok()
        .filter(|&p| p > 0)
        .unwrap_or_else(|| {
            log::warn!(
                "proxy.dns_port {} is out of valid port range (1-65535), using {DEFAULT_DNS_PORT}",
                s.proxy.dns_port
            );
            DEFAULT_DNS_PORT
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a query for `name` of type `qtype`.
    fn query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&FLAG_RD.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        for label in name.split('.') {
            out.push(label.len() as u8);
            out.extend_from_slice(label.as_bytes());
        }
        out.push(0);
        out.extend_from_slice(&qtype.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out
    }

    fn rcode(resp: &[u8]) -> u16 {
        u16::from_be_bytes([resp[2], resp[3]]) & 0x000F
    }

    fn ancount(resp: &[u8]) -> u16 {
        u16::from_be_bytes([resp[6], resp[7]])
    }

    /// The RDATA of the single answer record.
    fn rdata(resp: &[u8]) -> Vec<u8> {
        let q = parse_question(resp).expect("response echoes the question");
        let rdlen = usize::from(u16::from_be_bytes([resp[q.end + 10], resp[q.end + 11]]));
        resp[q.end + 12..q.end + 12 + rdlen].to_vec()
    }

    /// A dual-stack loopback config, as a proxy bound to `::` would serve.
    fn cfg() -> ResolverConfig {
        ResolverConfig::for_bind("localhost", "::".parse().unwrap())
    }

    #[test]
    fn a_query_under_tld_answers_loopback() {
        let resp = handle_query(&query(0x1234, "myapp.localhost", TYPE_A), &cfg()).unwrap();
        assert_eq!(&resp[0..2], &0x1234u16.to_be_bytes());
        assert_eq!(rcode(&resp), RCODE_NOERROR);
        assert_eq!(ancount(&resp), 1);
        assert_eq!(rdata(&resp), vec![127, 0, 0, 1]);
        // Authoritative answer, recursion desired echoed back, recursion not available.
        let flags = u16::from_be_bytes([resp[2], resp[3]]);
        assert_eq!(flags & FLAG_QR, FLAG_QR);
        assert_eq!(flags & FLAG_AA, FLAG_AA);
        assert_eq!(flags & FLAG_RD, FLAG_RD);
    }

    #[test]
    fn a_query_answers_multi_level_names() {
        // The hierarchical hostname shape: no wildcard depth limit applies.
        let resp = handle_query(
            &query(1, "core.fix-refs.entiredb.localhost", TYPE_A),
            &cfg(),
        )
        .unwrap();
        assert_eq!(rcode(&resp), RCODE_NOERROR);
        assert_eq!(rdata(&resp), vec![127, 0, 0, 1]);
    }

    #[test]
    fn tld_apex_resolves() {
        let resp = handle_query(&query(1, "localhost", TYPE_A), &cfg()).unwrap();
        assert_eq!(rcode(&resp), RCODE_NOERROR);
        assert_eq!(ancount(&resp), 1);
    }

    #[test]
    fn matching_is_case_insensitive() {
        let resp = handle_query(&query(1, "MyApp.LOCALHOST", TYPE_A), &cfg()).unwrap();
        assert_eq!(rcode(&resp), RCODE_NOERROR);
        assert_eq!(ancount(&resp), 1);
    }

    #[test]
    fn aaaa_query_answers_ipv6_loopback() {
        let resp = handle_query(&query(1, "myapp.localhost", TYPE_AAAA), &cfg()).unwrap();
        assert_eq!(rcode(&resp), RCODE_NOERROR);
        assert_eq!(ancount(&resp), 1);
        assert_eq!(rdata(&resp), Ipv6Addr::LOCALHOST.octets().to_vec());
    }

    #[test]
    fn lan_mode_answers_lan_ip_and_nodata_for_aaaa() {
        let cfg = ResolverConfig::lan("local", Ipv4Addr::new(192, 168, 1, 42));
        let a = handle_query(&query(1, "myapp.local", TYPE_A), &cfg).unwrap();
        assert_eq!(rdata(&a), vec![192, 168, 1, 42]);

        // No IPv6 equivalent to hand out: NODATA, not NXDOMAIN, so the client
        // falls back to the A record instead of treating the name as missing.
        let aaaa = handle_query(&query(1, "myapp.local", TYPE_AAAA), &cfg).unwrap();
        assert_eq!(rcode(&aaaa), RCODE_NOERROR);
        assert_eq!(ancount(&aaaa), 0);
    }

    #[test]
    fn name_outside_tld_is_refused_not_nxdomain() {
        let resp = handle_query(&query(1, "example.com", TYPE_A), &cfg()).unwrap();
        // REFUSED, so the stub resolver tries its next server. An
        // authoritative NXDOMAIN would end the lookup right here.
        assert_eq!(rcode(&resp), RCODE_REFUSED);
        assert_eq!(ancount(&resp), 0);
        // And no claim of authority over a zone we do not serve.
        assert_eq!(u16::from_be_bytes([resp[2], resp[3]]) & FLAG_AA, 0);
    }

    #[test]
    fn tld_suffix_without_label_boundary_is_refused() {
        // "notlocalhost" merely ends with the TLD's letters.
        let resp = handle_query(&query(1, "notlocalhost", TYPE_A), &cfg()).unwrap();
        assert_eq!(rcode(&resp), RCODE_REFUSED);
    }

    #[test]
    fn lan_mode_serves_ipv4_whatever_proxy_host_says() {
        // LAN mode hands out the detected interface address, which is IPv4,
        // regardless of `proxy.host`. Anything deriving the served family from
        // the bind address instead would get this wrong — `proxy doctor` did.
        let cfg = ResolverConfig::lan("local", Ipv4Addr::new(192, 168, 1, 42));
        assert_eq!(cfg.ipv4, Some(Ipv4Addr::new(192, 168, 1, 42)));
        assert_eq!(cfg.ipv6, None);
        // And with no address detected yet, still IPv4.
        let fallback = ResolverConfig::loopback("local");
        assert_eq!(fallback.ipv4, Some(Ipv4Addr::LOCALHOST));
        assert_eq!(fallback.ipv6, None);
    }

    #[test]
    fn answers_name_only_addresses_the_proxy_listens_on() {
        use std::net::IpAddr;

        // IPv6 only: there is no IPv4 address that reaches the listener, so an
        // A query gets NODATA rather than a loopback address nothing is on.
        let v6 = ResolverConfig::for_bind("test", IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(v6.ipv4, None);
        assert_eq!(v6.ipv6, Some(Ipv6Addr::LOCALHOST));
        let a = handle_query(&query(1, "x.test", TYPE_A), &v6).unwrap();
        assert_eq!(rcode(&a), RCODE_NOERROR);
        assert_eq!(ancount(&a), 0);
        let aaaa = handle_query(&query(1, "x.test", TYPE_AAAA), &v6).unwrap();
        assert_eq!(rdata(&aaaa), Ipv6Addr::LOCALHOST.octets().to_vec());

        // A specific address of either family is served as itself, not as
        // loopback, because loopback would not reach that listener.
        let specific_v4 = ResolverConfig::for_bind("test", "192.168.1.5".parse().unwrap());
        assert_eq!(specific_v4.ipv4, Some(Ipv4Addr::new(192, 168, 1, 5)));
        assert_eq!(specific_v4.ipv6, None);
        let specific_v6 = ResolverConfig::for_bind("test", "fd00::1".parse().unwrap());
        assert_eq!(specific_v6.ipv4, None);
        assert_eq!(specific_v6.ipv6, Some("fd00::1".parse().unwrap()));

        // Wildcards map to the loopback of the family they accept. A wildcard
        // IPv6 socket takes IPv4 too on a dual-stack host.
        let any_v4 = ResolverConfig::for_bind("test", "0.0.0.0".parse().unwrap());
        assert_eq!(any_v4.ipv4, Some(Ipv4Addr::LOCALHOST));
        assert_eq!(any_v4.ipv6, None);
        let any_v6 = ResolverConfig::for_bind("test", "::".parse().unwrap());
        assert_eq!(any_v6.ipv4, Some(Ipv4Addr::LOCALHOST));
        assert_eq!(any_v6.ipv6, Some(Ipv6Addr::LOCALHOST));
    }

    #[test]
    fn aaaa_is_nodata_unless_the_proxy_listens_on_ipv6() {
        // The default proxy binds 127.0.0.1, so there is no IPv6 address worth
        // handing out; NODATA sends the client to the A record instead.
        let v4_only = ResolverConfig::loopback("localhost");
        let resp = handle_query(&query(1, "myapp.localhost", TYPE_AAAA), &v4_only).unwrap();
        assert_eq!(rcode(&resp), RCODE_NOERROR);
        assert_eq!(ancount(&resp), 0);
        // The A record is still served.
        let a = handle_query(&query(1, "myapp.localhost", TYPE_A), &v4_only).unwrap();
        assert_eq!(ancount(&a), 1);
    }

    #[test]
    fn unsupported_record_type_under_tld_is_nodata() {
        const TYPE_MX: u16 = 15;
        let resp = handle_query(&query(1, "myapp.localhost", TYPE_MX), &cfg()).unwrap();
        assert_eq!(rcode(&resp), RCODE_NOERROR);
        assert_eq!(ancount(&resp), 0);
    }

    #[test]
    fn non_internet_class_is_refused() {
        let mut q = query(1, "myapp.localhost", TYPE_A);
        let len = q.len();
        q[len - 2..].copy_from_slice(&3u16.to_be_bytes()); // CLASS CH
        let resp = handle_query(&q, &cfg()).unwrap();
        assert_eq!(rcode(&resp), RCODE_REFUSED);
    }

    #[test]
    fn malformed_and_unsupported_messages() {
        // Shorter than a header: nothing to reply to.
        assert!(handle_query(&[0u8; 4], &cfg()).is_none());
        // A response, not a query.
        let mut resp_msg = query(1, "myapp.localhost", TYPE_A);
        resp_msg[2] |= 0x80;
        assert!(handle_query(&resp_msg, &cfg()).is_none());
        // Truncated question section.
        let q = query(1, "myapp.localhost", TYPE_A);
        let resp = handle_query(&q[..16], &cfg()).unwrap();
        assert_eq!(rcode(&resp), RCODE_FORMERR);
        // Non-query opcode (UPDATE = 5).
        let mut upd = query(1, "myapp.localhost", TYPE_A);
        upd[2] |= 5 << 3;
        let resp = handle_query(&upd, &cfg()).unwrap();
        assert_eq!(rcode(&resp), RCODE_NOTIMP);
    }

    #[test]
    fn compression_pointer_in_question_is_rejected() {
        let mut q = query(1, "myapp.localhost", TYPE_A);
        q[12] = 0xC0;
        let resp = handle_query(&q, &cfg()).unwrap();
        assert_eq!(rcode(&resp), RCODE_FORMERR);
    }

    #[tokio::test]
    async fn an_idle_tcp_client_is_dropped_rather_than_held() {
        // A client that announces a message and never sends it must not pin a
        // task and a socket. Driven directly so the timeout under test can be
        // short, rather than the ten seconds the server uses.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let idle = std::time::Duration::from_millis(50);

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve_tcp_conn(stream, &std::sync::RwLock::new(cfg()), idle).await
        });

        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        // Promise sixteen bytes, send none of them.
        client.write_all(&16u16.to_be_bytes()).await.unwrap();

        let err = tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .expect("the handler should give up on its own")
            .unwrap()
            .expect_err("an idle connection is an error, not a clean close");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);

        // And the client sees the socket closed.
        let mut buf = [0u8; 1];
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf))
            .await
            .expect("the connection is already closed")
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn the_tcp_connection_cap_is_bounded() {
        // The cap is what stops a local process pinning one task and socket per
        // connection; a generous but finite number for a loopback service.
        assert!((8..=1024).contains(&MAX_TCP_CONNECTIONS));
    }

    #[tokio::test]
    async fn serves_over_udp_and_tcp() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        // Port 0 lets the OS pick, but UDP and TCP must share a port, so probe
        // for a free one by binding TCP first and reusing its number.
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);

        let task = tokio::spawn({
            let cancel = cancel.clone();
            async move { serve(cfg(), addr, tx, cancel).await }
        });
        rx.await.unwrap().expect("resolver binds");

        let q = query(0x4242, "deep.nested.myapp.localhost", TYPE_A);

        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sock.send_to(&q, addr).await.unwrap();
        let mut buf = [0u8; 512];
        let (n, _) =
            tokio::time::timeout(std::time::Duration::from_secs(5), sock.recv_from(&mut buf))
                .await
                .expect("udp reply arrives")
                .unwrap();
        assert_eq!(rdata(&buf[..n]), vec![127, 0, 0, 1]);

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(&(q.len() as u16).to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&q).await.unwrap();
        let mut len_buf = [0u8; 2];
        stream.read_exact(&mut len_buf).await.unwrap();
        let mut resp = vec![0u8; usize::from(u16::from_be_bytes(len_buf))];
        stream.read_exact(&mut resp).await.unwrap();
        assert_eq!(rdata(&resp), vec![127, 0, 0, 1]);
        // A second query on the same connection is answered too.
        stream
            .write_all(&(q.len() as u16).to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&q).await.unwrap();
        stream.read_exact(&mut len_buf).await.unwrap();
        let mut resp2 = vec![0u8; usize::from(u16::from_be_bytes(len_buf))];
        stream.read_exact(&mut resp2).await.unwrap();
        assert_eq!(rcode(&resp2), RCODE_NOERROR);
        drop(stream);

        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
    }
}
