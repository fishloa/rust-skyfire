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
    let live = args.live.clone();
    let mgr = std::sync::Arc::new(manager::Manager::new(args.fixtures, live.clone()));

    for slug in &live {
        let mgr = mgr.clone();
        let slug = slug.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_millis(200));
            loop {
                ticker.tick().await;
                mgr.feed_live_step(&slug, 256 * 1024);
                if mgr.at_eof(&slug) {
                    break;
                }
            }
        });
    }

    let app = routes::router(mgr);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], args.port));
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    eprintln!("skyfire-server on http://{addr}  (fixtures served as HLS-of-TS)");
    axum::serve(listener, app).await.expect("serve");
}
