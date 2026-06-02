//! Snapshots for Chapter 3: the Listener now terminates TLS, and the server
//! certificate is delivered as a separate SDS `Secret` resource over the same
//! ADS stream.
//!
//! The Listener never carries cert bytes. Its `DownstreamTlsContext` references
//! the secret by name (`server_cert`) through an SDS config source, so Envoy
//! subscribes to that name and we answer with the current certificate. Rotating
//! the cert is just pushing a new snapshot whose `Secret` resource differs.

use std::collections::HashMap;

use prost::Message;

use xds_api::pb::google::protobuf::{Any, Duration as PbDuration};

use xds_api::pb::envoy::config::cluster::v3::cluster::{ClusterDiscoveryType, DiscoveryType};
use xds_api::pb::envoy::config::cluster::v3::Cluster;
use xds_api::pb::envoy::config::core::v3::address::Address as AddressKind;
use xds_api::pb::envoy::config::core::v3::config_source::ConfigSourceSpecifier;
use xds_api::pb::envoy::config::core::v3::data_source::Specifier as DataSpecifier;
use xds_api::pb::envoy::config::core::v3::socket_address::PortSpecifier;
use xds_api::pb::envoy::config::core::v3::transport_socket::ConfigType as TsConfigType;
use xds_api::pb::envoy::config::core::v3::{
    AggregatedConfigSource, ApiVersion, ConfigSource, DataSource, SocketAddress, TransportSocket,
};
use xds_api::pb::envoy::config::endpoint::v3::lb_endpoint::HostIdentifier;
use xds_api::pb::envoy::config::endpoint::v3::{
    ClusterLoadAssignment, Endpoint, LbEndpoint, LocalityLbEndpoints,
};
use xds_api::pb::envoy::config::listener::v3::filter::ConfigType as FilterConfigType;
use xds_api::pb::envoy::config::listener::v3::{Filter, FilterChain, Listener};
use xds_api::pb::envoy::config::route::v3::route::Action as RouteAction;
use xds_api::pb::envoy::config::route::v3::route_action::ClusterSpecifier;
use xds_api::pb::envoy::config::route::v3::route_match::PathSpecifier;
use xds_api::pb::envoy::config::route::v3::{
    Route, RouteAction as RouteActionMsg, RouteConfiguration, RouteMatch, VirtualHost,
};
use xds_api::pb::envoy::extensions::filters::http::router::v3::Router;
use xds_api::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_connection_manager::RouteSpecifier;
use xds_api::pb::envoy::extensions::filters::network::http_connection_manager::v3::http_filter::ConfigType as HttpFilterConfigType;
use xds_api::pb::envoy::extensions::filters::network::http_connection_manager::v3::{
    HttpConnectionManager, HttpFilter, Rds,
};
use xds_api::pb::envoy::extensions::transport_sockets::tls::v3::secret::Type as SecretType;
use xds_api::pb::envoy::extensions::transport_sockets::tls::v3::{
    Secret, SdsSecretConfig, TlsCertificate,
};
use xds_api::pb::envoy::service::discovery::v3::DiscoveryResponse;

use crate::tls::CertPem;
// DownstreamTlsContext / CommonTlsContext are not in xds-api 0.2; see xtls.rs.
use crate::xtls::{CommonTlsContext, DownstreamTlsContext};

const T_LISTENER: &str = "type.googleapis.com/envoy.config.listener.v3.Listener";
const T_ROUTE: &str = "type.googleapis.com/envoy.config.route.v3.RouteConfiguration";
const T_CLUSTER: &str = "type.googleapis.com/envoy.config.cluster.v3.Cluster";
const T_ENDPOINT: &str = "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment";
const T_SECRET: &str = "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.Secret";
const T_HCM: &str =
    "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager";
const T_ROUTER: &str = "type.googleapis.com/envoy.extensions.filters.http.router.v3.Router";
const T_DOWNSTREAM_TLS: &str =
    "type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.DownstreamTlsContext";

