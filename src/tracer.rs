use opentelemetry::{sdk::trace::Tracer, trace::TraceError};
use tracing_subscriber::{filter::LevelFilter, layer::SubscriberExt, prelude::*};

pub fn init_logs() {
    let stdout = tracing_subscriber::fmt::layer().with_filter(
        tracing_subscriber::EnvFilter::builder()
            .with_default_directive(LevelFilter::INFO.into())
            .from_env_lossy(),
    );
    let tracer = init_jaeger_tracer().expect("failed to init tracer");
    tracing_subscriber::registry()
        // output logs (tracing) to stdout with log level taken from env (default is INFO)
        .with(stdout)
        // output traces to jaeger with default log level (default is TRACE)
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .try_init()
        .expect("Failed to register tracer with registry");
}

fn init_jaeger_tracer() -> Result<Tracer, TraceError> {
    opentelemetry_jaeger::new_pipeline()
        .with_service_name("verification")
        .install_simple()
}
