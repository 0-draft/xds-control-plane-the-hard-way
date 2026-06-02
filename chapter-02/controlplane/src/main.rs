//! Snapshot swaps and rollback.
//!
//! Chapter 1 held one frozen snapshot. Here the control plane can change its
//! mind: an admin HTTP API on `:19000` pushes a new snapshot version, and the
//! ADS stream forwards it to Envoy without waiting to be asked (server-initiated
//! push, carried over a `tokio::sync::watch` channel).
//!
//! The interesting case is a *broken* push. `POST /push/broken` advertises a
//! Listener Envoy will reject. Envoy NACKs, and the control plane rolls the
//! advertised version back to the last-known-good snapshot, so `:10000` never
//! stops serving. `POST /push/good` is the happy path: a clean version swap
//! that Envoy ACKs.

mod snapshot;

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
    DeltaDiscoveryRequest, DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse,
};

use crate::snapshot::Snapshot;

const XDS_LISTEN: &str = "0.0.0.0:18000";
const ADMIN_LISTEN: &str = "0.0.0.0:19000";

/// Shared, mutable control-plane state. Cloneable; every clone points at the
/// same `watch` channel and last-known-good slot.
#[derive(Clone)]
struct Control {
    /// The snapshot the control plane currently wants Envoy to run. Each ADS
    /// stream holds a receiver and pushes whatever lands here.
    advertised: watch::Sender<Arc<Snapshot>>,
    /// The last snapshot Envoy accepted. Rollback target on NACK.
    last_good: Arc<Mutex<Arc<Snapshot>>>,
    upstream_ip: String,
    /// Monotonic version counter. v1 is the startup snapshot, so the first
    /// push is v2.
    version: Arc<AtomicU64>,
}

impl Control {
    fn next_version(&self) -> String {
        let n = self.version.fetch_add(1, Ordering::SeqCst) + 1;
        format!("v{n}")
    }

    fn push_good(&self) -> anyhow::Result<String> {
        let v = self.next_version();
        let snap = Arc::new(Snapshot::good(&v, &self.upstream_ip)?);
        *self.last_good.lock().unwrap() = snap.clone();
        let _ = self.advertised.send(snap);
        Ok(v)
    }

    fn push_broken(&self) -> anyhow::Result<String> {
        let v = self.next_version();
        let snap = Arc::new(Snapshot::broken(&v, &self.upstream_ip)?);
        // Deliberately do NOT touch last_good: this version is expected to fail.
        let _ = self.advertised.send(snap);
        Ok(v)
    }

