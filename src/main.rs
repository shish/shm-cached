use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use structopt::StructOpt;
use tokio::fs;
use tokio::sync::RwLock;
use warp::{http::Response, http::Uri, Filter};

mod db;
mod types;

#[macro_use]
extern crate log;

use crate::db::spawn_db_listener;
use crate::types::*;

#[tokio::main]
async fn main() {
    let args = Args::from_args();
    let dsn = args.dsn.clone();

    pretty_env_logger::init();
    let fqdn = gethostname::gethostname().into_string().unwrap();
    let name = match args.name.clone() {
        Some(name) => name,
        None => fqdn.split('.').next().unwrap().to_string(),
    };
    info!(
        "shm-cached {} built on {} - running on {} ({})",
        env!("VERGEN_SHA_SHORT"),
        env!("VERGEN_BUILD_DATE"),
        fqdn,
        name,
    );
    if args.version {
        return;
    }

    let silos = HashMap::new();

    let locked_stats = GlobalStats::default();
    let locked_args = Arc::new(RwLock::new(args.clone()));
    let locked_silos = Arc::new(RwLock::new(silos));

    spawn_db_listener(
        dsn.clone(),
        args.cache,
        locked_silos.clone(),
        locked_stats.clone(),
    )
    .await;
    spawn_summary(locked_stats.clone());

    // GET /stats -> show stats
    let locked_stats2 = locked_stats.clone();
    let stats_path = warp::path("stats")
        .and(warp::any().map(move || locked_stats2.clone()))
        .and_then(show_stats);

    // GET /<silo>/<hash>/<room> -> fetch from cache
    let cache_path = warp::path!(String / String / String)
        .and(warp::any().map(move || locked_args.clone()))
        .and(warp::any().map(move || locked_stats.clone()))
        .and(warp::any().map(move || locked_silos.clone()))
        .and(warp::any().map(move || name.clone()))
        .and_then(handle_request);

    let routes = stats_path.or(cache_path);

    if let Some(tls) = args.tls {
        let http = warp::serve(routes.clone()).run(([0, 0, 0, 0], args.port));
        let https = warp::serve(routes)
            .tls()
            .cert_path(format!("{}/fullchain.pem", tls))
            .key_path(format!("{}/privkey.pem", tls))
            .run(([0, 0, 0, 0], args.sport));
        privdrop(args.user);
        futures::future::join(http, https).await;
    } else {
        let http = warp::serve(routes.clone()).run(([0, 0, 0, 0], args.port));
        privdrop(args.user);
        http.await;
    }
}

fn privdrop(user: Option<String>) {
    if let Some(user) = user {
        privdrop::PrivDrop::default()
            // .chroot("/var/empty")
            .user(user)
            .apply()
            .unwrap();
    }
}

