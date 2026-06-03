//! Snapshot for Chapter 6: two endpoints whose EDS weights are driven by ORCA.
//!
//! The cluster is a plain EDS cluster with the default (weighted) round robin.
//! The novelty is upstream of the snapshot: the control plane streams ORCA load
//! reports from each backend and converts utilization into the
//! `load_balancing_weight` carried here, then re-pushes EDS. A lighter backend
//! gets a larger weight, so Envoy sends it more traffic.

use std::collections::HashMap;

use prost::Message;

use xds_api::pb::google::protobuf::{Any, Duration as PbDuration, UInt32Value};

use xds_api::pb::envoy::config::cluster::v3::cluster::{ClusterDiscoveryType, DiscoveryType};
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
use xds_api::pb::envoy::service::discovery::v3::DiscoveryResponse;

const T_LISTENER: &str = "type.googleapis.com/envoy.config.listener.v3.Listener";
const T_ROUTE: &str = "type.googleapis.com/envoy.config.route.v3.RouteConfiguration";
const T_CLUSTER: &str = "type.googleapis.com/envoy.config.cluster.v3.Cluster";
const T_ENDPOINT: &str = "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment";
const T_HCM: &str =
    "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager";
const T_ROUTER: &str = "type.googleapis.com/envoy.extensions.filters.http.router.v3.Router";

const LISTENER_NAME: &str = "primary_listener";
const ROUTE_NAME: &str = "primary_route";
const CLUSTER_NAME: &str = "upstream_cluster";
const LISTENER_PORT: u32 = 10000;
const UPSTREAM_PORT: u32 = 9000;

/// One backend endpoint: its literal IP and the EDS weight derived from ORCA.
#[derive(Clone)]
pub struct Weighted {
    pub ip: String,
    pub weight: u32,
}

#[derive(Clone)]
pub struct Snapshot {
    pub version: String,
    by_type: HashMap<String, Vec<Any>>,
}

impl Snapshot {
    pub fn build(version: &str, endpoints_in: &[Weighted]) -> anyhow::Result<Self> {
        let mut by_type = HashMap::new();
        by_type.insert(T_LISTENER.into(), vec![any(T_LISTENER, &listener()?)?]);
        by_type.insert(T_ROUTE.into(), vec![any(T_ROUTE, &route_config())?]);
        by_type.insert(T_CLUSTER.into(), vec![any(T_CLUSTER, &cluster())?]);
        by_type.insert(
            T_ENDPOINT.into(),
            vec![any(T_ENDPOINT, &endpoints(endpoints_in))?],
        );
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

fn endpoints(weighted: &[Weighted]) -> ClusterLoadAssignment {
    let lb_endpoints = weighted
        .iter()
        .map(|w| LbEndpoint {
            host_identifier: Some(HostIdentifier::Endpoint(Endpoint {
                address: Some(xds_api::pb::envoy::config::core::v3::Address {
                    address: Some(AddressKind::SocketAddress(SocketAddress {
                        address: w.ip.clone(),
                        port_specifier: Some(PortSpecifier::PortValue(UPSTREAM_PORT)),
                        ..Default::default()
                    })),
                }),
                ..Default::default()
            })),
            load_balancing_weight: Some(UInt32Value { value: w.weight }),
            ..Default::default()
        })
        .collect();

    ClusterLoadAssignment {
        cluster_name: CLUSTER_NAME.into(),
        endpoints: vec![LocalityLbEndpoints {
            lb_endpoints,
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Resolve a hostname to a literal IPv4, retrying briefly.
pub async fn resolve_one(host: &str) -> anyhow::Result<String> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=10 {
        match tokio::net::lookup_host((host, UPSTREAM_PORT as u16)).await {
            Ok(addrs) => {
                let all: Vec<_> = addrs.collect();
                if let Some(addr) = all.iter().find(|a| a.ip().is_ipv4()).or_else(|| all.first()) {
                    return Ok(addr.ip().to_string());
                }
                last_err = Some(anyhow::anyhow!("DNS returned no records for {host}"));
            }
            Err(e) => last_err = Some(anyhow::Error::from(e)),
        }
        tracing::warn!(host, attempt, "DNS resolution not ready, retrying");
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("dns retries exhausted for {host}")))
}
