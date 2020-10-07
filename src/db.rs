use core::task::Poll;
use flexihash::Flexihash;
use futures::channel::mpsc;
use futures::FutureExt;
use futures::{future, stream, StreamExt};
use std::path::Path;
use tokio_postgres::{AsyncMessage, NoTls};

use crate::types::{GlobalSilos, GlobalStats};

// Backported from nightly #![feature(poll_map)]
// https://github.com/rust-lang/rust/pull/63512/files
pub fn map_err<U, F, T, E>(s: Poll<Option<Result<T, E>>>, f: F) -> Poll<Option<Result<T, U>>>
where
    F: FnOnce(E) -> U,
{
    match s {
        Poll::Ready(Some(Ok(t))) => Poll::Ready(Some(Ok(t))),
        Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(f(e)))),
        Poll::Ready(None) => Poll::Ready(None),
        Poll::Pending => Poll::Pending,
    }
}

pub async fn spawn_db_listener(
    dsn: String,
    cache: String,
    locked_silos: GlobalSilos,
    locked_stats: GlobalStats,
) {
    let (client, mut connection) = tokio_postgres::connect(dsn.as_str(), NoTls).await.unwrap();

    let (tx, mut rx) = mpsc::unbounded();
    // let stream = stream::poll_fn(move |cx| connection.poll_message(cx).map_err(|e| panic!(e)));
    let stream = stream::poll_fn(move |cx| map_err(connection.poll_message(cx), |e| panic!(e)));
    let c = stream.forward(tx).map(|r| r.unwrap());
    tokio::spawn(c);

    client.query("LISTEN config", &[]).await.unwrap();
    populate_silo(&locked_silos, &client, "_thumbs", "image_tlink").await;
    populate_silo(&locked_silos, &client, "_images", "image_ilink").await;

    client.query("LISTEN shm_image_bans", &[]).await.unwrap();
    let existing_bans = client
        .query(
            "SELECT hash FROM image_bans WHERE date > now() - interval '7 days' ORDER BY hash",
            &[],
        )
        .await
        .unwrap();
    for row in existing_bans {
        let hash = row.get(0);
        clean(&cache, &locked_silos, hash).await;
    }

    tokio::spawn(async move {
        loop {
            if let Some(AsyncMessage::Notification(future_notification)) = rx.next().await {
                let notification = future::ready(Some(future_notification)).await.unwrap();
                if notification.channel() == "config" {
                    populate_silo(&locked_silos, &client, "_thumbs", "image_tlink").await;
                    populate_silo(&locked_silos, &client, "_images", "image_ilink").await;
                }
                if notification.channel() == "shm_image_bans" {
                    {
                        let mut stats = locked_stats.write().await;
                        stats.purged += 1;
                    }
                    clean(&cache, &locked_silos, notification.payload()).await;
                }
            }
        }
    });
}

async fn populate_silo(
    locked_silos: &GlobalSilos,
    client: &tokio_postgres::Client,
    name: &str,
    key: &str,
) {
    let name = name.to_string();
    let key = key.to_string();
    let mut silos = locked_silos.write().await;

    let rows = client
        .query("SELECT value FROM config WHERE name = $1::TEXT", &[&key])
        .await
        .unwrap();
    let targets: &str = rows[0].get(0);

    let parts: Vec<&str> = targets.split(|c| c == '{' || c == '}').collect();
    let targets = parts.get(1).unwrap();
    println!("{} -> {}", name, targets);

    let mut fh = Flexihash::new();

    for target in targets.split(',') {
        let parts: Vec<&str> = target.split('=').collect();
        let (target, weight) = match parts.len() {
            2 => (parts[0], parts[1]),
            _ => panic!("Invalid target"),
        };
        fh.add_target(target, u32::from_str_radix(weight, 10).unwrap());
    }

    // println!("{} -> {}", name, fh);
    silos.insert(name.to_string(), fh);
}

async fn clean(cache: &str, locked_silos: &GlobalSilos, hash: &str) {
    let silos = locked_silos.read().await;
    for silo in silos.keys() {
        let path = Path::new(cache)
            .join(&silo)
            .join(&hash[0..2])
            .join(&hash[2..4])
            .join(&hash);
        if path.exists() {
            if let Err(x) = std::fs::remove_file(path.clone()) {
                println!("Failed to remove {:?}: {}", path, x);
            } else {
                println!("Purged {:?}", path);
            }
        }
    }
}
