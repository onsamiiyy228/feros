//! Lightweight STUN Binding client for server-side IP discovery.
//!
//! Sends a STUN Binding Request to the configured STUN server and parses
//! the XOR-MAPPED-ADDRESS from the response to discover the public IP.
//!
//! Used by `connection.rs` to add a server-reflexive ICE candidate,
//! replacing the old `connect("8.8.8.8:80")` hack.
//!
//! Protocol reference: [RFC 5389](https://tools.ietf.org/html/rfc5389)

use std::net::SocketAddr;

use tokio::net::UdpSocket;
use tracing::{debug, warn};

/// STUN magic cookie (RFC 5389 §6).
const MAGIC_COOKIE: u32 = 0x2112_A442;

/// STUN Binding Request message type.
const BINDING_REQUEST: u16 = 0x0001;

/// STUN Binding Response message type.
const BINDING_RESPONSE: u16 = 0x0101;

/// XOR-MAPPED-ADDRESS attribute type.
const XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// MAPPED-ADDRESS attribute type (fallback for older servers).
const MAPPED_ADDRESS: u16 = 0x0001;

/// Resolve the server's public IP by sending a STUN Binding Request.
///
/// Binds a **separate** ephemeral UDP socket for the STUN exchange so the
/// main WebRTC socket is never polluted with non-ICE traffic. The returned
/// address is the server-reflexive mapping of this probe socket — the NAT
/// mapping for the *real* WebRTC socket may differ, but in practice the
/// public IP is the same (only the port may change).
///
/// # Timeout
///
/// Waits up to 2 seconds for a response. Returns `None` on timeout or error.
pub async fn stun_binding(stun_server: SocketAddr) -> Option<SocketAddr> {
    // Bind a throwaway socket — avoids eating ICE/DTLS packets off the
    // main WebRTC socket during the STUN exchange window.
    let socket = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => s,
        Err(e) => {
            warn!("STUN: failed to bind probe socket: {}", e);
            return None;
        }
    };

    // Build a STUN Binding Request (20 bytes: header only, no attributes)
    let mut request = [0u8; 20];

    // Message type: Binding Request
    request[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    // Message length: 0 (no attributes)
    request[2..4].copy_from_slice(&0u16.to_be_bytes());
    // Magic cookie
    request[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    // Transaction ID: 12 random bytes from uuid v4
    let txn_id = txn_id_from_uuid();
    request[8..20].copy_from_slice(&txn_id);

    // Send the request
    if let Err(e) = socket.send_to(&request, stun_server).await {
        warn!("STUN binding request failed: {}", e);
        return None;
    }

    // Wait for response (2s timeout)
    let mut buf = [0u8; 512];
    let len = match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        socket.recv_from(&mut buf),
    )
    .await
    {
        Ok(Ok((len, _))) => len,
        Ok(Err(e)) => {
            warn!("STUN recv failed: {}", e);
            return None;
        }
        Err(_) => {
            warn!("STUN binding timed out (2s)");
            return None;
        }
    };

    parse_stun_response(&buf[..len], &txn_id)
}

/// Parse a STUN Binding Response and extract the mapped address.
fn parse_stun_response(data: &[u8], expected_txn_id: &[u8; 12]) -> Option<SocketAddr> {
    if data.len() < 20 {
        return None;
    }

    // Check message type
    let msg_type = u16::from_be_bytes([data[0], data[1]]);
    if msg_type != BINDING_RESPONSE {
        debug!("STUN: not a binding response (type=0x{:04x})", msg_type);
        return None;
    }

    // Check magic cookie
    let cookie = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    if cookie != MAGIC_COOKIE {
        debug!("STUN: bad magic cookie");
        return None;
    }

    // Check transaction ID matches
    if data[8..20] != expected_txn_id[..] {
        debug!("STUN: transaction ID mismatch");
        return None;
    }

    // Parse attributes
    let msg_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    let attrs_end = (20 + msg_len).min(data.len());
    let mut offset = 20;

    while offset + 4 <= attrs_end {
        let attr_type = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let attr_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        let attr_start = offset + 4;
        let attr_end = attr_start + attr_len;

        if attr_end > attrs_end {
            break;
        }

        match attr_type {
            XOR_MAPPED_ADDRESS => {
                return parse_xor_mapped_address(&data[attr_start..attr_end]);
            }
            MAPPED_ADDRESS => {
                // Fallback: older STUN servers may only return MAPPED-ADDRESS
                return parse_mapped_address(&data[attr_start..attr_end]);
            }
            _ => {}
        }

        // Advance to next attribute (padded to 4-byte boundary)
        offset = attr_start + ((attr_len + 3) & !3);
    }

    debug!("STUN: no mapped address in response");
    None
}

/// Parse XOR-MAPPED-ADDRESS (RFC 5389 §15.2).
fn parse_xor_mapped_address(data: &[u8]) -> Option<SocketAddr> {
    if data.len() < 8 {
        return None;
    }

    let family = data[1];
    let x_port = u16::from_be_bytes([data[2], data[3]]);
    let port = x_port ^ (MAGIC_COOKIE >> 16) as u16;

    match family {
        0x01 => {
            // IPv4
            let x_addr = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
            let addr = x_addr ^ MAGIC_COOKIE;
            let ip = std::net::Ipv4Addr::from(addr);
            Some(SocketAddr::new(ip.into(), port))
        }
        0x02 => {
            // IPv6 (less common but handle it)
            if data.len() < 20 {
                return None;
            }
            debug!("STUN: IPv6 XOR-MAPPED-ADDRESS not yet supported");
            None
        }
        _ => None,
    }
}

