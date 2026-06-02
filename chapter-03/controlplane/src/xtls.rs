//! Hand-rolled prost mirrors for the two TLS-context messages that xds-api 0.2
//! does not generate: `DownstreamTlsContext` and `CommonTlsContext`.
//!
//! This is the Hard Way showing through. xds-api gives us the SDS `Secret`
//! types but not the listener-side wrappers that point at them, so we define
//! the minimal subset ourselves. The field numbers match envoy's real protos
//! (`common_tls_context = 1`, `tls_certificate_sds_secret_configs = 6`), so the
//! bytes we encode are wire-compatible: Envoy decodes them under the real
//! `DownstreamTlsContext` type URL exactly as if they came from the full proto.
//!
//! We reuse the genuine `SdsSecretConfig` from xds-api as the repeated element,
//! which keeps the nested wire format honest.

use xds_api::pb::envoy::extensions::transport_sockets::tls::v3::SdsSecretConfig;

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DownstreamTlsContext {
    #[prost(message, optional, tag = "1")]
    pub common_tls_context: Option<CommonTlsContext>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct CommonTlsContext {
    #[prost(message, repeated, tag = "6")]
    pub tls_certificate_sds_secret_configs: Vec<SdsSecretConfig>,
}
