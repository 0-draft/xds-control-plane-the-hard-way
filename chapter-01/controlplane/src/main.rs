//! Hello, xDS.
//!
//! A bare-minimum ADS server that does three things in this order:
//! it listens on `:18000` plaintext HTTP/2 gRPC, holds a single hard-coded
//! snapshot `v1` (one Listener, RouteConfiguration, Cluster, and
//! ClusterLoadAssignment), and classifies every inbound `DiscoveryRequest`
//! as SUBSCRIBE / ACK / NACK before responding. No snapshot mutation, no
//! Delta. That's what Chapter 2+ are for.

mod snapshot;

use std::pin::Pin;

use tokio::sync::mpsc;
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

#[derive(Clone)]
struct AdsServer {
    snapshot: Snapshot,
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
        let snapshot = self.snapshot.clone();
        let (tx, rx) = mpsc::channel::<Result<DiscoveryResponse, Status>>(16);

        tokio::spawn(async move {
            // Track the last version we successfully sent for each type_url,
            // so we don't reply on every keepalive ACK.
            let mut sent_versions: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();

            while let Some(msg) = inbound.next().await {
                let req = match msg {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(error = %e, "stream recv error");
                        break;
                    }
                };

                let type_url = req.type_url.clone();
                let node_id = req.node.as_ref().map(|n| n.id.clone()).unwrap_or_default();
                let short_type = short_type(&type_url);

                if let Some(err) = &req.error_detail {
                    warn!(
                        node = %node_id,
                        kind = "NACK",
                        ty = short_type,
                        version = %req.version_info,
                        nonce = %req.response_nonce,
                        msg = %err.message,
                        "client rejected config"
                    );
                    // Resend v1 so the operator can fix and retry without a fresh stream.
                    if let Some(resp) = snapshot.build_response(&type_url) {
                        let _ = tx.send(Ok(resp)).await;
                    }
                    continue;
                }

                let already_sent = sent_versions
                    .get(&type_url)
                    .map(|v| v == &snapshot.version)
                    .unwrap_or(false);

                if !req.response_nonce.is_empty() && req.version_info == snapshot.version {
                    info!(
                        node = %node_id,
                        kind = "ACK ",
                        ty = short_type,
                        version = %req.version_info,
                        nonce = %req.response_nonce,
                        "client accepted config"
                    );
                    continue;
                }

                if already_sent {
                    // A re-subscribe after ACK with the same version: ignore quietly.
                    continue;
                }

                info!(
                    node = %node_id,
                    kind = "SUB ",
                    ty = short_type,
                    resources = ?req.resource_names,
                    "client subscribed"
                );

                let Some(resp) = snapshot.build_response(&type_url) else {
                    warn!(ty = short_type, "unknown type_url, ignoring");
                    continue;
                };

                if tx.send(Ok(resp)).await.is_err() {
                    break;
                }
                sent_versions.insert(type_url, snapshot.version.clone());
            }

            info!(%peer, "ADS stream closed");
        });

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream)))
    }

    type DeltaAggregatedResourcesStream =
        Pin<Box<dyn Stream<Item = Result<DeltaDiscoveryResponse, Status>> + Send + 'static>>;

    async fn delta_aggregated_resources(
        &self,
        _request: Request<Streaming<DeltaDiscoveryRequest>>,
    ) -> Result<Response<Self::DeltaAggregatedResourcesStream>, Status> {
        Err(Status::unimplemented(
            "Chapter 1 is SotW only; Delta lands in Chapter 4",
        ))
    }
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let snapshot = Snapshot::v1().await?;
    info!(version = %snapshot.version, "snapshot loaded");

    let server = AdsServer { snapshot };
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
