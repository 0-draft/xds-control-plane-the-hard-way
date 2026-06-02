//! Self-signed certificate generation for SDS.
//!
//! "Static SDS" means the secret material originates here, in the control
//! plane, rather than from a real secret store. Each call mints a fresh
//! self-signed leaf, which is exactly what makes hot rotation observable: two
//! rotations produce two different certificates for the same SNI.

/// A PEM-encoded certificate and its private key.
pub struct CertPem {
    pub cert: String,
    pub key: String,
}

/// Generate a fresh self-signed certificate covering `sans`.
pub fn self_signed(sans: &[&str]) -> anyhow::Result<CertPem> {
    let san_strings: Vec<String> = sans.iter().map(|s| s.to_string()).collect();
    let signed = rcgen::generate_simple_self_signed(san_strings)?;
    Ok(CertPem {
        cert: signed.cert.pem(),
        key: signed.key_pair.serialize_pem(),
    })
}
