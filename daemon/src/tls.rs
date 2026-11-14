use anyhow::{Context, Result};
use rustls_pki_types::{CertificateDer, pem::PemObject};
use sha2::{Digest, Sha256};

use crate::config::Paths;

pub struct Tls {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    /// Lowercase hex SHA-256 of the DER certificate; the phone pins this.
    pub fingerprint: String,
}

/// Load the self-signed certificate, generating it on first run.
pub fn load_or_create(paths: &Paths, hostname: &str) -> Result<Tls> {
    let (cert_path, key_path) = (paths.cert(), paths.key());
    if !cert_path.exists() || !key_path.exists() {
        let sans = vec!["vulcan.local".to_string(), format!("{hostname}.local")];
        let ck = rcgen::generate_simple_self_signed(sans).context("generating certificate")?;
        std::fs::write(&cert_path, ck.cert.pem())?;
        crate::util::write_private(&key_path, ck.signing_key.serialize_pem().as_bytes())?;
        tracing::info!("generated new TLS certificate");
    }
    let cert_pem = std::fs::read(&cert_path)?;
    let key_pem = std::fs::read(&key_path)?;
    let der = CertificateDer::from_pem_slice(&cert_pem).context("parsing cert.pem")?;
    let fingerprint = hex::encode(Sha256::digest(der.as_ref()));
    Ok(Tls { cert_pem, key_pem, fingerprint })
}
