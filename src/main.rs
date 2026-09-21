use bgpx::{
    app::{self, App},
    session::Config,
};
use clap::Parser;
use serde_json::json;
use std::{net::IpAddr, time::Duration};

// glibc's malloc keeps freed RIB memory in its own free list for reuse instead of returning
// it to the OS, so RSS stays at its high-water mark after a burst of withdraws. malloc_trim
// asks it to actually give unused pages back.
#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn malloc_trim(pad: usize) -> i32;
}
#[cfg(target_os = "linux")]
fn trim_heap() {
    unsafe { malloc_trim(0) };
}
#[cfg(not(target_os = "linux"))]
fn trim_heap() {}

#[derive(Parser)]
#[command(
    version,
    about = "BGP Unicast and FlowSpec receiver with an embedded web UI"
)]
struct Args {
    #[arg(long, default_value = "0.0.0.0")]
    host: IpAddr,
    #[arg(long, default_value_t = 8080)]
    port: u16,
    #[arg(long)]
    local_as: Option<u32>,
    #[arg(long)]
    router_id: Option<std::net::Ipv4Addr>,
    #[arg(long)]
    peer_ip: Option<std::net::Ipv4Addr>,
    #[arg(long)]
    peer_as: Option<u32>,
    #[arg(long, default_value_t = 90)]
    hold_time: u16,
    #[arg(long, default_value_t = 5.0)]
    reconnect_delay: f64,
    #[arg(long, default_value_t = 5.0)]
    connect_timeout: f64,
    #[arg(long, default_value_t = 1.0)]
    active_retry_delay: f64,
    #[arg(long, default_value_t = 179)]
    listen_port: u16,
    #[arg(long)]
    json_output: Option<String>,
    #[arg(long,default_value="INFO",value_parser=["DEBUG","INFO","WARNING","ERROR"])]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(args.log_level.to_lowercase())
        .with_writer(std::io::stderr)
        .init();
    let config = match (args.local_as, args.router_id, args.peer_ip, args.peer_as) {
        (Some(local_as), Some(router_id), Some(peer_ip), Some(peer_as)) => Some(Config {
            local_as,
            router_id,
            peer_ip,
            peer_as,
            hold_time: args.hold_time,
            reconnect_delay: args.reconnect_delay,
            connect_timeout: args.connect_timeout,
            active_retry_delay: args.active_retry_delay,
            listen_port: args.listen_port,
            json_output: args.json_output.clone(),
        }),
        (None, None, None, None) => None,
        _ => {
            return Err(
                "Provide all four BGP options: --local-as, --router-id, --peer-ip, --peer-as"
                    .into(),
            )
        }
    };
    let app = App::new(args.json_output);
    let listener = tokio::net::TcpListener::bind((args.host, args.port)).await?;
    if let Some(config) = config {
        app.start(config).await?;
    }
    let persistence = tokio::spawn({
        let app = app.clone();
        async move {
            let mut tick: u32 = 0;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                tick += 1;
                let app = app.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = app.flush(false) {
                        app.emit("error", "error", e, json!({}));
                    }
                    // ponytail: fixed 30s cadence rather than triggering off withdraw volume;
                    // malloc_trim isn't free to call, and a flat timer is simpler than tracking
                    // how much was freed since the last trim.
                    if tick.is_multiple_of(30) {
                        app.reclaim_memory();
                        trim_heap();
                    }
                })
                .await;
            }
        }
    });
    eprintln!("Web UI: http://{}", listener.local_addr()?);
    let shutdown_app = app.clone();
    let shutdown = async move {
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        shutdown_app.shutdown.send_replace(true);
        shutdown_app.stop().await;
    };
    let result = axum::serve(listener, app::router(app.clone()))
        .with_graceful_shutdown(shutdown)
        .await;
    persistence.abort();
    let _ = persistence.await;
    result?;
    Ok(())
}
