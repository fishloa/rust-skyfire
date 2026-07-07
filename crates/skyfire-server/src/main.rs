use skyfire_server::{manager, routes};

use clap::Parser;

#[derive(Parser)]
#[command(name = "skyfire-server", about = "Serve fixture TS as HLS-of-TS")]
struct Args {
    /// Directory of `<slug>.ts` fixtures to serve.
    #[arg(long)]
    fixtures: std::path::PathBuf,
    /// Port to listen on.
    #[arg(long, default_value_t = 8090)]
    port: u16,
    /// Slugs to serve in live-sim (rolling) mode instead of VOD.
    #[arg(long)]
    live: Vec<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let mgr = std::sync::Arc::new(manager::Manager::new(args.fixtures, args.live));
    let app = routes::router(mgr);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], args.port));
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    eprintln!("skyfire-server on http://{addr}  (fixtures served as HLS-of-TS)");
    axum::serve(listener, app).await.expect("serve");
}
