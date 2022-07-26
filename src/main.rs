use verification::{init_logs, run_http_server, Args, Config};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    init_logs();
    let args = Args::default();
    let config = Config::from_file(args.config_path).expect("Failed to parse config");
    run_http_server(config).await
}
