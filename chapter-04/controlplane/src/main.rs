//! Delta xDS.
//!
//! The same stack as the earlier chapters, but the ADS stream is incremental.
//! Where State-of-the-World resent every resource of a type on any change,
//! Delta sends only the resources whose version moved, and names removed
//! resources explicitly. We implement `delta_aggregated_resources` here; the
//! SotW entry point is left unimplemented to keep the chapter about Delta.
//!
//! The demo: `POST /bump` changes only the RouteConfiguration. Watch the push
//! that follows carry exactly one resource (RDS), while CDS / LDS / EDS stay
//! silent because their versions did not change.

mod snapshot;

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request as HttpRequest, Response as HttpResponse, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{transport::Server, Request, Response, Status, Streaming};
use tracing::{info, warn};

use xds_api::pb::envoy::service::discovery::v3::aggregated_discovery_service_server::{
    AggregatedDiscoveryService, AggregatedDiscoveryServiceServer,
};
use xds_api::pb::envoy::service::discovery::v3::{
    DeltaDiscoveryRequest, DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse, Resource,
};

use crate::snapshot::Snapshot;

const XDS_LISTEN: &str = "0.0.0.0:18000";
const ADMIN_LISTEN: &str = "0.0.0.0:19000";

#[derive(Clone)]
struct Control {
    advertised: watch::Sender<Arc<Snapshot>>,
    upstream_ip: String,
    route_version: Arc<AtomicU64>,
}

impl Control {
    fn bump(&self) -> anyhow::Result<u64> {
        let rv = self.route_version.fetch_add(1, Ordering::SeqCst) + 1;
        let snap = Arc::new(Snapshot::build(rv, &self.upstream_ip)?);
        let _ = self.advertised.send(snap);
        Ok(rv)
    }

    fn status(&self) -> String {
        let snap = self.advertised.borrow();
        format!(
            "route_version=v{} system_version={}\n",
            self.route_version.load(Ordering::SeqCst),
            snap.system_version
        )
    }
}

/// What a single stream is subscribed to for one resource type.
#[derive(Default)]
struct TypeSub {
    wildcard: bool,
    names: HashSet<String>,
}

#[derive(Clone)]
struct AdsServer {
    control: Control,
}

#[tonic::async_trait]
impl AggregatedDiscoveryService for AdsServer {
    type StreamAggregatedResourcesStream =
        Pin<Box<dyn Stream<Item = Result<DiscoveryResponse, Status>> + Send + 'static>>;

    async fn stream_aggregated_resources(
        &self,
        _request: Request<Streaming<DiscoveryRequest>>,
    ) -> Result<Response<Self::StreamAggregatedResourcesStream>, Status> {
        Err(Status::unimplemented(
            "Chapter 4 is Delta only; SotW lives in Chapters 1-3",
        ))
    }

    type DeltaAggregatedResourcesStream =
        Pin<Box<dyn Stream<Item = Result<DeltaDiscoveryResponse, Status>> + Send + 'static>>;

    async fn delta_aggregated_resources(
        &self,
        request: Request<Streaming<DeltaDiscoveryRequest>>,
    ) -> Result<Response<Self::DeltaAggregatedResourcesStream>, Status> {
        let peer = request
            .remote_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|| "?".into());
        info!(%peer, "delta ADS stream opened");

        let mut inbound = request.into_inner();
        let control = self.control.clone();
        let (tx, rx_out) = mpsc::channel::<Result<DeltaDiscoveryResponse, Status>>(16);

