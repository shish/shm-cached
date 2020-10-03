#![feature(poll_map)]
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use utime;
use warp::{http::Response, http::Uri, Filter};
use structopt::StructOpt;

mod types;
mod silos;

use crate::types::*;
use crate::silos::spawn_balancer;

#[tokio::main]
async fn main() {
    let args = Args::from_args();
    let dsn = args.dsn.clone();
    pretty_env_logger::init();

    let silos = HashMap::new();

    let locked_stats = GlobalStats::default();
    let locked_args = Arc::new(RwLock::new(args));
    let locked_silos = Arc::new(RwLock::new(silos));

    spawn_balancer(locked_silos.clone(), dsn).await;
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
        .and(warp::header::<String>("host"))
        .and_then(handle_request);

    let routes = stats_path.or(cache_path);

    warp::serve(routes).run(([0, 0, 0, 0], 8050)).await;
}

fn spawn_summary(locked_stats: GlobalStats) {
    tokio::spawn(async move {
        loop {
            {
                let stats = locked_stats.read().await;
                println!("hits={} misses={}", stats.hits, stats.misses);
            }
            tokio::time::delay_for(Duration::from_secs(60)).await;
        }
    });
}

async fn show_stats(locked_stats: GlobalStats) -> Result<impl warp::reply::Reply, warp::Rejection> {
    let stats = locked_stats.read().await;
    // Ok(warp::reply::json(stats))
    Ok(format!(
        "Requests: {}
Hits: {}
Misses: {}
Errors: {}
Invalid: {}
Missing: {}
Redirect: {}
",
        stats.requests,
        stats.hits,
        stats.misses,
        stats.errors,
        stats.invalid,
        stats.missing,
        stats.redirect
    ))
}

async fn handle_request(
    silo: String,
    hash: String,
    human: String,
    locked_args: GlobalArgs,
    locked_stats: GlobalStats,
    locked_silos: GlobalSilos,
    host: String,
) -> Result<Box<dyn warp::Reply>, warp::Rejection> {
    let args = locked_args.read().await;
    let silos = locked_silos.read().await;
    let me = host.split(".").next().unwrap().to_string();

    {
        let mut stats = locked_stats.write().await;
        stats.requests += 1;
    }

    let owners = match silos.get(&silo) {
        Some(s) => s.lookup_list(&hash, 2),
        None => {
            let mut stats = locked_stats.write().await;
            stats.invalid += 1;
            panic!("No such silo: {}", silo)
        }
    };

    let path = Path::new(args.cache.as_str())
        .join(&silo)
        .join(&hash[0..2])
        .join(&hash[2..4])
        .join(&hash);
    let url = Path::new(args.backend.as_str())
        .join(&silo)
        .join(&hash)
        .join("human.jpg");

    let owner = owners.get(0).unwrap().clone();
    let backup = owners.get(1).unwrap().clone();
    if owner != me && backup != me {
        {
            let mut stats = locked_stats.write().await;
            stats.redirect += 1;
        }
        if path.exists() {
            if let Err(x) = std::fs::remove_file(path.clone()) {
                println!("Failed to remove {:?}: {}", path, x);
            }
        }

        let target = format!("https://{}.paheal.net/{}/{}/{}", owner, silo, hash, human)
            .parse::<Uri>()
            .unwrap();
        return Ok(Box::new(warp::redirect(target)));
    }

    let mtime: SystemTime;
    let body: Vec<u8>;
    if path.exists() {
        {
            let mut stats = locked_stats.write().await;
            stats.hits += 1;
        }

        let mtime_secs = utime::get_file_times(path.clone()).unwrap().1;
        mtime = UNIX_EPOCH + Duration::from_secs(mtime_secs as u64);
        body = fs::read(path).unwrap();
    } else {
        {
            let mut stats = locked_stats.write().await;
            stats.misses += 1;
        }

        // headers={"User-Agent": "shm-cached"}
        let res = reqwest::get(url.to_str().unwrap()).await.unwrap();
        let headers = res.headers();
        if res.status() != reqwest::StatusCode::OK {
            panic!("Bad response: {}", res.status());
        }

        mtime = httpdate::parse_http_date(headers.get("last-modified").unwrap().to_str().unwrap())
            .unwrap();
        body = Vec::from(res.text().await.unwrap());

        fs::create_dir_all(path.parent().unwrap()).expect("Failed to create parent dir");
        fs::write(path.clone(), &body).expect("Failed to write file");
        let mtime_secs = mtime.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        utime::set_file_times(path.clone(), mtime_secs, mtime_secs).expect("Failed to set mtime");
    }
    return Ok(Box::new(
        Response::builder()
            .status(200)
            .header("Content-Type", "image/jpeg")
            .header("Last-Modified", httpdate::fmt_http_date(mtime))
            .header("Cache-Control", "public, max-age=31556926")
            .body(body),
    ));
}