    fn status(&self) -> String {
        let advertised = self.advertised.borrow().version.clone();
        let good = self.last_good.lock().unwrap().version.clone();
        format!("advertised={advertised} last_good={good}\n")
    }
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
        request: Request<Streaming<DiscoveryRequest>>,
    ) -> Result<Response<Self::StreamAggregatedResourcesStream>, Status> {
        let peer = request
            .remote_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|| "?".into());
        info!(%peer, "ADS stream opened");

        let mut inbound = request.into_inner();
        let control = self.control.clone();
        let (tx, rx_out) = mpsc::channel::<Result<DiscoveryResponse, Status>>(16);

        tokio::spawn(async move {
            let mut advertised_rx = control.advertised.subscribe();
            // Resource types this client has subscribed to, so a push only goes
            // out for types Envoy actually asked about.
            let mut subscribed: HashSet<String> = HashSet::new();
            // The last version we successfully sent per type, to avoid resending
            // the same version on every keepalive ACK.
            let mut sent_version: HashMap<String, String> = HashMap::new();

            'outer: loop {
                tokio::select! {
                    biased;

                    // Inbound request: SUBSCRIBE / ACK / NACK.
                    maybe = inbound.next() => {
                        let Some(msg) = maybe else { break 'outer; };
                        let req = match msg {
                            Ok(r) => r,
                            Err(e) => { warn!(error = %e, "stream recv error"); break 'outer; }
                        };

                        let type_url = req.type_url.clone();
                        let node_id = req.node.as_ref().map(|n| n.id.clone()).unwrap_or_default();
                        let st = short_type(&type_url);
                        subscribed.insert(type_url.clone());

                        if let Some(err) = &req.error_detail {
                            warn!(
                                node = %node_id, kind = "NACK", ty = st,
                                version = %req.version_info, nonce = %req.response_nonce,
                                msg = %err.message, "client rejected config"
                            );
                            // Roll back: re-advertise the last-known-good snapshot.
                            // The watch arm below picks it up and re-sends it.
                            let good = control.last_good.lock().unwrap().clone();
                            info!(rollback_to = %good.version, "rolling back after NACK");
                            let _ = control.advertised.send(good);
                            continue;
                        }

                        if req.response_nonce.is_empty() {
                            info!(
                                node = %node_id, kind = "SUB ", ty = st,
                                resources = ?req.resource_names, "client subscribed"
                            );
                            let snap = control.advertised.borrow().clone();
                            if push_if_new(&type_url, &snap, &tx, &mut sent_version).await.is_err() {
                                break 'outer;
                            }
                        } else {
                            info!(
                                node = %node_id, kind = "ACK ", ty = st,
                                version = %req.version_info, nonce = %req.response_nonce,
                                "client accepted config"
                            );
                        }
                    }

                    // The advertised snapshot changed: push it for every
                    // subscribed type whose version moved.
                    changed = advertised_rx.changed() => {
                        if changed.is_err() { break 'outer; }
                        let snap = advertised_rx.borrow_and_update().clone();
                        for ty in subscribed.iter().cloned().collect::<Vec<_>>() {
                            if push_if_new(&ty, &snap, &tx, &mut sent_version).await.is_err() {
                                break 'outer;
                            }
                        }
                    }
                }
            }

            info!(%peer, "ADS stream closed");
        });

        let stream = ReceiverStream::new(rx_out);
        Ok(Response::new(Box::pin(stream)))
    }

    type DeltaAggregatedResourcesStream =
        Pin<Box<dyn Stream<Item = Result<DeltaDiscoveryResponse, Status>> + Send + 'static>>;

    async fn delta_aggregated_resources(
        &self,
        _request: Request<Streaming<DeltaDiscoveryRequest>>,
    ) -> Result<Response<Self::DeltaAggregatedResourcesStream>, Status> {
        Err(Status::unimplemented(
            "Delta lands in Chapter 4; Chapter 2 is SotW only",
        ))
    }
}

/// Send `snap`'s response for `type_url`, unless this stream already sent that
/// exact version. Returns `Err` only when the outbound channel is closed.
async fn push_if_new(
    type_url: &str,
    snap: &Snapshot,
    tx: &mpsc::Sender<Result<DiscoveryResponse, Status>>,
    sent_version: &mut HashMap<String, String>,
) -> Result<(), ()> {
    if sent_version.get(type_url).map(String::as_str) == Some(snap.version.as_str()) {
        return Ok(());
    }
    let Some(resp) = snap.build_response(type_url) else {
        return Ok(());
    };
    info!(ty = short_type(type_url), version = %snap.version, "pushing config");
    if tx.send(Ok(resp)).await.is_err() {
        return Err(());
    }
    sent_version.insert(type_url.to_string(), snap.version.clone());
    Ok(())
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
        (&Method::POST, "/push/good") => match control.push_good() {
            Ok(v) => (StatusCode::OK, format!("pushed good {v}\n")),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("error: {e}\n")),
        },
        (&Method::POST, "/push/broken") => match control.push_broken() {
            Ok(v) => (StatusCode::OK, format!("pushed broken {v}\n")),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("error: {e}\n")),
        },
        (_, "/status") => (StatusCode::OK, control.status()),
        _ => (
            StatusCode::NOT_FOUND,
            "routes: POST /push/good, POST /push/broken, GET /status\n".into(),
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

    let v1 = Arc::new(Snapshot::good("v1", &upstream_ip)?);
    info!(version = %v1.version, "initial snapshot loaded");

    let (advertised, _rx) = watch::channel(v1.clone());
    let control = Control {
        advertised,
        last_good: Arc::new(Mutex::new(v1)),
        upstream_ip,
        version: Arc::new(AtomicU64::new(1)),
    };

    tokio::spawn(run_admin(control.clone()));

    let server = AdsServer {
        control: control.clone(),
    };
    let addr = XDS_LISTEN.parse()?;
    info!(%addr, "xDS server listening");

    Server::builder()
        .add_service(AggregatedDiscoveryServiceServer::new(server))
        .serve_with_shutdown(addr, async {
            let _ = tokio::signal::ctrl_c().await;
            info!("shutdown signal received");
        })
        .await?;

    Ok(())
}
