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
