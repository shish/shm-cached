use anyhow::Result;
use clap::Parser;
use flexihash::Flexihash;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod db;
mod service;
mod stats;
mod tcp;

use crate::db::spawn_db_listener;
use crate::service::App;
use crate::stats::{spawn_summary, GlobalStats};
use crate::tcp::{tcp_server, tls_server};

// HTTP cache optimised for Shimmie galleries
#[derive(Parser, Clone)]
#[clap(author, about, long_about = None)]
pub struct Args {
    /// Where the cached files should be stored
    #[clap(short = 'c', default_value = "/data/shm_cache/")]
    pub cache: String,

    /// Where we should fetch files if we don't have a local copy
    #[clap(short = 'b', default_value = "http://localhost:81")]
    pub backend: String,

    /// Where should we find our load balancer settings
    #[clap(short = 'd', default_value = "user=test host=localhost")]
    pub dsn: String,

    /// User to switch to after binding sockets
    #[clap(short = 'u')]
    pub user: Option<String>,

    /// This host's name for consistent hash lookups
    #[clap(short = 'n')]
    pub name: Option<String>,

    /// IP address to bind to
    #[clap(short = 'a', default_value = "0.0.0.0")]
    pub address: std::net::IpAddr,

    /// HTTP Port
    #[clap(short = 'p')]
    pub port: Option<u16>,

    /// HTTPS Port
    #[clap(short = 's')]
    pub sport: Option<u16>,

    /// Contact email address for HTTPS certificates
    #[clap(short = 't')]
    pub tls: Option<String>,

    /// This host's FQDN for HTTPS certificates
    #[clap(short = 'f')]
    pub fqdn: Option<String>,

    /// Don't send stats to dashboards
    #[clap(long = "no-stats")]
    pub no_stats: bool,

    /// Show version
    #[structopt(long = "version")]
    pub version: bool,
}

pub type GlobalArgs = Arc<Args>;
pub type GlobalSilos = Arc<RwLock<HashMap<String, Flexihash>>>;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "shm_cached=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let fqdn = match args.fqdn.clone() {
        Some(fqdn) => fqdn,
        None => gethostname::gethostname().into_string().unwrap(),
    };
    let name = match args.name.clone() {
        Some(name) => name,
        None => fqdn.split('.').next().unwrap().to_owned(),
    };

    tracing::info!(
        "shm-cached {} built on {} - running on {} ({})",
        env!("VERGEN_GIT_SHA").chars().take(7).collect::<String>(),
        env!("VERGEN_BUILD_DATE"),
        fqdn,
        name,
    );
    if args.version {
        return Ok(());
    }

    let locked_stats = GlobalStats::default();
    if !args.no_stats {
        spawn_summary(&name, locked_stats.clone());
    }

    let locked_silos = GlobalSilos::default();
    spawn_db_listener(
        &args.dsn,
        &name,
        &args.cache,
        locked_silos.clone(),
        locked_stats.clone(),
    )
    .await?;

    let app = App {
        name: name.clone(),
        args: Arc::new(args.clone()),
        locked_stats: locked_stats.clone(),
        locked_silos: locked_silos.clone(),
    };

    let http = tcp_server(&args, app.clone()).await?;
    let https = tls_server(&args, app.clone(), &fqdn).await?;

    drop_privs(&args.user)?;

    await_all(vec![http, https]).await?;

    Ok(())
}

fn drop_privs(user: &Option<String>) -> Result<()> {
    if let Some(user) = user {
        tracing::info!("Dropping from root to {}", user);
        privdrop::PrivDrop::default()
            // .chroot("/var/empty")
            .user(user)
            .apply()?;
    }
    Ok(())
}

async fn await_all(servers: Vec<Option<tokio::task::JoinHandle<()>>>) -> Result<()> {
    let servers: Vec<_> = servers.into_iter().filter_map(|x| x).collect();
    if servers.is_empty() {
        return Err(anyhow::anyhow!(
            "No listener provided, use -p and / or -s for HTTP or HTTPS listener"
        ));
    }
    let _ = futures::future::join_all(servers).await;
    Ok(())
}
