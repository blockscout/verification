use std::net::SocketAddr;

use lazy_static::lazy_static;

use actix_web::{dev::Server, App, HttpServer};
use actix_web_prom::{PrometheusMetrics, PrometheusMetricsBuilder};
use prometheus::{register_int_counter_vec, IntCounterVec, Registry};

lazy_static! {
    pub static ref VERIFICATION: IntCounterVec = register_int_counter_vec!(
        "verify_contract",
        "number of contract verifications",
        &["language", "endpoint", "status"]
    )
    .unwrap();
}

fn build_registry() -> Registry {
    let registry = Registry::new();
    registry.register(Box::new(VERIFICATION.clone())).unwrap();
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
