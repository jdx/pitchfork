//! TLS ClientHello parsing, used to route a connection before any TLS
//! handshake takes place.
//!
//! The proxy peeks at the bytes a client sent without consuming them, reads the
//! Server Name Indication (SNI) extension, and uses the hostname to decide
//! whether to terminate TLS itself or splice the raw stream to a daemon that
//! terminates TLS on its own. Because the bytes are only peeked, the same
//! ClientHello is still there for whichever path is chosen.
//!
//! Only the parts of RFC 8446 §4.1.2 / RFC 6066 §3 needed for that decision are
//! parsed. Nothing here validates the handshake; rustls does that on the
//! terminate path, and the daemon does it on the passthrough path.

/// TLS record content type for handshake records.
const CONTENT_TYPE_HANDSHAKE: u8 = 0x16;

/// Handshake message type for a ClientHello.
const HANDSHAKE_TYPE_CLIENT_HELLO: u8 = 0x01;

/// Extension type for `server_name` (RFC 6066 §3).
const EXTENSION_SERVER_NAME: u16 = 0x0000;

/// `NameType.host_name` inside the `server_name` extension.
const NAME_TYPE_HOST_NAME: u8 = 0x00;

/// The outcome of looking for an SNI hostname in the bytes read so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SniPeek {
    /// The ClientHello carried a `server_name` extension with this hostname.
    /// Lowercased, since host names are case-insensitive (RFC 4343).
    Found(String),
    /// A complete ClientHello was parsed and it carried no usable SNI
    /// hostname — an IP-address connection, or a client that omits the
    /// extension.
    Absent,
    /// The bytes end mid-message: a complete ClientHello may still arrive.
    /// The caller should read more and try again.
    Incomplete,
    /// The bytes cannot be the start of a TLS handshake, so no amount of
    /// waiting will produce a ClientHello.
    NotTls,
}

/// A cursor over a byte slice that yields `None` instead of panicking when the
/// slice runs out, so a truncated ClientHello reads as [`SniPeek::Incomplete`]
/// rather than an index-out-of-bounds.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn u8(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn u16(&mut self) -> Option<u16> {
        let hi = self.u8()? as u16;
        let lo = self.u8()? as u16;
        Some((hi << 8) | lo)
    }

    fn u24(&mut self) -> Option<u32> {
        let a = self.u8()? as u32;
        let b = self.u8()? as u32;
        let c = self.u8()? as u32;
        Some((a << 16) | (b << 8) | c)
    }

    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    /// Skip a length-prefixed block whose length is `len_bytes` wide.
    fn skip_prefixed(&mut self, len_bytes: usize) -> Option<()> {
        let len = match len_bytes {
            1 => self.u8()? as usize,
            2 => self.u16()? as usize,
            _ => return None,
        };
        self.bytes(len).map(|_| ())
    }
}

