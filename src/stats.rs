use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use serde::Serialize;

#[derive(Default, Debug, Serialize, Clone)]
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

pub type GlobalStats = Arc<RwLock<HashMap<String, Arc<RwLock<Stats>>>>>;

/// Spawn a separate future which will, every 10 seconds, send a bunch of
/// UDP packets containing summaries of how many requests we've served
pub fn spawn_summary(name: &str, locked_global_stats: GlobalStats) {
    let name = name.to_string();
    tokio::spawn(async move {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("failed to bind stats socket");
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            {
                for (silo, locked_stats) in locked_global_stats.read().await.iter() {
                    let mut stats = locked_stats.write().await;
                    let total = stats.hits - stats.last_hit + stats.misses - stats.last_miss;
                    let hitrate = if total > 0 {
                        (stats.hits - stats.last_hit) * 100 / total
                    } else {
                        stats.last_hitrate
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

                    stats.last_hit = stats.hits;
                    stats.last_miss = stats.misses;
                    stats.last_hitrate = hitrate;
                }
            }
        }
    });
}