const LISTENER_NAME: &str = "primary_listener";
const ROUTE_NAME: &str = "primary_route";
const CLUSTER_NAME: &str = "upstream_cluster";
const SECRET_NAME: &str = "server_cert";
const LISTENER_PORT: u32 = 10000;

const UPSTREAM_HOST: &str = "upstream";
const UPSTREAM_PORT: u32 = 9000;

#[derive(Clone)]
pub struct Snapshot {
    pub version: String,
    by_type: HashMap<String, Vec<Any>>,
}

impl Snapshot {
    /// Build a snapshot whose Listener terminates TLS using `cert`, delivered
    /// over SDS.
    pub fn build(version: &str, upstream_ip: &str, cert: &CertPem) -> anyhow::Result<Self> {
        let mut by_type = HashMap::new();
        by_type.insert(T_LISTENER.into(), vec![any(T_LISTENER, &listener()?)?]);
        by_type.insert(T_ROUTE.into(), vec![any(T_ROUTE, &route_config())?]);
        by_type.insert(T_CLUSTER.into(), vec![any(T_CLUSTER, &cluster())?]);
        by_type.insert(
            T_ENDPOINT.into(),
            vec![any(T_ENDPOINT, &endpoints(upstream_ip))?],
        );
        by_type.insert(T_SECRET.into(), vec![any(T_SECRET, &secret(cert))?]);
        Ok(Self {
            version: version.into(),
            by_type,
        })
    }

    pub fn build_response(&self, type_url: &str) -> Option<DiscoveryResponse> {
        let resources = self.by_type.get(type_url)?.clone();
        Some(DiscoveryResponse {
            version_info: self.version.clone(),
            resources,
            canary: false,
            type_url: type_url.into(),
            nonce: format!("{}-{}", short(type_url), self.version),
            control_plane: None,
        })
    }
}

fn short(type_url: &str) -> &'static str {
    match type_url {
        T_LISTENER => "lds",
        T_ROUTE => "rds",
        T_CLUSTER => "cds",
        T_ENDPOINT => "eds",
        T_SECRET => "sds",
        _ => "xds",
    }
}

fn any<M: Message>(type_url: &str, msg: &M) -> anyhow::Result<Any> {
    Ok(Any {
        type_url: type_url.into(),
        value: msg.encode_to_vec(),
    })
}

fn ads_config_source() -> ConfigSource {
    ConfigSource {
        resource_api_version: ApiVersion::V3 as i32,
        config_source_specifier: Some(ConfigSourceSpecifier::Ads(AggregatedConfigSource {})),
        ..Default::default()
    }
}

fn listener() -> anyhow::Result<Listener> {
    let router_any = any(T_ROUTER, &Router::default())?;

    let hcm = HttpConnectionManager {
        stat_prefix: "ingress_http".into(),
        route_specifier: Some(RouteSpecifier::Rds(Rds {
            config_source: Some(ads_config_source()),
            route_config_name: ROUTE_NAME.into(),
        })),
        http_filters: vec![HttpFilter {
            name: "envoy.filters.http.router".into(),
            config_type: Some(HttpFilterConfigType::TypedConfig(router_any)),
            ..Default::default()
        }],
        ..Default::default()
    };
    let hcm_any = any(T_HCM, &hcm)?;

    // The cert never lives in the Listener. The filter chain's transport socket
    // points at the SDS secret named `server_cert`, fetched over ADS.
    let tls_any = any(T_DOWNSTREAM_TLS, &downstream_tls_context())?;

    Ok(Listener {
        name: LISTENER_NAME.into(),
        address: Some(xds_api::pb::envoy::config::core::v3::Address {
            address: Some(AddressKind::SocketAddress(SocketAddress {
                address: "0.0.0.0".into(),
                port_specifier: Some(PortSpecifier::PortValue(LISTENER_PORT)),
                ..Default::default()
            })),
        }),
        filter_chains: vec![FilterChain {
            filters: vec![Filter {
                name: "envoy.filters.network.http_connection_manager".into(),
                config_type: Some(FilterConfigType::TypedConfig(hcm_any)),
            }],
            transport_socket: Some(TransportSocket {
                name: "envoy.transport_sockets.tls".into(),
                config_type: Some(TsConfigType::TypedConfig(tls_any)),
            }),
            ..Default::default()
        }],
        ..Default::default()
    })
}