        tokio::spawn(async move {
            let mut advertised_rx = control.advertised.subscribe();
            let mut subs: HashMap<String, TypeSub> = HashMap::new();
            // type_url -> (resource_name -> version) we have delivered.
            let mut sent: HashMap<String, HashMap<String, String>> = HashMap::new();
            let mut nonce_ctr: u64 = 0;

            'outer: loop {
                tokio::select! {
                    biased;

                    maybe = inbound.next() => {
                        let Some(msg) = maybe else { break 'outer; };
                        let req = match msg {
                            Ok(r) => r,
                            Err(e) => { warn!(error = %e, "stream recv error"); break 'outer; }
                        };

                        let type_url = req.type_url.clone();
                        let node_id = req.node.as_ref().map(|n| n.id.clone()).unwrap_or_default();
                        let st = short_type(&type_url);

                        if let Some(err) = &req.error_detail {
                            warn!(
                                node = %node_id, kind = "NACK", ty = st,
                                nonce = %req.response_nonce, msg = %err.message,
                                "client rejected config"
                            );
                            continue;
                        }

                        // Record what the client says it already has.
                        let sent_map = sent.entry(type_url.clone()).or_default();
                        for (name, ver) in &req.initial_resource_versions {
                            sent_map.insert(name.clone(), ver.clone());
                        }

                        // Update the subscription for this type.
                        let known = subs.contains_key(&type_url);
                        let sub = subs.entry(type_url.clone()).or_default();
                        for n in &req.resource_names_subscribe {
                            if n == "*" { sub.wildcard = true; } else { sub.names.insert(n.clone()); }
                        }
                        for n in &req.resource_names_unsubscribe {
                            if n == "*" { sub.wildcard = false; } else { sub.names.remove(n); }
                        }
                        // Legacy wildcard: a first request for a type with no
                        // explicit names is a wildcard subscription.
                        if !known
                            && req.resource_names_subscribe.is_empty()
                            && req.resource_names_unsubscribe.is_empty()
                        {
                            sub.wildcard = true;
                        }

                        let changing = !req.resource_names_subscribe.is_empty()
                            || !req.resource_names_unsubscribe.is_empty()
                            || req.response_nonce.is_empty();
                        if changing {
                            info!(
                                node = %node_id, kind = "SUB ", ty = st,
                                subscribe = ?req.resource_names_subscribe,
                                wildcard = sub.wildcard, "client subscribed"
                            );
                        } else {
                            info!(
                                node = %node_id, kind = "ACK ", ty = st,
                                nonce = %req.response_nonce, "client accepted config"
                            );
                        }

                        let snap = control.advertised.borrow().clone();
                        if send_delta(&type_url, &snap, &mut subs, &mut sent, &tx, &mut nonce_ctr)
                            .await
                            .is_err()
                        {
                            break 'outer;
                        }
                    }

                    changed = advertised_rx.changed() => {
                        if changed.is_err() { break 'outer; }
                        let snap = advertised_rx.borrow_and_update().clone();
                        for type_url in subs.keys().cloned().collect::<Vec<_>>() {
                            if send_delta(&type_url, &snap, &mut subs, &mut sent, &tx, &mut nonce_ctr)
                                .await
                                .is_err()
                            {
                                break 'outer;
                            }
                        }
                    }
                }
            }

            info!(%peer, "delta ADS stream closed");
        });

        let stream = ReceiverStream::new(rx_out);
        Ok(Response::new(Box::pin(stream)))
    }
}

/// Compute and send the incremental update for one resource type: the resources
/// whose version changed since we last sent them, plus any removals. Sends
/// nothing (a pure ACK on the client's side) when nothing moved.
async fn send_delta(
    type_url: &str,
    snap: &Snapshot,
    subs: &mut HashMap<String, TypeSub>,
    sent: &mut HashMap<String, HashMap<String, String>>,
    tx: &mpsc::Sender<Result<DeltaDiscoveryResponse, Status>>,
    nonce_ctr: &mut u64,
) -> Result<(), ()> {
    let sub = subs.entry(type_url.to_string()).or_default();
    let wanted: Vec<String> = if sub.wildcard {
        snap.names(type_url)
    } else {
        sub.names.iter().cloned().collect()
    };

    let sent_map = sent.entry(type_url.to_string()).or_default();

    let mut resources = Vec::new();
    for name in &wanted {
        if let Some(entry) = snap.get(type_url, name) {
            if sent_map.get(name) != Some(&entry.version) {
                resources.push(Resource {
                    name: name.clone(),
                    version: entry.version.clone(),
                    resource: Some(entry.body.clone()),
                    ..Default::default()
                });
            }
        }
    }

    let mut removed = Vec::new();
    for name in sent_map.keys().cloned().collect::<Vec<_>>() {
        let still_wanted = sub.wildcard || sub.names.contains(&name);
        if still_wanted && snap.get(type_url, &name).is_none() {
            removed.push(name);
        }
    }

    if resources.is_empty() && removed.is_empty() {
        return Ok(());
    }

    for r in &resources {
        sent_map.insert(r.name.clone(), r.version.clone());
    }
    for name in &removed {
        sent_map.remove(name);
    }

    *nonce_ctr += 1;
    let nonce = format!("{}-{}", short_type(type_url).to_lowercase(), nonce_ctr);
    info!(
        ty = short_type(type_url),
        resources = resources.len(),
        removed = removed.len(),
        "pushing delta"
    );

    let resp = DeltaDiscoveryResponse {
        system_version_info: snap.system_version.clone(),
        resources,
        type_url: type_url.to_string(),
        removed_resources: removed,
        nonce,
        control_plane: None,
        ..Default::default()
    };
    tx.send(Ok(resp)).await.map_err(|_| ())
}

fn short_type(type_url: &str) -> &'static str {
    if type_url.ends_with(".Listener") {
        "LDS"
    } else if type_url.ends_with(".RouteConfiguration") {
        "RDS"
    } else if type_url.ends_with(".Cluster") {
        "CDS"
    } else if type_url.ends_with(".ClusterLoadAssignment") {
        "EDS"
    } else {
        "???"
    }
}

