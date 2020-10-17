use flexihash::Flexihash;
use std::collections::HashMap;
use std::sync::Arc;
use structopt::StructOpt;
use tokio::sync::RwLock;

#[derive(Default, Debug)]
pub struct Stats {
    pub requests: usize,
    pub hits: usize,
    pub misses: usize,
    pub redirect: usize,
    pub invalid: usize,
    pub missing: usize,
    pub purged: usize,
    pub cleaned: usize,
    pub inflight: usize,
    pub block_disk: usize,
    pub block_net: usize,
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            //"requests={} hits={} misses={} redirect={} invalid={} missing={} \
            //purged={} cleaned={} inflight={} net_read={} disk_read={}",
            "requests={},hits={},misses={},redirect={},invalid={},missing={}",
            self.requests,
            self.hits,
            self.misses,
            self.redirect,
            self.invalid,
            self.missing,
        )
    }
}

pub type GlobalStats = Arc<RwLock<Stats>>;

#[derive(StructOpt, Clone)]
#[structopt(about = "HTTP cache optimised for Shimmie galleries")]
pub struct Args {
    /// Where the cached files should be stored
    #[structopt(short = "c", default_value = "/data/shm_cache/")]
    pub cache: String,

    /// Where we should fetch files if we don't have a local copy
    #[structopt(short = "b", default_value = "http://localhost:81")]
    pub backend: String,

    /// Where should we find our load balancer settings
    #[structopt(short = "d", default_value = "user=test host=localhost")]
    pub dsn: String,

    /// Path to a folder containing cert.pem and privkey.pem
    #[structopt(short = "t")]
    pub tls: Option<String>,

    /// Show version
    #[structopt(long = "version")]
    pub version: bool,
}

pub type GlobalArgs = Arc<RwLock<Args>>;
pub type GlobalSilos = Arc<RwLock<HashMap<String, Flexihash>>>;
