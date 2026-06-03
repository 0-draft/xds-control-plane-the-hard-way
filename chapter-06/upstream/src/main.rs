//! Upstream backend for Chapter 6.
//!
//! One h2c port serves two things at once: ordinary HTTP data (the "hello"
//! body, with an `x-upstream` header so we can tell which backend answered) and
//! the ORCA `OpenRcaService` gRPC. Envoy's out-of-band ORCA client opens
//! `StreamCoreMetrics` on the data endpoint, and we stream an `OrcaLoadReport`
//! every interval whose `application_utilization` is fixed by `ORCA_UTILIZATION`.
//! A "busy" backend reports a high number, an "idle" one a low number.

use std::env;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderValue, Method, Uri};
use axum::response::Response as AxumResponse;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};
use tracing::info;

mod proto {
    include!(concat!(env!("OUT_DIR"), "/orca.rs"));
}

use proto::xds::data::orca::v3::OrcaLoadReport;
use proto::xds::service::orca::v3::open_rca_service_server::{OpenRcaService, OpenRcaServiceServer};
use proto::xds::service::orca::v3::OrcaLoadReportRequest;

#[derive(Clone)]
struct Orca {
    utilization: f64,
}

#[tonic::async_trait]
impl OpenRcaService for Orca {
    type StreamCoreMetricsStream =
        Pin<Box<dyn Stream<Item = Result<OrcaLoadReport, Status>> + Send>>;

    async fn stream_core_metrics(
        &self,
        _request: Request<OrcaLoadReportRequest>,
    ) -> Result<Response<Self::StreamCoreMetricsStream>, Status> {
        // Report on a fixed interval (we dropped the request's report_interval
        // field to keep the vendored protos import-free).
        let interval = Duration::from_secs(1);
        let util = self.utilization;
        info!(
            interval_ms = interval.as_millis() as u64,
            utilization = util,
            "ORCA OOB stream opened"
        );

        let stream = async_stream::stream! {
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                yield Ok(OrcaLoadReport {
                    cpu_utilization: util,
                    application_utilization: util,
                    ..Default::default()
                });
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }
}

async fn hello(method: Method, uri: Uri) -> AxumResponse {
    let hostname = env::var("HOSTNAME").unwrap_or_else(|_| "upstream".to_string());
    let body = format!(
        "hello from {hostname}\npath: {}\nmethod: {}\n",
        uri.path(),
        method
    );
    let mut resp = AxumResponse::new(Body::from(body));
    resp.headers_mut()
        .insert("content-type", HeaderValue::from_static("text/plain"));
    if let Ok(h) = HeaderValue::from_str(&hostname) {
        resp.headers_mut().insert("x-upstream", h);
    }
    resp
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let port: u16 = env::var("UPSTREAM_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(9000);
    let util: f64 = env::var("ORCA_UTILIZATION")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.5);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    // gRPC ORCA service + an HTTP fallback, served together over h2c.
    let orca = OpenRcaServiceServer::new(Orca { utilization: util });
    let app = tonic::service::Routes::new(orca)
        .into_axum_router()
        .fallback(hello);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, utilization = util, "upstream listening (HTTP data + ORCA gRPC over h2c)");
    axum::serve(listener, app).await?;
    Ok(())
}
