use clap::Parser;
use flexihash::Flexihash;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Default, Debug)]
pub struct Stats {
    pub requests: usize,
    pub hits: usize,
    pub misses: usize,
    pub redirect: usize,
    pub missing: usize,

    pub paheal: usize,
    pub google: usize,
    pub norefer: usize,
    pub external: usize,

    pub inflight: usize,
    pub block_disk: usize,
    pub block_net: usize,

    pub last_hit: usize,
    pub last_miss: usize,
    pub last_hitrate: usize,
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "requests={},hits={},misses={},redirect={},missing={},\
            paheal={},google={},norefer={},external={},\
            inflight={},block_net={},block_disk={}",
            self.requests,
            self.hits,
            self.misses,
            self.redirect,
            self.missing,
            self.paheal,
            self.google,
            self.norefer,
            self.external,
            self.inflight,
            self.block_disk,
            self.block_net,
        )
    }
}

// HTTP cache optimised for Shimmie galleries
#[derive(Parser, Clone)]
#[clap(author, version, about, long_about = None)]
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

    /// Contact email address for TLS certificates
    #[clap(short = 't')]
    pub tls: Option<String>,

    /// User to switch to after binding sockets
    #[clap(short = 'u')]
    pub user: Option<String>,

    /// This host's name
    #[clap(short = 'n')]
    pub name: Option<String>,

    /// IP address to bind to
    #[clap(short = 'a', default_value = "0.0.0.0")]
    pub address: String,

    /// HTTP Port
    #[clap(short = 'p', default_value = "8080")]
    pub port: u16,

    /// HTTPS Port
    #[clap(short = 's', default_value = "8443")]
    pub sport: u16,

    /// Don't send stats to dashboards
    #[clap(long = "no-stats")]
    pub no_stats: bool,
}

pub type GlobalArgs = Arc<Args>;
pub type GlobalSilos = Arc<RwLock<HashMap<String, Flexihash>>>;
pub type GlobalStats = Arc<RwLock<HashMap<String, Arc<RwLock<Stats>>>>>;
