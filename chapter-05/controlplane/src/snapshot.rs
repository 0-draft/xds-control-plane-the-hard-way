//! xdstp://-named resources across two authorities.
//!
//! Every resource is named with an `xdstp://` URL instead of a bare string, and
//! the names live under two authorities: `hardway` owns the Listener and Route,
//! `edge` owns the Cluster and Endpoint. A single control plane serves both
//! authorities over one Delta ADS stream, because Envoy does not yet support
//! mapping an authority to a separate ConfigSource (see the README).
//!
//! The cross-references use xdstp names too: the Listener's RDS points at the
//! route's xdstp URL, the route targets the cluster's xdstp URL, and the
//! cluster's EDS service name is the endpoint's xdstp URL. So Envoy walks the
//! graph entirely through xdstp names.

use std::collections::HashMap;

use prost::Message;

use xds_api::pb::google::protobuf::{Any, Duration as PbDuration};

use xds_api::pb::envoy::config::cluster::v3::cluster::{
    ClusterDiscoveryType, DiscoveryType, EdsClusterConfig,
};
use xds_api::pb::envoy::config::cluster::v3::Cluster;
use xds_api::pb::envoy::config::core::v3::address::Address as AddressKind;
use xds_api::pb::envoy::config::core::v3::config_source::ConfigSourceSpecifier;
use xds_api::pb::envoy::config::core::v3::socket_address::PortSpecifier;
use xds_api::pb::envoy::config::core::v3::{
    AggregatedConfigSource, ApiVersion, ConfigSource, SocketAddress,
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

pub const T_LISTENER: &str = "type.googleapis.com/envoy.config.listener.v3.Listener";
pub const T_ROUTE: &str = "type.googleapis.com/envoy.config.route.v3.RouteConfiguration";
pub const T_CLUSTER: &str = "type.googleapis.com/envoy.config.cluster.v3.Cluster";
pub const T_ENDPOINT: &str = "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment";
const T_HCM: &str =
    "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager";
const T_ROUTER: &str = "type.googleapis.com/envoy.extensions.filters.http.router.v3.Router";

// Two authorities, one control plane.
const AUTH_HW: &str = "hardway";
const AUTH_EDGE: &str = "edge";

const LISTENER_PORT: u32 = 10000;
const UPSTREAM_HOST: &str = "upstream";
const UPSTREAM_PORT: u32 = 9000;

/// Build the xdstp:// URL for one resource: `xdstp://{authority}/{proto}/{id}`,
/// where `proto` is the fully qualified proto type without the
/// `type.googleapis.com/` prefix.
fn xdstp(authority: &str, type_url: &str, id: &str) -> String {
    let proto = type_url.trim_start_matches("type.googleapis.com/");
    format!("xdstp://{authority}/{proto}/{id}")
}

pub fn lds_name() -> String {
    xdstp(AUTH_HW, T_LISTENER, "primary")
}
pub fn rds_name() -> String {
    xdstp(AUTH_HW, T_ROUTE, "primary")
}
pub fn cds_name() -> String {
    xdstp(AUTH_EDGE, T_CLUSTER, "upstream")
}
pub fn eds_name() -> String {
    xdstp(AUTH_EDGE, T_ENDPOINT, "upstream")
}

#[derive(Clone)]
pub struct ResourceEntry {
    pub version: String,
    pub body: Any,
}

#[derive(Clone)]
pub struct Snapshot {
    pub system_version: String,
    by_type: HashMap<String, HashMap<String, ResourceEntry>>,
}

impl Snapshot {
    pub fn build(upstream_ip: &str) -> anyhow::Result<Self> {
        let mut by_type: HashMap<String, HashMap<String, ResourceEntry>> = HashMap::new();
        put(&mut by_type, T_LISTENER, &lds_name(), "v1", &listener()?)?;
        put(&mut by_type, T_ROUTE, &rds_name(), "v1", &route_config())?;
        put(&mut by_type, T_CLUSTER, &cds_name(), "v1", &cluster())?;
        put(
            &mut by_type,
            T_ENDPOINT,
            &eds_name(),
            "v1",
            &endpoints(upstream_ip),
        )?;
        Ok(Self {
            system_version: "v1".into(),
            by_type,
        })
    }

    pub fn names(&self, type_url: &str) -> Vec<String> {
        self.by_type
            .get(type_url)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Names of this type whose xdstp URL starts with `prefix` (glob expansion).
    pub fn names_with_prefix(&self, type_url: &str, prefix: &str) -> Vec<String> {
        self.by_type
            .get(type_url)
            .map(|m| {
                m.keys()
                    .filter(|n| n.starts_with(prefix))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn get(&self, type_url: &str, name: &str) -> Option<&ResourceEntry> {
        self.by_type.get(type_url)?.get(name)
    }
}

fn put(
    by_type: &mut HashMap<String, HashMap<String, ResourceEntry>>,
    type_url: &str,
    name: &str,
    version: &str,
    msg: &impl Message,
) -> anyhow::Result<()> {
    let body = Any {
        type_url: type_url.into(),
        value: msg.encode_to_vec(),
    };
    by_type.entry(type_url.into()).or_default().insert(
        name.into(),
        ResourceEntry {
            version: version.into(),
            body,
        },
    );
    Ok(())
}

fn any<M: Message>(type_url: &str, msg: &M) -> Any {
    Any {
        type_url: type_url.into(),
        value: msg.encode_to_vec(),
    }
}

fn ads_config_source() -> ConfigSource {
    ConfigSource {
        resource_api_version: ApiVersion::V3 as i32,
        config_source_specifier: Some(ConfigSourceSpecifier::Ads(AggregatedConfigSource {})),
        ..Default::default()
    }
}

fn listener() -> anyhow::Result<Listener> {
    let router_any = any(T_ROUTER, &Router::default());

    let hcm = HttpConnectionManager {
        stat_prefix: "ingress_http".into(),
        route_specifier: Some(RouteSpecifier::Rds(Rds {
            config_source: Some(ads_config_source()),
            // RDS by xdstp singleton name.
            route_config_name: rds_name(),
        })),
        http_filters: vec![HttpFilter {
            name: "envoy.filters.http.router".into(),
            config_type: Some(HttpFilterConfigType::TypedConfig(router_any)),
            ..Default::default()
        }],
        ..Default::default()
    };
    let hcm_any = any(T_HCM, &hcm);

    Ok(Listener {
        name: lds_name(),
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
            ..Default::default()
        }],
        ..Default::default()
    })
}

fn route_config() -> RouteConfiguration {
    RouteConfiguration {
        name: rds_name(),
        virtual_hosts: vec![VirtualHost {
            name: "all".into(),
            domains: vec!["*".into()],
            routes: vec![Route {
                r#match: Some(RouteMatch {
                    path_specifier: Some(PathSpecifier::Prefix("/".into())),
                    ..Default::default()
                }),
                action: Some(RouteAction::Route(RouteActionMsg {
                    // Target the cluster by its xdstp URL (different authority).
                    cluster_specifier: Some(ClusterSpecifier::Cluster(cds_name())),
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
        name: cds_name(),
        connect_timeout: Some(PbDuration {
            seconds: 5,
            nanos: 0,
        }),
        cluster_discovery_type: Some(ClusterDiscoveryType::Type(DiscoveryType::Eds as i32)),
        eds_cluster_config: Some(EdsClusterConfig {
            eds_config: Some(ads_config_source()),
            // EDS by xdstp singleton name.
            service_name: eds_name(),
        }),
        ..Default::default()
    }
}

fn endpoints(ip: &str) -> ClusterLoadAssignment {
    ClusterLoadAssignment {
        cluster_name: eds_name(),
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
