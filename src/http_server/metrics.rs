use lazy_static::lazy_static;

use actix_web::{dev::Server, App, HttpServer};
use actix_web_prom::{PrometheusMetrics, PrometheusMetricsBuilder};
use prometheus::{register_int_counter_vec, IntCounterVec, Registry};

lazy_static! {
    pub static ref VERIFICATION: IntCounterVec = register_int_counter_vec!(
        "verify_contract",
        "contract verification metrics",
        &["language", "endpoint", "status"]
    )
    .unwrap();
}

fn registry() -> Registry {
    let registry = Registry::new();
    registry.register(Box::new(VERIFICATION.clone())).unwrap();
    registry
}

#[derive(Clone)]
pub struct Metrics {
    private: PrometheusMetrics,
    public: PrometheusMetrics,
}

impl Metrics {
    pub fn new(endpoint: String) -> Self {
        let shared_registry = registry();

        let private = PrometheusMetricsBuilder::new("private_verification")
            .registry(shared_registry.clone())
            .endpoint(&endpoint)
            .build()
            .unwrap();
        let public = PrometheusMetricsBuilder::new("verification")
            .registry(shared_registry)
            .build()
            .unwrap();

        Self { private, public }
    }

    pub fn public(&self) -> &PrometheusMetrics {
        &self.public
    }

    pub fn run_private_server(&self, port: u16) -> Server {
        let private = self.private.clone();
        HttpServer::new(move || App::new().wrap(private.clone()))
            .bind(("0.0.0.0", port))
            .unwrap()
            .run()
    }
}