/// Look for an SNI hostname in `buf`, which holds the first bytes a client
/// sent on a connection.
///
/// A ClientHello may be split across several TLS records, so the handshake
/// bytes are reassembled from every complete handshake record in `buf` before
/// being parsed.
///
/// Reassembly stops at the first record of another content type rather than
/// rejecting the connection: a TLS 1.3 client may follow its hello with a
/// compatibility ChangeCipherSpec or early data in the same flight, and the
/// hello that arrived before it is still the thing to route on. Only a
/// connection whose *first* record is not a handshake is ruled out.
pub fn parse_sni(buf: &[u8]) -> SniPeek {
    let mut records = Reader::new(buf);
    let mut handshake: Vec<u8> = Vec::new();

    loop {
        if records.remaining() == 0 {
            break;
        }
        // A record header is 5 bytes: type (1), legacy version (2), length (2).
        if records.remaining() < 5 {
            // Not even a full header. With handshake bytes already in hand the
            // partial record cannot add to them, so parse what arrived;
            // otherwise the first byte is enough to rule out a non-TLS client
            // such as a plain HTTP request.
            if !handshake.is_empty() {
                break;
            }
            return match records.u8() {
                Some(CONTENT_TYPE_HANDSHAKE) => SniPeek::Incomplete,
                Some(_) => SniPeek::NotTls,
                None => SniPeek::Incomplete,
            };
        }
        let Some(content_type) = records.u8() else {
            return SniPeek::Incomplete;
        };
        if content_type != CONTENT_TYPE_HANDSHAKE {
            // Another content type ends the handshake bytes. Whatever hello
            // already arrived is what this connection is routed on; with none,
            // the connection never started a handshake at all.
            if !handshake.is_empty() {
                break;
            }
            return SniPeek::NotTls;
        }
        // Legacy record version: not checked, a ClientHello advertises its
        // real version inside the handshake body and via extensions.
        if records.u16().is_none() {
            return SniPeek::Incomplete;
        }
        let Some(len) = records.u16() else {
            return SniPeek::Incomplete;
        };
        match records.bytes(len as usize) {
            Some(payload) => handshake.extend_from_slice(payload),
            // The record is announced but has not fully arrived.
            None => break,
        }
    }

    parse_handshake(&handshake)
}

/// Parse reassembled handshake bytes and return the SNI hostname.
fn parse_handshake(handshake: &[u8]) -> SniPeek {
    let mut r = Reader::new(handshake);

    match r.u8() {
        Some(HANDSHAKE_TYPE_CLIENT_HELLO) => {}
        // A handshake that opens with anything else is not a new client
        // connection (e.g. a mid-session renegotiation reaching us by
        // mistake); there is no hostname to route on.
        Some(_) => return SniPeek::NotTls,
        None => return SniPeek::Incomplete,
    }

    let Some(body_len) = r.u24() else {
        return SniPeek::Incomplete;
    };
    if r.remaining() < body_len as usize {
        return SniPeek::Incomplete;
    }

    // client_version (2) + random (32)
    if r.bytes(34).is_none() {
        return SniPeek::Incomplete;
    }
    // legacy_session_id, cipher_suites, legacy_compression_methods
    if r.skip_prefixed(1).is_none() || r.skip_prefixed(2).is_none() || r.skip_prefixed(1).is_none()
    {
        return SniPeek::Incomplete;
    }

    // Extensions are optional in the wire format (SSLv3-era hellos omit them).
    if r.remaining() == 0 {
        return SniPeek::Absent;
    }
    let Some(ext_total) = r.u16() else {
        return SniPeek::Incomplete;
    };
    let Some(ext_bytes) = r.bytes(ext_total as usize) else {
        return SniPeek::Incomplete;
    };

    let mut ext = Reader::new(ext_bytes);
    while ext.remaining() > 0 {
        let (Some(ext_type), Some(ext_len)) = (ext.u16(), ext.u16()) else {
            return SniPeek::Incomplete;
        };
        let Some(body) = ext.bytes(ext_len as usize) else {
            return SniPeek::Incomplete;
        };
        if ext_type == EXTENSION_SERVER_NAME {
            return parse_server_name_list(body);
        }
    }

    SniPeek::Absent
}

