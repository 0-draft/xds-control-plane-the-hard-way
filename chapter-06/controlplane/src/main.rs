//! ORCA out-of-band load reports, consumed by the control plane.
//!
//! Envoy's `client_side_weighted_round_robin` does not actually receive ORCA
//! reports from hosts (the wiring was closed as not-planned upstream), so we put
//! the out-of-band ORCA *client* where it can run: in the control plane. It
//! opens `OpenRcaService.StreamCoreMetrics` to each backend, turns the reported
//! utilization into an EDS `load_balancing_weight` (lighter backend, larger
//! weight), and pushes a new snapshot. Envoy's default weighted round robin then
//! sends more traffic to the lighter backend.

mod snapshot;

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

use crate::snapshot::{Snapshot, Weighted};

mod orca {
    include!(concat!(env!("OUT_DIR"), "/orca.rs"));
}
use orca::xds::service::orca::v3::open_rca_service_client::OpenRcaServiceClient;
use orca::xds::service::orca::v3::OrcaLoadReportRequest;

const XDS_LISTEN: &str = "0.0.0.0:18000";
const BACKENDS: &[&str] = &["upstream-a", "upstream-b"];
const UPSTREAM_PORT: u16 = 9000;

#[derive(Clone)]
struct Control {
    advertised: watch::Sender<Arc<Snapshot>>,
    /// Ordered (host, ip) for each backend; ip is the literal EDS address.
    backends: Arc<Vec<(String, String)>>,
    /// host -> last reported utilization.
    util: Arc<Mutex<HashMap<String, f64>>>,
    version: Arc<AtomicU64>,
}

impl Control {
    /// Record a fresh ORCA reading; re-push EDS only when it actually moved.
    fn observe(&self, host: &str, util: f64) {
        let changed = {
            let mut m = self.util.lock().unwrap();
            let prev = m.get(host).copied();
            if prev.map(|p| (p - util).abs() < 0.001).unwrap_or(false) {
                false
            } else {
                m.insert(host.to_string(), util);
                true
            }
        };
        if changed {
            info!(%host, utilization = util, "ORCA report");
            self.recompute_and_push();
        }
    }

    fn recompute_and_push(&self) {
        let weighted: Vec<Weighted> = {
            let util = self.util.lock().unwrap();
            self.backends
                .iter()
                .map(|(host, ip)| {
                    let u = util.get(host).copied().unwrap_or(0.5);
                    Weighted {
                        ip: ip.clone(),
                        weight: weight_from_util(u),
                    }
                })
                .collect()
        };

        let v = format!("v{}", self.version.fetch_add(1, Ordering::SeqCst) + 1);
        match Snapshot::build(&v, &weighted) {
            Ok(snap) => {
                let summary: Vec<String> = self
                    .backends
                    .iter()
                    .zip(&weighted)
                    .map(|((h, _), w)| format!("{h}={}", w.weight))
                    .collect();
                info!(version = %v, weights = ?summary, "pushing EDS weights from ORCA");
                let _ = self.advertised.send(Arc::new(snap));
            }
            Err(e) => warn!(error = %e, "failed to build snapshot"),
        }
    }
}

/// Lighter backend -> larger weight. util 0.1 -> 900, util 0.9 -> 100.
fn weight_from_util(util: f64) -> u32 {
    let u = util.clamp(0.0, 0.99);
    (((1.0 - u) * 1000.0).round() as u32).max(1)
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
                            warn!(node = %node_id, kind = "NACK", ty = st, msg = %err.message, "client rejected config");
                            continue;
                        }
                        if req.response_nonce.is_empty() {
                            info!(node = %node_id, kind = "SUB ", ty = st, resources = ?req.resource_names, "client subscribed");
                            let snap = control.advertised.borrow().clone();
                            if push_if_new(&type_url, &snap, &tx, &mut sent_version).await.is_err() {
                                break 'outer;
                            }
                        } else {
                            info!(node = %node_id, kind = "ACK ", ty = st, version = %req.version_info, "client accepted config");
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

        Ok(Response::new(Box::pin(ReceiverStream::new(rx_out))))
    }

    type DeltaAggregatedResourcesStream =
        Pin<Box<dyn Stream<Item = Result<DeltaDiscoveryResponse, Status>> + Send + 'static>>;

    async fn delta_aggregated_resources(
        &self,
        _request: Request<Streaming<DeltaDiscoveryRequest>>,
    ) -> Result<Response<Self::DeltaAggregatedResourcesStream>, Status> {
        Err(Status::unimplemented("Chapter 6 is SotW only"))
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
    } else {
        "???"
    }
}

/// One per backend: stream ORCA reports and feed them into the control plane.
/// Reconnects forever, because a backend may not be up the instant we start.
async fn run_orca_client(control: Control, host: String) {
    loop {
        let endpoint = format!("http://{host}:{UPSTREAM_PORT}");
        match OpenRcaServiceClient::connect(endpoint).await {
            Ok(mut client) => {
                let req = OrcaLoadReportRequest {
                    request_cost_names: vec![],
                };
                match client.stream_core_metrics(req).await {
                    Ok(resp) => {
                        info!(%host, "ORCA OOB client connected");
                        let mut stream = resp.into_inner();
                        loop {
                            match stream.message().await {
                                Ok(Some(report)) => {
                                    let util = if report.application_utilization > 0.0 {
                                        report.application_utilization
                                    } else {
                                        report.cpu_utilization
                                    };
                                    control.observe(&host, util);
                                }
                                Ok(None) => {
                                    warn!(%host, "ORCA stream ended");
                                    break;
                                }
                                Err(e) => {
                                    warn!(%host, error = %e, "ORCA stream error");
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => warn!(%host, error = %e, "StreamCoreMetrics failed"),
                }
            }
            Err(e) => warn!(%host, error = %e, "ORCA connect failed"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
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

    // Resolve each backend's literal IP for EDS.
    let mut backends = Vec::new();
    for host in BACKENDS {
        let ip = snapshot::resolve_one(host).await?;
        info!(host = %host, %ip, "resolved backend");
        backends.push((host.to_string(), ip));
    }

    // Start with equal weights; ORCA reports adjust them within a second.
    let initial: Vec<Weighted> = backends
        .iter()
        .map(|(_, ip)| Weighted {
            ip: ip.clone(),
            weight: weight_from_util(0.5),
        })
        .collect();
    let v1 = Arc::new(Snapshot::build("v1", &initial)?);
    info!(version = %v1.version, "initial snapshot loaded (equal weights)");

    let (advertised, _rx) = watch::channel(v1);
    let control = Control {
        advertised,
        backends: Arc::new(backends),
        util: Arc::new(Mutex::new(HashMap::new())),
        version: Arc::new(AtomicU64::new(1)),
    };

    // One ORCA OOB client per backend.
    for (host, _) in control.backends.iter() {
        tokio::spawn(run_orca_client(control.clone(), host.clone()));
    }

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
