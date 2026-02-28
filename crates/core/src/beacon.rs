use serde::{Deserialize, Serialize};

/// Magic bytes to identify Architect beacons.
pub const BEACON_MAGIC: &[u8; 4] = b"ARCH";

/// Default UDP port for beacon broadcast/listen.
pub const BEACON_PORT: u16 = 9877;

/// Maximum beacon datagram size.
pub const BEACON_MAX_SIZE: usize = 512;

/// Beacon payload broadcast by the coordinator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorBeacon {
    /// The gRPC port the coordinator is listening on.
    pub grpc_port: u16,
    /// First 8 bytes of FNV-1a hash of the cluster token.
    /// Agents compare this locally to filter beacons without
    /// exposing the actual token over unencrypted UDP.
    pub token_hash: [u8; 8],
    /// Coordinator hostname for display.
    pub hostname: String,
    /// Coordinator node ID.
    pub client_id: String,
}

/// Serialize a beacon into a datagram: `MAGIC + bincode(payload)`.
pub fn encode_beacon(beacon: &CoordinatorBeacon) -> Result<Vec<u8>, bincode::Error> {
    let payload = bincode::serialize(beacon)?;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(BEACON_MAGIC);
    buf.extend_from_slice(&payload);
    Ok(buf)
}

/// Deserialize a beacon from a datagram. Returns `None` if magic bytes don't match.
pub fn decode_beacon(data: &[u8]) -> Option<CoordinatorBeacon> {
    if data.len() < 4 || &data[..4] != BEACON_MAGIC {
        return None;
    }
    bincode::deserialize(&data[4..]).ok()
}

/// Compute the 8-byte token hash for beacon matching.
/// Uses FNV-1a — NOT cryptographic, only for filtering beacons.
pub fn hash_token(token: &str) -> [u8; 8] {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in token.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h.to_le_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_beacon() {
        let beacon = CoordinatorBeacon {
            grpc_port: 9876,
            token_hash: hash_token("test-token"),
            hostname: "coordinator".into(),
            client_id: "abc-123".into(),
        };
        let encoded = encode_beacon(&beacon).unwrap();
        assert_eq!(&encoded[..4], BEACON_MAGIC);
        let decoded = decode_beacon(&encoded).unwrap();
        assert_eq!(decoded.grpc_port, 9876);
        assert_eq!(decoded.hostname, "coordinator");
        assert_eq!(decoded.token_hash, beacon.token_hash);
    }

    #[test]
    fn decode_invalid_magic() {
        assert!(decode_beacon(b"NOPE1234").is_none());
    }

    #[test]
    fn decode_too_short() {
        assert!(decode_beacon(b"AR").is_none());
    }

    #[test]
    fn token_hash_deterministic() {
        let h1 = hash_token("my-secret");
        let h2 = hash_token("my-secret");
        assert_eq!(h1, h2);
    }

    #[test]
    fn token_hash_differs() {
        let h1 = hash_token("token-a");
        let h2 = hash_token("token-b");
        assert_ne!(h1, h2);
    }
}
