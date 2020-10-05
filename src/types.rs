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
    pub inflight: usize,
    pub block_disk: usize,
    pub block_net: usize,
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "requests={} hits={} misses={} redirect={} invalid={} missing={} purged={} inflight={} net_read={} disk_read={}",
            self.requests,
            self.hits,
            self.misses,
            self.redirect,
            self.invalid,
            self.missing,
            self.purged,
            self.inflight,
            self.block_net,
            self.block_disk,
        )
    }
}

impl Stats {
    pub fn reset(&mut self) {
        self.requests = 0;
        self.hits = 0;
        self.misses = 0;
        self.invalid = 0;
        self.missing = 0;
        self.redirect = 0;
        self.purged = 0;
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
}

pub type GlobalArgs = Arc<RwLock<Args>>;
pub type GlobalSilos = Arc<RwLock<HashMap<String, Flexihash>>>;