async fn handle_admin(
    req: HttpRequest<hyper::body::Incoming>,
    control: Control,
) -> Result<HttpResponse<Full<Bytes>>, Infallible> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    let (status, body) = match (&method, path.as_str()) {
        (&Method::POST, "/bump") => match control.bump() {
            Ok(rv) => (StatusCode::OK, format!("bumped route to v{rv}\n")),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("error: {e}\n")),
        },
        (_, "/status") => (StatusCode::OK, control.status()),
        _ => (
            StatusCode::NOT_FOUND,
            "routes: POST /bump, GET /status\n".into(),
        ),
    };

    Ok(HttpResponse::builder()
        .status(status)
        .header("content-type", "text/plain")
        .body(Full::new(Bytes::from(body)))
        .unwrap())
}

async fn run_admin(control: Control) -> anyhow::Result<()> {
    let listener = TcpListener::bind(ADMIN_LISTEN).await?;
    info!(addr = ADMIN_LISTEN, "admin API listening");
    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let control = control.clone();
        tokio::spawn(async move {
            let svc = service_fn(move |req| handle_admin(req, control.clone()));
            if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
                warn!(error = ?e, "admin connection error");
            }
        });
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let upstream_ip = snapshot::resolve_upstream().await?;
    info!(ip = %upstream_ip, "resolved upstream");

    let v1 = Arc::new(Snapshot::build(1, &upstream_ip)?);
    info!(system = %v1.system_version, "initial snapshot loaded");

    let (advertised, _rx) = watch::channel(v1);
    let control = Control {
        advertised,
        upstream_ip,
        route_version: Arc::new(AtomicU64::new(1)),
    };

    tokio::spawn(run_admin(control.clone()));

    let server = AdsServer {
        control: control.clone(),
    };
    let addr = XDS_LISTEN.parse()?;
    info!(%addr, "delta xDS server listening");

    Server::builder()
        .add_service(AggregatedDiscoveryServiceServer::new(server))
        .serve_with_shutdown(addr, async {
            let _ = tokio::signal::ctrl_c().await;
            info!("shutdown signal received");
        })
        .await?;

    Ok(())
}
