use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

#[derive(Default, Debug, Serialize)]
pub struct Stats {
    pub requests: AtomicUsize,
    pub hits: AtomicUsize,
    pub misses: AtomicUsize,
    pub redirect: AtomicUsize,
    pub missing: AtomicUsize,

    pub paheal: AtomicUsize,
    pub google: AtomicUsize,
    pub norefer: AtomicUsize,
    pub external: AtomicUsize,

    pub inflight: AtomicUsize,
    pub block_disk: AtomicUsize,
    pub block_net: AtomicUsize,

    pub last_hit: AtomicUsize,
    pub last_miss: AtomicUsize,
    pub last_hitrate: AtomicUsize,
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "requests={},hits={},misses={},redirect={},missing={},\
            paheal={},google={},norefer={},external={},\
            inflight={},block_net={},block_disk={}",
            self.requests.load(Ordering::Relaxed),
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
            self.redirect.load(Ordering::Relaxed),
            self.missing.load(Ordering::Relaxed),
            self.paheal.load(Ordering::Relaxed),
            self.google.load(Ordering::Relaxed),
            self.norefer.load(Ordering::Relaxed),
            self.external.load(Ordering::Relaxed),
            self.inflight.load(Ordering::Relaxed),
            self.block_disk.load(Ordering::Relaxed),
            self.block_net.load(Ordering::Relaxed),
        )
    }
}

pub type GlobalStats = Arc<RwLock<HashMap<String, Arc<Stats>>>>;

/// Spawn a separate future which will, every 10 seconds, send a bunch of
/// UDP packets containing summaries of how many requests we've served
pub fn spawn_summary(name: &str, locked_global_stats: GlobalStats) {
    let name = name.to_string();
    tokio::spawn(async move {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("failed to bind stats socket");
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            {
                for (silo, stats) in locked_global_stats.read().await.iter() {
                    let total = stats.hits.load(Ordering::SeqCst)
                        - stats.last_hit.load(Ordering::SeqCst)
                        + stats.misses.load(Ordering::SeqCst)
                        - stats.last_miss.load(Ordering::SeqCst);
                    let hitrate = if total > 0 {
                        (stats.hits.load(Ordering::SeqCst) - stats.last_hit.load(Ordering::SeqCst))
                            * 100
                            / total
                    } else {
                        stats.last_hitrate.load(Ordering::SeqCst)
                    };
                    let msg = format!(
                        "shm_cached,name={},silo={} {},hitrate={}",
                        name,
                        silo,
                        stats.to_string(),
                        hitrate
                    );
                    tracing::debug!("{}", msg);
                    socket
                        .send_to(msg.as_bytes(), "127.0.0.1:8094")
                        .expect("failed to send message");

                    stats
                        .last_hit
                        .store(stats.hits.load(Ordering::SeqCst), Ordering::SeqCst);
                    stats
                        .last_miss
                        .store(stats.misses.load(Ordering::SeqCst), Ordering::SeqCst);
                    stats.last_hitrate.store(hitrate, Ordering::SeqCst);
                }
            }
        }
    });
}
