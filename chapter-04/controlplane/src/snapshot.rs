//! Per-resource versioned snapshots for Delta xDS.
//!
//! State-of-the-World keyed everything by one snapshot version. Delta is
//! incremental: every *resource* carries its own version, and the server sends
//! only the resources whose version changed. So the snapshot here is a map of
//! `type_url -> (resource_name -> entry)`, where each entry knows its version.
//!
//! The demo mutation is `bump`: it bumps only the RouteConfiguration (adding an
//! `x-config-version` response header), leaving the Listener, Cluster, and
//! Endpoint versions untouched. A Delta push then carries exactly one resource.

use std::collections::HashMap;

use prost::Message;

use xds_api::pb::google::protobuf::{Any, Duration as PbDuration};

use xds_api::pb::envoy::config::cluster::v3::cluster::{ClusterDiscoveryType, DiscoveryType};
use xds_api::pb::envoy::config::cluster::v3::Cluster;
use xds_api::pb::envoy::config::core::v3::address::Address as AddressKind;
use xds_api::pb::envoy::config::core::v3::config_source::ConfigSourceSpecifier;
use xds_api::pb::envoy::config::core::v3::socket_address::PortSpecifier;
use xds_api::pb::envoy::config::core::v3::{
    AggregatedConfigSource, ApiVersion, ConfigSource, HeaderValue, HeaderValueOption, SocketAddress,
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

const LISTENER_NAME: &str = "primary_listener";
const ROUTE_NAME: &str = "primary_route";
const CLUSTER_NAME: &str = "upstream_cluster";
const LISTENER_PORT: u32 = 10000;

const UPSTREAM_HOST: &str = "upstream";
const UPSTREAM_PORT: u32 = 9000;

/// One resource and the version at which it currently sits.
#[derive(Clone)]
pub struct ResourceEntry {
    pub version: String,
    pub body: Any,
}

#[derive(Clone)]
pub struct Snapshot {
    pub system_version: String,
    /// type_url -> (resource_name -> entry)
    by_type: HashMap<String, HashMap<String, ResourceEntry>>,
}

impl Snapshot {
    /// Build a snapshot. The Listener / Cluster / Endpoint are always at `v1`;
    /// the RouteConfiguration sits at `v{route_version}` and tags responses
    /// with an `x-config-version` header so the bump is visible to `curl`.
    pub fn build(route_version: u64, upstream_ip: &str) -> anyhow::Result<Self> {
        let mut by_type: HashMap<String, HashMap<String, ResourceEntry>> = HashMap::new();
        put(&mut by_type, T_LISTENER, LISTENER_NAME, "v1", &listener()?)?;
        put(&mut by_type, T_CLUSTER, CLUSTER_NAME, "v1", &cluster())?;
        put(
            &mut by_type,
            T_ENDPOINT,
            CLUSTER_NAME,
            "v1",
            &endpoints(upstream_ip),
        )?;
        let rv = format!("v{route_version}");
        put(
            &mut by_type,
            T_ROUTE,
            ROUTE_NAME,
            &rv,
            &route_config(route_version),
        )?;
        Ok(Self {
            system_version: format!("sys-{route_version}"),
            by_type,
        })
    }

    /// All resource names currently held for a type.
    pub fn names(&self, type_url: &str) -> Vec<String> {
        self.by_type
            .get(type_url)
            .map(|m| m.keys().cloned().collect())
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
            route_config_name: ROUTE_NAME.into(),
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
            ..Default::default()
        }],
        ..Default::default()
    })
}

fn route_config(route_version: u64) -> RouteConfiguration {
    RouteConfiguration {
        name: ROUTE_NAME.into(),
        virtual_hosts: vec![VirtualHost {
            name: "all".into(),
            domains: vec!["*".into()],
            // Make the route version observable from outside: every response
            // carries the header, so a bump is visible to `curl -i`.
            response_headers_to_add: vec![HeaderValueOption {
                header: Some(HeaderValue {
                    key: "x-config-version".into(),
                    value: format!("v{route_version}"),
                    ..Default::default()
                }),
                ..Default::default()
            }],
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
