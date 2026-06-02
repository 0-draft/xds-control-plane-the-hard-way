//! Static SDS to start mTLS.
//!
//! The Listener from Chapter 2 now terminates TLS. The server certificate is
//! not baked into the Listener; it travels as its own SDS `Secret` resource
//! over the same ADS stream. Because the secret is just another pushable
//! resource, rotating the certificate is the same machinery as a snapshot
//! swap: `POST /rotate` mints a fresh self-signed cert, bumps the version, and
//! pushes a new snapshot. Envoy swaps the cert on live connections without a
//! restart.

mod snapshot;
mod tls;
mod xtls;

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
    DeltaDiscoveryRequest, DeltaDiscoveryResponse, DiscoveryRequest, DiscoveryResponse,
};

use crate::snapshot::Snapshot;

const XDS_LISTEN: &str = "0.0.0.0:18000";
const ADMIN_LISTEN: &str = "0.0.0.0:19000";
const TLS_SANS: &[&str] = &["localhost"];

#[derive(Clone)]
struct Control {
    advertised: watch::Sender<Arc<Snapshot>>,
    upstream_ip: String,
    version: Arc<AtomicU64>,
}

impl Control {
    fn next_version(&self) -> String {
        let n = self.version.fetch_add(1, Ordering::SeqCst) + 1;
        format!("v{n}")
    }

    /// Mint a fresh certificate and advertise a new snapshot carrying it.
    fn rotate(&self) -> anyhow::Result<String> {
        let v = self.next_version();
        let cert = tls::self_signed(TLS_SANS)?;
        let snap = Arc::new(Snapshot::build(&v, &self.upstream_ip, &cert)?);
        let _ = self.advertised.send(snap);
        Ok(v)
    }

    fn status(&self) -> String {
        format!("advertised={}\n", self.advertised.borrow().version)
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
            let mut subscribed: HashSet<String> = HashSet::new();
            let mut sent_version: HashMap<String, String> = HashMap::new();

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
                        subscribed.insert(type_url.clone());

                        if let Some(err) = &req.error_detail {
                            warn!(
                                node = %node_id, kind = "NACK", ty = st,
                                version = %req.version_info, nonce = %req.response_nonce,
                                msg = %err.message, "client rejected config"
                            );
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
            "Delta lands in Chapter 4; Chapter 3 is SotW only",
        ))
    }
}

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
    } else if type_url.ends_with(".Secret") {
        "SDS"
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
        (&Method::POST, "/rotate") => match control.rotate() {
            Ok(v) => (StatusCode::OK, format!("rotated certificate, now {v}\n")),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("error: {e}\n")),
        },
        (_, "/status") => (StatusCode::OK, control.status()),
        _ => (
            StatusCode::NOT_FOUND,
            "routes: POST /rotate, GET /status\n".into(),
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

    let cert = tls::self_signed(TLS_SANS)?;
    let v1 = Arc::new(Snapshot::build("v1", &upstream_ip, &cert)?);
    info!(version = %v1.version, "initial snapshot loaded (with server cert)");

    let (advertised, _rx) = watch::channel(v1);
    let control = Control {
        advertised,
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
