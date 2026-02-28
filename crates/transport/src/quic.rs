use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use quinn::{ClientConfig, Endpoint, ServerConfig};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use architect_core::transport::{Transport, TransportType};
use architect_core::types::NodeId;

/// QUIC transport — multiplexed, reliable, and low-latency.
pub struct QuicTransport {
    endpoint: Endpoint,
    connections: Arc<Mutex<HashMap<NodeId, quinn::Connection>>>,
    /// DER-encoded server certificate (available after server init).
    cert_der: Vec<u8>,
}

impl QuicTransport {
    /// Create a new QUIC transport bound to the given address.
    /// Returns the transport and the DER-encoded server certificate for pinning.
    pub async fn new(bind_addr: SocketAddr) -> Result<Self> {
        let (server_config, cert_der) = configure_server()?;
        let endpoint = Endpoint::server(server_config, bind_addr)?;

        info!("QUIC transport bound to {}", bind_addr);

        Ok(Self {
            endpoint,
            connections: Arc::new(Mutex::new(HashMap::new())),
            cert_der,
        })
    }

    /// Returns the DER-encoded server certificate for distribution to clients.
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// Connect to a remote peer with certificate pinning.
    /// `expected_cert_der` is the DER bytes of the server's certificate.
    pub async fn connect_pinned(
        &self,
        addr: SocketAddr,
        server_name: &str,
        expected_cert_der: &[u8],
    ) -> Result<quinn::Connection> {
        let client_config = configure_client_pinned(expected_cert_der.to_vec());
        let mut endpoint = self.endpoint.clone();
        endpoint.set_default_client_config(client_config);

        let connection = endpoint.connect(addr, server_name)?.await?;

        info!("QUIC connected to {} (cert pinned)", addr);
        Ok(connection)
    }

    /// Register a connection for a node.
    pub async fn register_connection(&self, node_id: NodeId, conn: quinn::Connection) {
        let mut conns = self.connections.lock().await;
        conns.insert(node_id, conn);
    }
}

#[async_trait]
impl Transport for QuicTransport {
    async fn send(&self, node_id: NodeId, data: &[u8]) -> Result<()> {
        let conns = self.connections.lock().await;
        let conn = conns
            .get(&node_id)
            .ok_or_else(|| anyhow::anyhow!("No QUIC connection for node {}", node_id))?;

        let mut send = conn.open_uni().await?;
        send.write_all(data).await?;
        send.finish()?;

        debug!("QUIC sent {} bytes to {}", data.len(), node_id);
        Ok(())
    }

    async fn receive(&self, _node_id: NodeId) -> Result<Vec<u8>> {
        // QUIC receive is typically handled via accept_uni()
        // This is a simplified implementation
        anyhow::bail!("Use accept loop for QUIC receive")
    }

    async fn broadcast(&self, data: &[u8]) -> Result<()> {
        let conns = self.connections.lock().await;
        for (node_id, conn) in conns.iter() {
            let mut send = conn.open_uni().await?;
            send.write_all(data).await?;
            send.finish()?;
            debug!("QUIC broadcast {} bytes to {}", data.len(), node_id);
        }
        Ok(())
    }

    async fn bandwidth_mbps(&self) -> f32 {
        500.0
    }

    async fn latency_ms(&self) -> f32 {
        5.0
    }

    async fn is_available(&self) -> bool {
        true
    }

    fn transport_type(&self) -> TransportType {
        TransportType::Quic
    }
}

/// Generate a self-signed server config for QUIC.
/// Returns the server config and the DER-encoded certificate for pinning.
fn configure_server() -> Result<(ServerConfig, Vec<u8>)> {
    let cert = rcgen::generate_simple_self_signed(vec!["architect".into()])?;
    let cert_der = cert.cert.der().to_vec();
    let cert_chain = vec![rustls::pki_types::CertificateDer::from(cert.cert)];
    let key = rustls::pki_types::PrivateKeyDer::try_from(cert.key_pair.serialize_der())
        .map_err(|e| anyhow::anyhow!("Key conversion failed: {}", e))?;

    let server_config = ServerConfig::with_single_cert(cert_chain, key)?;
    Ok((server_config, cert_der))
}

/// Create a client config that pins to a specific server certificate.
/// The client will only accept connections to servers presenting this exact certificate.
fn configure_client_pinned(expected_cert_der: Vec<u8>) -> ClientConfig {
    let crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(CertPinningVerifier { expected_cert_der }))
        .with_no_client_auth();

    ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
            .expect("Failed to create QUIC client config"),
    ))
}

/// Save a certificate to the config directory for pinning.
pub fn save_cert(cert_der: &[u8]) -> Result<()> {
    let path = architect_core::config::data_dir().join("quic-cert.der");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, cert_der)?;
    info!("QUIC certificate saved to {:?}", path);
    Ok(())
}

/// Load a pinned certificate from the config directory.
pub fn load_cert() -> Result<Vec<u8>> {
    let path = architect_core::config::data_dir().join("quic-cert.der");
    let data = std::fs::read(&path)
        .map_err(|e| anyhow::anyhow!("No pinned cert at {:?}: {}", path, e))?;
    info!("Loaded pinned QUIC certificate from {:?}", path);
    Ok(data)
}

/// TLS certificate verifier that pins to a specific certificate's DER bytes.
#[derive(Debug)]
struct CertPinningVerifier {
    expected_cert_der: Vec<u8>,
}

impl rustls::client::danger::ServerCertVerifier for CertPinningVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.expected_cert_der.as_slice() {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            warn!("Certificate pinning failed: server cert does not match expected");
            Err(rustls::Error::General(
                "Certificate pinning failed: server certificate does not match".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ED25519,
        ]
    }
}