/// Parse MAPPED-ADDRESS (RFC 5389 §15.1) — fallback for old servers.
fn parse_mapped_address(data: &[u8]) -> Option<SocketAddr> {
    if data.len() < 8 {
        return None;
    }

    let family = data[1];
    let port = u16::from_be_bytes([data[2], data[3]]);

    match family {
        0x01 => {
            let ip = std::net::Ipv4Addr::new(data[4], data[5], data[6], data[7]);
            Some(SocketAddr::new(ip.into(), port))
        }
        _ => None,
    }
}

/// Generate 12 random bytes for the STUN transaction ID from a UUID v4.
fn txn_id_from_uuid() -> [u8; 12] {
    let uuid = uuid::Uuid::new_v4();
    let mut out = [0u8; 12];
    out.copy_from_slice(&uuid.as_bytes()[..12]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal STUN Binding Response with a XOR-MAPPED-ADDRESS attribute.
    ///
    /// Returns (packet_bytes, transaction_id).
    fn build_stun_response(ip: std::net::Ipv4Addr, port: u16) -> (Vec<u8>, [u8; 12]) {
        let txn_id: [u8; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

        // XOR-MAPPED-ADDRESS attribute (12 bytes: 4 header + 8 value)
        let x_port = port ^ (MAGIC_COOKIE >> 16) as u16;
        let x_addr = u32::from(ip) ^ MAGIC_COOKIE;

        let mut attr = Vec::new();
        attr.extend_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes()); // type
        attr.extend_from_slice(&8u16.to_be_bytes()); // length
        attr.push(0x00); // reserved
        attr.push(0x01); // family: IPv4
        attr.extend_from_slice(&x_port.to_be_bytes());
        attr.extend_from_slice(&x_addr.to_be_bytes());

        // STUN header (20 bytes)
        let msg_len = attr.len() as u16;
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&BINDING_RESPONSE.to_be_bytes());
        pkt.extend_from_slice(&msg_len.to_be_bytes());
        pkt.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        pkt.extend_from_slice(&txn_id);
        pkt.extend_from_slice(&attr);

        (pkt, txn_id)
    }

    /// Build a STUN response with MAPPED-ADDRESS (non-XOR, for old servers).
    fn build_mapped_address_response(ip: std::net::Ipv4Addr, port: u16) -> (Vec<u8>, [u8; 12]) {
        let txn_id: [u8; 12] = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];

        let mut attr = Vec::new();
        attr.extend_from_slice(&MAPPED_ADDRESS.to_be_bytes());
        attr.extend_from_slice(&8u16.to_be_bytes());
        attr.push(0x00); // reserved
        attr.push(0x01); // family: IPv4
        attr.extend_from_slice(&port.to_be_bytes());
        attr.extend_from_slice(&ip.octets());

        let msg_len = attr.len() as u16;
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&BINDING_RESPONSE.to_be_bytes());
        pkt.extend_from_slice(&msg_len.to_be_bytes());
        pkt.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        pkt.extend_from_slice(&txn_id);
        pkt.extend_from_slice(&attr);

        (pkt, txn_id)
    }

    #[test]
    fn xor_mapped_address_ipv4() {
        let ip = std::net::Ipv4Addr::new(203, 0, 113, 42);
        let port = 12345;
        let (pkt, txn_id) = build_stun_response(ip, port);

        let result = parse_stun_response(&pkt, &txn_id);
        assert_eq!(result, Some(SocketAddr::new(ip.into(), port)));
    }

    #[test]
    fn mapped_address_fallback() {
        let ip = std::net::Ipv4Addr::new(198, 51, 100, 1);
        let port = 54321;
        let (pkt, txn_id) = build_mapped_address_response(ip, port);

        let result = parse_stun_response(&pkt, &txn_id);
        assert_eq!(result, Some(SocketAddr::new(ip.into(), port)));
    }

    #[test]
    fn rejects_wrong_txn_id() {
        let (pkt, _) = build_stun_response(std::net::Ipv4Addr::new(1, 2, 3, 4), 80);
        let wrong_txn: [u8; 12] = [99; 12];

        assert_eq!(parse_stun_response(&pkt, &wrong_txn), None);
    }

    #[test]
    fn rejects_bad_magic_cookie() {
        let (mut pkt, txn_id) = build_stun_response(std::net::Ipv4Addr::new(1, 2, 3, 4), 80);
        // Corrupt magic cookie
        pkt[4] = 0xFF;

        assert_eq!(parse_stun_response(&pkt, &txn_id), None);
    }

    #[test]
    fn rejects_non_binding_response() {
        let (mut pkt, txn_id) = build_stun_response(std::net::Ipv4Addr::new(1, 2, 3, 4), 80);
        // Change msg type to Binding Request
        pkt[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());

        assert_eq!(parse_stun_response(&pkt, &txn_id), None);
    }

    #[test]
    fn rejects_too_short_packet() {
        let txn_id = [0u8; 12];
        assert_eq!(parse_stun_response(&[0; 10], &txn_id), None);
        assert_eq!(parse_stun_response(&[], &txn_id), None);
    }

    #[test]
    fn txn_id_from_uuid_is_12_bytes_and_nonzero() {
        let id = txn_id_from_uuid();
        assert_eq!(id.len(), 12);
        // Should not be all zeros (extremely unlikely with uuid v4)
        assert!(id.iter().any(|&b| b != 0));
    }

    #[test]
    fn two_txn_ids_differ() {
        let a = txn_id_from_uuid();
        let b = txn_id_from_uuid();
        assert_ne!(a, b);
    }
}
