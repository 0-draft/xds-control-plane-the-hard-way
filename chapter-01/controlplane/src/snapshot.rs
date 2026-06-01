//! The hard-coded "v1" snapshot for Chapter 1.
//!
//! All four resources are built from scratch using the proto types
//! exposed by `xds-api`. We then wrap each into a `google.protobuf.Any`
//! so the ADS server can stream them back as `DiscoveryResponse.resources`.

use std::collections::HashMap;

use prost::Message;

// `xds-api` regenerates its own `google.protobuf.{Any, Duration}` instead of
// reusing `prost-types`, so all envoy proto fields expect *these* types.
// Importing `prost_types::Any` here would compile but every field assignment
// would error with mismatched types.
use xds_api::pb::google::protobuf::{Any, Duration as PbDuration};

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

// type URLs. Defining them as constants keeps `match` arms readable
// and makes it explicit which envoy protos we're committing to.
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

// Resolved at startup via Docker's embedded DNS to the `upstream` container.
// EDS endpoints must carry a literal IP. If you put a hostname in an EDS
// SocketAddress, Envoy NACKs with `malformed IP address`. The cluster type
// would need to be STRICT_DNS/LOGICAL_DNS for hostnames to be allowed, and
// in that case the endpoints come from the Cluster itself (no EDS at all).
const UPSTREAM_HOST: &str = "upstream";
const UPSTREAM_PORT: u32 = 9000;

#[derive(Clone)]
pub struct Snapshot {
    pub version: String,
    by_type: HashMap<String, Vec<Any>>,
}

impl Snapshot {
    pub async fn v1() -> anyhow::Result<Self> {
        let upstream_ip = resolve_with_retry(UPSTREAM_HOST, UPSTREAM_PORT).await?;
        tracing::info!(host = UPSTREAM_HOST, ip = %upstream_ip, "resolved upstream");

        let mut by_type = HashMap::new();
        by_type.insert(T_LISTENER.into(), vec![any(T_LISTENER, &listener()?)?]);
        by_type.insert(T_ROUTE.into(), vec![any(T_ROUTE, &route_config())?]);
        by_type.insert(T_CLUSTER.into(), vec![any(T_CLUSTER, &cluster())?]);
        by_type.insert(
            T_ENDPOINT.into(),
            vec![any(T_ENDPOINT, &endpoints(&upstream_ip))?],
        );
        Ok(Self {
            version: "v1".into(),
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
    // 1. The HTTP router filter goes at the end of the HCM filter chain.
    let router_any = any(T_ROUTER, &Router::default())?;

    // 2. HCM points its routes at the RDS resource named "primary_route".
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

    // 3. The Listener wraps a single FilterChain containing the HCM as a network filter.
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

async fn resolve_with_retry(host: &str, port: u32) -> anyhow::Result<String> {
    // The upstream container can take a beat to come up under
    // docker-compose, so retry briefly instead of crash-looping.
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=10 {
        match tokio::net::lookup_host((host, port as u16)).await {
            Ok(addrs) => {
                let v4_first: Vec<_> = addrs.collect();
                if let Some(addr) = v4_first
                    .iter()
                    .find(|a| a.ip().is_ipv4())
                    .or_else(|| v4_first.first())
                {
                    return Ok(addr.ip().to_string());
                }
                last_err = Some(anyhow::anyhow!("DNS returned no records for {host}"));
            }
            Err(e) => {
                last_err = Some(anyhow::Error::from(e));
            }
        }
        tracing::warn!(host, attempt, "DNS resolution not ready, retrying");
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("dns retries exhausted")))
}
