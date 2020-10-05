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
    pub invalid: usize,
    pub errors: usize,
    pub missing: usize,
    pub redirect: usize,
    pub purged: usize,
    pub block_disk_write: usize,
    pub block_disk_read: usize,
    pub block_net_read: usize,
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "requests={} hits={} misses={} errors={} invalid={} missing={} redirect={} purged={} inflight={} net_read={} disk_write={} disk_read={}",
            self.requests,
            self.hits,
            self.misses,
            self.errors,
            self.invalid,
            self.missing,
            self.redirect,
            self.purged,
            self.requests - self.hits - self.misses - self.errors - self.invalid - self.missing - self.redirect,
            self.block_net_read,
            self.block_disk_write,
            self.block_disk_read,
        )
    }
}

impl Stats {
    pub fn reset(&mut self) {
        self.requests = 0;
        self.hits = 0;
        self.misses = 0;
        self.errors = 0;
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