/// Parse the `ServerNameList` body of a `server_name` extension.
fn parse_server_name_list(body: &[u8]) -> SniPeek {
    let mut r = Reader::new(body);
    let Some(list_len) = r.u16() else {
        return SniPeek::Incomplete;
    };
    let Some(list) = r.bytes(list_len as usize) else {
        return SniPeek::Incomplete;
    };

    let mut names = Reader::new(list);
    while names.remaining() > 0 {
        let (Some(name_type), Some(name_len)) = (names.u8(), names.u16()) else {
            return SniPeek::Incomplete;
        };
        let Some(name) = names.bytes(name_len as usize) else {
            return SniPeek::Incomplete;
        };
        if name_type != NAME_TYPE_HOST_NAME {
            continue;
        }
        // A host name is ASCII on the wire (an internationalized name is sent
        // A-label encoded), so anything else is not routable.
        return match std::str::from_utf8(name) {
            Ok(host) if !host.is_empty() && host.is_ascii() => {
                SniPeek::Found(host.trim_end_matches('.').to_ascii_lowercase())
            }
            _ => SniPeek::Absent,
        };
    }

    SniPeek::Absent
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `server_name` extension body for one host name.
    fn server_name_ext(host: &str) -> Vec<u8> {
        let mut entry = vec![NAME_TYPE_HOST_NAME];
        entry.extend_from_slice(&(host.len() as u16).to_be_bytes());
        entry.extend_from_slice(host.as_bytes());

        let mut body = (entry.len() as u16).to_be_bytes().to_vec();
        body.extend_from_slice(&entry);
        body
    }

    /// Wrap an extension body with its type and length.
    fn extension(ext_type: u16, body: &[u8]) -> Vec<u8> {
        let mut out = ext_type.to_be_bytes().to_vec();
        out.extend_from_slice(&(body.len() as u16).to_be_bytes());
        out.extend_from_slice(body);
        out
    }

    /// A ClientHello handshake message carrying `extensions`.
    fn client_hello(extensions: Vec<Vec<u8>>) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]); // client_version TLS 1.2
        body.extend_from_slice(&[0x11; 32]); // random
        body.push(0); // empty legacy_session_id
        body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]); // one cipher suite
        body.extend_from_slice(&[0x01, 0x00]); // compression: null

        let ext_bytes: Vec<u8> = extensions.concat();
        body.extend_from_slice(&(ext_bytes.len() as u16).to_be_bytes());
        body.extend_from_slice(&ext_bytes);

        let mut msg = vec![HANDSHAKE_TYPE_CLIENT_HELLO];
        let len = body.len() as u32;
        msg.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
        msg.extend_from_slice(&body);
        msg
    }

    /// Wrap handshake bytes in a single TLS record.
    fn record(payload: &[u8]) -> Vec<u8> {
        let mut out = vec![CONTENT_TYPE_HANDSHAKE, 0x03, 0x01];
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// Split handshake bytes across several records of `chunk` bytes each, the
    /// shape a fragmenting client produces.
    fn records_of(payload: &[u8], chunk: usize) -> Vec<u8> {
        payload.chunks(chunk).flat_map(record).collect()
    }

    #[test]
    fn test_parse_sni_finds_hostname() {
        let hello = client_hello(vec![extension(
            EXTENSION_SERVER_NAME,
            &server_name_ext("api.localhost"),
        )]);
        assert_eq!(
            parse_sni(&record(&hello)),
            SniPeek::Found("api.localhost".to_string())
        );
    }

    #[test]
    fn test_parse_sni_lowercases_and_strips_root_dot() {
        // Host names are case-insensitive, and a trailing dot names the same
        // host, so both spellings must resolve to one routing key.
        let hello = client_hello(vec![extension(
            EXTENSION_SERVER_NAME,
            &server_name_ext("API.LocalHost."),
        )]);
        assert_eq!(
            parse_sni(&record(&hello)),
            SniPeek::Found("api.localhost".to_string())
        );
    }

    #[test]
    fn test_parse_sni_skips_other_extensions() {
        let hello = client_hello(vec![
            extension(0x002b, &[0x02, 0x03, 0x04]), // supported_versions
            extension(EXTENSION_SERVER_NAME, &server_name_ext("app.localhost")),
            extension(0x0010, &[0x00, 0x03, 0x02, b'h', b'2']), // ALPN
        ]);
        assert_eq!(
            parse_sni(&record(&hello)),
            SniPeek::Found("app.localhost".to_string())
        );
    }

    #[test]
    fn test_parse_sni_absent_without_extension() {
        // A client connecting by IP address sends no server_name extension.
        let hello = client_hello(vec![extension(0x002b, &[0x02, 0x03, 0x04])]);
        assert_eq!(parse_sni(&record(&hello)), SniPeek::Absent);
    }

    #[test]
    fn test_parse_sni_absent_with_empty_extension_list() {
        let hello = client_hello(vec![]);
        assert_eq!(parse_sni(&record(&hello)), SniPeek::Absent);
    }

    #[test]
    fn test_parse_sni_absent_when_extensions_omitted_entirely() {
        // An SSLv3-era hello ends after the compression list.
        let full = client_hello(vec![]);
        // Drop the 2-byte extensions length and the (empty) extension block.
        let mut body = full[4..].to_vec();
        body.truncate(body.len() - 2);
        let mut msg = vec![HANDSHAKE_TYPE_CLIENT_HELLO];
        let len = body.len() as u32;
        msg.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
        msg.extend_from_slice(&body);
        assert_eq!(parse_sni(&record(&msg)), SniPeek::Absent);
    }

    #[test]
    fn test_parse_sni_across_fragmented_records() {
        // A ClientHello split over several records still yields its hostname
        // once every fragment has arrived.
        let hello = client_hello(vec![extension(
            EXTENSION_SERVER_NAME,
            &server_name_ext("split.localhost"),
        )]);
        for chunk in [1usize, 5, 17, 64] {
            assert_eq!(
                parse_sni(&records_of(&hello, chunk)),
                SniPeek::Found("split.localhost".to_string()),
                "fragmented into {chunk}-byte records"
            );
        }
    }

    #[test]
    fn test_parse_sni_incomplete_prefixes_of_a_fragmented_hello() {
        // Every strict prefix of a fragmented ClientHello is incomplete: the
        // caller must wait rather than route on a half-read hello.
        let hello = client_hello(vec![extension(
            EXTENSION_SERVER_NAME,
            &server_name_ext("split.localhost"),
        )]);
        let wire = records_of(&hello, 7);
        for n in 1..wire.len() {
            assert_eq!(
                parse_sni(&wire[..n]),
                SniPeek::Incomplete,
                "prefix of {n} bytes should be incomplete"
            );
        }
        assert_eq!(
            parse_sni(&wire),
            SniPeek::Found("split.localhost".to_string())
        );
    }

    #[test]
    fn test_parse_sni_incomplete_on_truncated_record() {
        let hello = client_hello(vec![extension(
            EXTENSION_SERVER_NAME,
            &server_name_ext("api.localhost"),
        )]);
        let wire = record(&hello);
        assert_eq!(parse_sni(&wire[..wire.len() - 10]), SniPeek::Incomplete);
        assert_eq!(parse_sni(&[]), SniPeek::Incomplete);
        assert_eq!(parse_sni(&[CONTENT_TYPE_HANDSHAKE]), SniPeek::Incomplete);
    }

    /// A TLS 1.3 client may send a compatibility ChangeCipherSpec, or early
    /// data, right behind its hello. The hostname must still be found, or the
    /// connection would be terminated with the proxy's certificate instead of
    /// spliced to the daemon.
    #[test]
    fn test_parse_sni_finds_hostname_before_a_non_handshake_record() {
        let hello = client_hello(vec![extension(
            EXTENSION_SERVER_NAME,
            &server_name_ext("api.localhost"),
        )]);

        // ClientHello followed by a dummy ChangeCipherSpec record.
        let mut wire = record(&hello);
        wire.extend_from_slice(&[0x14, 0x03, 0x03, 0x00, 0x01, 0x01]);
        assert_eq!(
            parse_sni(&wire),
            SniPeek::Found("api.localhost".to_string())
        );

        // …and then application data, as an 0-RTT client sends.
        wire.extend_from_slice(&[0x17, 0x03, 0x03, 0x00, 0x03, 0xaa, 0xbb, 0xcc]);
        assert_eq!(
            parse_sni(&wire),
            SniPeek::Found("api.localhost".to_string())
        );

        // A fragmented hello followed by the same trailing record.
        let mut fragmented = records_of(&hello, 9);
        fragmented.extend_from_slice(&[0x14, 0x03, 0x03, 0x00, 0x01, 0x01]);
        assert_eq!(
            parse_sni(&fragmented),
            SniPeek::Found("api.localhost".to_string())
        );
    }

    /// A truncated record header behind a complete hello is not a reason to
    /// keep waiting: the hostname is already known.
    #[test]
    fn test_parse_sni_ignores_a_partial_trailing_record_header() {
        let hello = client_hello(vec![extension(
            EXTENSION_SERVER_NAME,
            &server_name_ext("api.localhost"),
        )]);
        let mut wire = record(&hello);
        wire.extend_from_slice(&[0x14, 0x03]);
        assert_eq!(
            parse_sni(&wire),
            SniPeek::Found("api.localhost".to_string())
        );
    }

    #[test]
    fn test_parse_sni_rejects_non_tls() {
        // A plain HTTP request on the TLS port, and an alert record, are both
        // ruled out immediately rather than waited on.
        assert_eq!(parse_sni(b"GET / HTTP/1.1\r\n"), SniPeek::NotTls);
        assert_eq!(
            parse_sni(&[0x15, 0x03, 0x01, 0x00, 0x02, 0x01, 0x00]),
            SniPeek::NotTls
        );
        assert_eq!(parse_sni(b"G"), SniPeek::NotTls);
    }

    #[test]
    fn test_parse_sni_rejects_non_client_hello_handshake() {
        // A ServerHello (type 2) is not a routable client connection.
        let mut msg = vec![0x02, 0x00, 0x00, 0x02, 0x03, 0x03];
        msg = record(&msg);
        assert_eq!(parse_sni(&msg), SniPeek::NotTls);
    }

    #[test]
    fn test_parse_sni_absent_for_non_ascii_hostname() {
        // Internationalized names travel A-label encoded; raw UTF-8 is not a
        // routable host name.
        let hello = client_hello(vec![extension(
            EXTENSION_SERVER_NAME,
            &server_name_ext("café.localhost"),
        )]);
        assert_eq!(parse_sni(&record(&hello)), SniPeek::Absent);
    }

    #[test]
    fn test_parse_sni_absent_for_empty_hostname() {
        let hello = client_hello(vec![extension(EXTENSION_SERVER_NAME, &server_name_ext(""))]);
        assert_eq!(parse_sni(&record(&hello)), SniPeek::Absent);
    }

    #[test]
    fn test_parse_sni_skips_unknown_name_types() {
        // An entry with an unknown NameType is skipped, and a following
        // host_name entry still resolves.
        let mut entries = vec![0x7f, 0x00, 0x02, 0xaa, 0xbb];
        let host = "second.localhost";
        entries.push(NAME_TYPE_HOST_NAME);
        entries.extend_from_slice(&(host.len() as u16).to_be_bytes());
        entries.extend_from_slice(host.as_bytes());
        let mut body = (entries.len() as u16).to_be_bytes().to_vec();
        body.extend_from_slice(&entries);

        let hello = client_hello(vec![extension(EXTENSION_SERVER_NAME, &body)]);
        assert_eq!(
            parse_sni(&record(&hello)),
            SniPeek::Found("second.localhost".to_string())
        );
    }

    #[test]
    fn test_parse_sni_does_not_panic_on_arbitrary_bytes() {
        // Length fields are attacker-controlled; every path must return a
        // verdict rather than index out of bounds.
        let hello = client_hello(vec![extension(
            EXTENSION_SERVER_NAME,
            &server_name_ext("api.localhost"),
        )]);
        let wire = record(&hello);
        for i in 0..wire.len() {
            for corruption in [0x00u8, 0xff, 0x7f] {
                let mut bad = wire.clone();
                bad[i] = corruption;
                let _ = parse_sni(&bad);
            }
        }
    }
}