fn downstream_tls_context() -> DownstreamTlsContext {
    DownstreamTlsContext {
        common_tls_context: Some(CommonTlsContext {
            tls_certificate_sds_secret_configs: vec![SdsSecretConfig {
                name: SECRET_NAME.into(),
                sds_config: Some(ads_config_source()),
            }],
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn secret(cert: &CertPem) -> Secret {
    Secret {
        name: SECRET_NAME.into(),
        r#type: Some(SecretType::TlsCertificate(TlsCertificate {
            certificate_chain: Some(DataSource {
                specifier: Some(DataSpecifier::InlineString(cert.cert.clone())),
                ..Default::default()
            }),
            private_key: Some(DataSource {
                specifier: Some(DataSpecifier::InlineString(cert.key.clone())),
                ..Default::default()
            }),
            ..Default::default()
        })),
    }
}

fn route_config() -> RouteConfiguration {
    RouteConfiguration {
        name: ROUTE_NAME.into(),
        virtual_hosts: vec![VirtualHost {
            name: "all".into(),
            domains: vec!["*".into()],
            routes: vec![Route {
                r#match: Some(RouteMatch {
                    path_specifier: Some(PathSpecifier::Prefix("/".into())),
                    ..Default::default()
                }),
                action: Some(RouteAction::Route(RouteActionMsg {
                    cluster_specifier: Some(ClusterSpecifier::Cluster(CLUSTER_NAME.into())),
                    ..Default::default()
                })),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn cluster() -> Cluster {
    Cluster {
        name: CLUSTER_NAME.into(),
        connect_timeout: Some(PbDuration {
            seconds: 5,
            nanos: 0,
        }),
        cluster_discovery_type: Some(ClusterDiscoveryType::Type(DiscoveryType::Eds as i32)),
        eds_cluster_config: Some(
            xds_api::pb::envoy::config::cluster::v3::cluster::EdsClusterConfig {
                eds_config: Some(ads_config_source()),
                ..Default::default()
            },
        ),
        ..Default::default()
    }
}

fn endpoints(ip: &str) -> ClusterLoadAssignment {
    ClusterLoadAssignment {
        cluster_name: CLUSTER_NAME.into(),
        endpoints: vec![LocalityLbEndpoints {
            lb_endpoints: vec![LbEndpoint {
                host_identifier: Some(HostIdentifier::Endpoint(Endpoint {
                    address: Some(xds_api::pb::envoy::config::core::v3::Address {
                        address: Some(AddressKind::SocketAddress(SocketAddress {
                            address: ip.into(),
                            port_specifier: Some(PortSpecifier::PortValue(UPSTREAM_PORT)),
                            ..Default::default()
                        })),
                    }),
                    ..Default::default()
                })),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Resolve the upstream container to a literal IPv4 once, at startup.
pub async fn resolve_upstream() -> anyhow::Result<String> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=10 {
        match tokio::net::lookup_host((UPSTREAM_HOST, UPSTREAM_PORT as u16)).await {
            Ok(addrs) => {
                let all: Vec<_> = addrs.collect();
                if let Some(addr) = all.iter().find(|a| a.ip().is_ipv4()).or_else(|| all.first()) {
                    return Ok(addr.ip().to_string());
                }
                last_err = Some(anyhow::anyhow!("DNS returned no records for {UPSTREAM_HOST}"));
            }
            Err(e) => last_err = Some(anyhow::Error::from(e)),
        }
        tracing::warn!(
            host = UPSTREAM_HOST,
            attempt,
            "DNS resolution not ready, retrying"
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("dns retries exhausted")))
}
