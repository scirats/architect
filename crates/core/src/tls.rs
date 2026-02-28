use std::path::{Path, PathBuf};

use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use tracing::info;

use crate::config::data_dir;

/// Returns `~/.architect/certs/`.
pub fn certs_dir() -> PathBuf {
    data_dir().join("certs")
}

/// Paths for the generated TLS artifacts.
pub struct CertPaths {
    pub ca_cert: PathBuf,
    pub ca_key: PathBuf,
    pub server_cert: PathBuf,
    pub server_key: PathBuf,
}

/// Paths for a generated client certificate (mTLS).
pub struct ClientCertPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
}

impl CertPaths {
    pub fn default_paths() -> Self {
        let dir = certs_dir();
        Self {
            ca_cert: dir.join("ca.pem"),
            ca_key: dir.join("ca-key.pem"),
            server_cert: dir.join("server.pem"),
            server_key: dir.join("server-key.pem"),
        }
    }
}

/// Generate a self-signed CA and server certificate if they don't already exist.
/// Returns the paths to the generated files.
pub fn ensure_certs() -> anyhow::Result<CertPaths> {
    let paths = CertPaths::default_paths();
    let dir = certs_dir();
    std::fs::create_dir_all(&dir)?;

    if paths.ca_cert.exists() && paths.server_cert.exists() && paths.server_key.exists() {
        info!("TLS certs already exist at {:?}", dir);
        return Ok(paths);
    }

    info!("Generating self-signed TLS certificates in {:?}", dir);
    generate_certs(&paths)?;
    Ok(paths)
}

fn generate_certs(paths: &CertPaths) -> anyhow::Result<()> {
    // Generate CA
    let ca_key = KeyPair::generate()?;
    let mut ca_params = CertificateParams::new(vec!["Architect CA".to_string()])?;
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(rcgen::DnType::CommonName, "Architect CA");
    ca_dn.push(rcgen::DnType::OrganizationName, "Architect");
    ca_params.distinguished_name = ca_dn;
    let ca_cert = ca_params.self_signed(&ca_key)?;

    // Generate server cert signed by CA
    let server_key = KeyPair::generate()?;
    let mut server_params = CertificateParams::new(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "0.0.0.0".to_string(),
    ])?;
    let mut server_dn = DistinguishedName::new();
    server_dn.push(rcgen::DnType::CommonName, "Architect Server");
    server_dn.push(rcgen::DnType::OrganizationName, "Architect");
    server_params.distinguished_name = server_dn;
    let server_cert = server_params.signed_by(&server_key, &ca_cert, &ca_key)?;

    // Write PEM files (including CA key for mTLS client cert generation)
    atomic_write(&paths.ca_cert, ca_cert.pem())?;
    atomic_write(&paths.ca_key, ca_key.serialize_pem())?;
    atomic_write(&paths.server_cert, server_cert.pem())?;
    atomic_write(&paths.server_key, server_key.serialize_pem())?;

    // Restrict CA key permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&paths.ca_key, std::fs::Permissions::from_mode(0o600));
    }

    info!("TLS certificates generated successfully");
    Ok(())
}

/// Generate a client certificate signed by the CA, for mTLS.
/// Returns paths to the client cert and key files.
pub fn generate_client_cert(node_id: &str) -> anyhow::Result<ClientCertPaths> {
    let paths = CertPaths::default_paths();

    if !paths.ca_key.exists() {
        anyhow::bail!("CA key not found at {:?} — cannot generate client cert", paths.ca_key);
    }

    let ca_key_pem = std::fs::read_to_string(&paths.ca_key)?;
    let ca_key = KeyPair::from_pem(&ca_key_pem)?;

    let _ca_cert_pem = std::fs::read_to_string(&paths.ca_cert)?;
    let mut ca_cert_params = CertificateParams::new(vec!["Architect CA".to_string()])?;
    ca_cert_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(rcgen::DnType::CommonName, "Architect CA");
    ca_dn.push(rcgen::DnType::OrganizationName, "Architect");
    ca_cert_params.distinguished_name = ca_dn;
    let ca_cert = ca_cert_params.self_signed(&ca_key)?;

    let client_key = KeyPair::generate()?;
    let client_params = CertificateParams::new(vec![node_id.to_string()])?;
    let client_cert = client_params.signed_by(&client_key, &ca_cert, &ca_key)?;

    let dir = certs_dir();
    let cert_path = dir.join(format!("client-{}.pem", node_id));
    let key_path = dir.join(format!("client-{}-key.pem", node_id));

    atomic_write(&cert_path, client_cert.pem())?;
    atomic_write(&key_path, client_key.serialize_pem())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
    }

    info!("Generated client cert for node {}", node_id);
    Ok(ClientCertPaths {
        cert: cert_path,
        key: key_path,
    })
}

/// Load a client identity for mTLS (agent side).
pub fn load_client_identity(node_id: &str) -> anyhow::Result<tonic::transport::Identity> {
    let dir = certs_dir();
    let cert_path = dir.join(format!("client-{}.pem", node_id));
    let key_path = dir.join(format!("client-{}-key.pem", node_id));

    if !cert_path.exists() || !key_path.exists() {
        anyhow::bail!("Client cert not found for node {}", node_id);
    }

    let cert = std::fs::read_to_string(&cert_path)?;
    let key = std::fs::read_to_string(&key_path)?;
    Ok(tonic::transport::Identity::from_pem(cert, key))
}

/// Load the server identity (cert + key) for tonic TLS server config.
pub fn load_server_identity(paths: &CertPaths) -> anyhow::Result<tonic::transport::Identity> {
    let cert = std::fs::read_to_string(&paths.server_cert)?;
    let key = std::fs::read_to_string(&paths.server_key)?;
    Ok(tonic::transport::Identity::from_pem(cert, key))
}

/// Load the CA certificate for tonic TLS client config (agent side).
pub fn load_ca_certificate(ca_path: &Path) -> anyhow::Result<tonic::transport::Certificate> {
    let pem = std::fs::read_to_string(ca_path)?;
    Ok(tonic::transport::Certificate::from_pem(pem))
}

/// Write-then-rename for atomicity.
fn atomic_write(dest: &Path, content: impl AsRef<str>) -> anyhow::Result<()> {
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, content.as_ref())?;
    std::fs::rename(&tmp, dest)?;
    Ok(())
}
