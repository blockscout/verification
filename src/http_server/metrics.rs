use std::net::SocketAddr;

use lazy_static::lazy_static;

use actix_web::{dev::Server, App, HttpServer};
use actix_web_prom::{PrometheusMetrics, PrometheusMetricsBuilder};
use prometheus::{
    register_histogram, register_int_counter_vec, Histogram, IntCounterVec, Registry,
};

use crate::{VerificationResponse, VerificationStatus};

lazy_static! {
    pub static ref VERIFICATION: IntCounterVec = register_int_counter_vec!(
        "verify_contract",
        "number of contract verifications",
        &["language", "endpoint", "status"],
    )
    .unwrap();
    pub static ref COMPILER_FETCH_TIME: Histogram = register_histogram!(
        "compiler_fetch_time",
        "donwload time for compilers",
        vec![0.1, 0.5, 1.0, 2.0, 3.0, 5.0, 7.0, 9.0, 20.0],
    )
    .unwrap();
    pub static ref COMPILE_TIME: Histogram =
        register_histogram!("compile_time", "contract compilation time").unwrap();
}

pub fn count_verify_contract(response: &VerificationResponse, method: &str) {
    let status = match response.status {
        VerificationStatus::Ok => "ok",
        VerificationStatus::Failed => "fail",
    };
    VERIFICATION
        .with_label_values(&["solidity", method, status])
        .inc();
}

fn build_registry() -> Registry {
    let registry = Registry::new();
    registry.register(Box::new(VERIFICATION.clone())).unwrap();
    registry
        .register(Box::new(COMPILER_FETCH_TIME.clone()))
        .unwrap();
    registry.register(Box::new(COMPILE_TIME.clone())).unwrap();
    registry
}

#[derive(Clone)]
pub struct Metrics {
    endpoint: PrometheusMetrics,
    middleware: PrometheusMetrics,
}

impl Metrics {
    pub fn new(endpoint: String) -> Self {
        let shared_registry = build_registry();

        let endpoint = PrometheusMetricsBuilder::new("verification_metrics_endpoint")
            .registry(shared_registry.clone())
            .endpoint(&endpoint)
            .build()
            .unwrap();
        let middleware = PrometheusMetricsBuilder::new("verification")
            .registry(shared_registry)
            .build()
            .unwrap();

        Self {
            endpoint,
            middleware,
        }
    }

    pub fn middleware(&self) -> &PrometheusMetrics {
        &self.middleware
    }

    pub fn run_server(&self, addr: SocketAddr) -> Server {
        let endpoint = self.endpoint.clone();
        HttpServer::new(move || App::new().wrap(endpoint.clone()))
            .bind(addr)
            .unwrap()
            .run()
    }
}