fn spawn_summary(locked_stats: GlobalStats) {
    tokio::spawn(async move {
        let mut last_hit = 0;
        let mut last_miss = 0;
        let mut last_hitrate = 0;
        loop {
            {
                let stats = locked_stats.read().await;

                let total = stats.hits - last_hit + stats.misses - last_miss;
                let hitrate = if total > 0 {
                    (stats.hits - last_hit) * 100 / total
                } else {
                    last_hitrate
                };
                let msg = format!("shm_cached {},hitrate={}", stats.to_string(), hitrate);
                debug!("{}", msg);
                {
                    let socket = std::net::UdpSocket::bind("127.0.0.1:0")
                        .expect("failed to bind host socket");
                    socket
                        .send_to(msg.as_bytes(), "127.0.0.1:8094")
                        .expect("failed to send message");
                }

                last_hit = stats.hits;
                last_miss = stats.misses;
                last_hitrate = hitrate;
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

async fn show_stats(locked_stats: GlobalStats) -> Result<impl warp::reply::Reply, warp::Rejection> {
    let stats = locked_stats.read().await;
    // Ok(warp::reply::json(stats))
    Ok(stats.to_string())
}

async fn handle_request(
    silo: String,
    hash: String,
    human: String,
    locked_args: GlobalArgs,
    locked_stats: GlobalStats,
    locked_silos: GlobalSilos,
    me: String,
) -> Result<Box<dyn warp::Reply>, warp::Rejection> {
    {
        let mut stats = locked_stats.write().await;
        stats.inflight += 1;
    }
    let ret = handle_request_inner(
        silo,
        hash,
        human,
        locked_args,
        locked_stats.clone(),
        locked_silos,
        me,
    )
    .await;
    {
        let mut stats = locked_stats.write().await;
        stats.inflight -= 1;
    }
    return ret;
}

async fn handle_request_inner(
    silo: String,
    hash: String,
    human: String,
    locked_args: GlobalArgs,
    locked_stats: GlobalStats,
    locked_silos: GlobalSilos,
    me: String,
) -> Result<Box<dyn warp::Reply>, warp::Rejection> {
    {
        let mut stats = locked_stats.write().await;
        stats.requests += 1;
    }
    let content_type = if human.ends_with(".mp4") {"video/mp4"} else {"image/jpeg"};

    let owners = {
        let silos = locked_silos.read().await;
        match silos.get(&silo) {
            Some(s) => s.lookup_list(&hash, 2),
            None => {
                let mut stats = locked_stats.write().await;
                stats.invalid += 1;
                return Err(warp::reject::not_found());
            }
        }
    };

    let (path, url) = {
        let args = locked_args.read().await;
        (
            Path::new(args.cache.as_str())
                .join(&silo)
                .join(&hash[0..2])
                .join(&hash[2..4])
                .join(&hash),
            Path::new(args.backend.as_str())
                .join(&silo)
                .join(&hash)
                .join("human.jpg"),
        )
    };

    // ================================================================
    // If we don't own this image, redirect to the owner
    // ================================================================
    let default = "no-backup".to_string();
    let owner = owners.get(0).unwrap().clone();
    let backup = owners.get(1).unwrap_or(&default).clone();
    if owner != me && backup != me {
        /*
        if path.exists() {
            if let Err(x) = fs::remove_file(path.clone()).await {
                error!("Failed to remove {:?}: {}", path, x);
            }
            let mut stats = locked_stats.write().await;
            stats.cleaned += 1;
        }
        */

        // TODO: parse this out of the image_ilink / tlink etc
        let target = format!("https://{}.paheal.net/{}/{}/{}", owner, silo, hash, human)
            .parse::<Uri>()
            .unwrap();

        let mut stats = locked_stats.write().await;
        stats.redirect += 1;
        return Ok(Box::new(warp::redirect(target)));
    }

    // ================================================================
    // If we own this image and it's on disk, serve it
    // ================================================================
    if path.exists() {
        {
            let mut stats = locked_stats.write().await;
            stats.block_disk += 1;
        }
        let mtime_secs = utime::get_file_times(path.clone()).unwrap().1;
        let mtime = UNIX_EPOCH + Duration::from_secs(mtime_secs as u64);
        let body = fs::read(path).await.unwrap();
        {
            let mut stats = locked_stats.write().await;
            stats.block_disk -= 1;
        }

        let mut stats = locked_stats.write().await;
        stats.hits += 1;
        return Ok(Box::new(
            Response::builder()
                .status(200)
                .header("Content-Type", content_type)
                .header("Last-Modified", httpdate::fmt_http_date(mtime))
                .header("Cache-Control", "public, max-age=31556926")
                .body(body),
        ));
    }

    // ================================================================
    // If we own this image and it's missing, fetch it
    // ================================================================
    {
        let mut stats = locked_stats.write().await;
        stats.block_net += 1;
    }
    let res = reqwest::get(url.to_str().unwrap()).await.unwrap();
    {
        let mut stats = locked_stats.write().await;
        stats.block_net -= 1;
    }

    if res.status() != reqwest::StatusCode::OK {
        let mut stats = locked_stats.write().await;
        stats.missing += 1;
        return Err(warp::reject::not_found());
    }

    let headers = res.headers();
    let mtime =
        httpdate::parse_http_date(headers.get("last-modified").unwrap().to_str().unwrap()).unwrap();
    let body = res.bytes().await.unwrap();

    let body_to_write = body.clone();
    tokio::spawn(async move {
        fs::create_dir_all(path.parent().unwrap())
            .await
            .expect("Failed to create parent dir");
        fs::write(path.clone(), &body_to_write)
            .await
            .expect("Failed to write file");
        let mtime_secs = mtime.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        utime::set_file_times(path.clone(), mtime_secs, mtime_secs).expect("Failed to set mtime");
    });

    let mut stats = locked_stats.write().await;
    stats.misses += 1;
    return Ok(Box::new(
        Response::builder()
            .status(200)
            .header("Content-Type", content_type)
            .header("Last-Modified", httpdate::fmt_http_date(mtime))
            .header("Cache-Control", "public, max-age=31556926")
            .body(body),
    ));
}
