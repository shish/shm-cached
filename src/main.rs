use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use structopt::StructOpt;
use tokio::fs;
use tokio::sync::RwLock;
use warp::{http::Response, http::Uri, Filter};
use hyper::Client;
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
        name.clone(),
        args.cache,
        locked_silos.clone(),
        locked_stats.clone(),
    )
    .await;
    spawn_summary(name.clone(), locked_stats.clone());

    // GET /robots.txt -> hard-coding which silos shouldn't be crawled
    let robots = warp::path!("robots.txt").map(|| "User-agent: *\nDisallow: /_thumbs/\nAllow: /\n");

    // GET /.well-known/acme-challenge/* -> Let's Encrypt challenge responses
    let certbot = warp::path!(".well-known" / "acme-challenge" / String).and_then(handle_acme);

    // GET /<silo>/<hash>/<room> -> fetch from cache
    let cache_path = warp::path!(String / String / String)
        .and(warp::any().map(move || locked_args.clone()))
        .and(warp::any().map(move || locked_stats.clone()))
        .and(warp::any().map(move || locked_silos.clone()))
        .and(warp::any().map(move || name.clone()))
        .and(warp::header::optional::<String>("referer"))
        .and_then(handle_request);

    let routes = robots.or(certbot).or(cache_path);

    // I really want these to be parameters to a separate function, but
    // when I put this in a separate function it throws a bunch of errors
    let addr: std::net::IpAddr = args.address.parse().unwrap();
    let http_addr = (addr, args.port);
    let https_addr = (addr, args.sport);
    let tls = args.tls;
    let user = args.user;

    let http = warp::serve(routes.clone()).run(http_addr);
    if let Some(tls) = tls {
        let https = warp::serve(routes)
            .tls()
            .cert_path(format!("{}/fullchain.pem", tls))
            .key_path(format!("{}/privkey.pem", tls))
            .run(https_addr);
        drop_privs(user);
        futures::future::join(http, https).await;
    } else {
        drop_privs(user);
        http.await;
    }
}

fn drop_privs(user: Option<String>) {
    if let Some(user) = user {
        privdrop::PrivDrop::default()
            // .chroot("/var/empty")
            .user(user)
            .apply()
            .unwrap();
    }
}

fn spawn_summary(name: String, locked_global_stats: GlobalStats) {
    tokio::spawn(async move {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("failed to bind stats socket");
        loop {
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
                    debug!("{}", msg);
                    socket
                        .send_to(msg.as_bytes(), "127.0.0.1:8094")
                        .expect("failed to send message");

                    stats.last_hit = stats.hits;
                    stats.last_miss = stats.misses;
                    stats.last_hitrate = hitrate;
                }
            }
            tokio::time::delay_for(Duration::from_secs(10)).await;
        }
    });
}

async fn handle_acme(file: String) -> Result<Box<dyn warp::Reply>, warp::Rejection> {
    let certbot = format!("http://localhost:888/.well-known/acme-challenge/{}", file);
    let client = Client::new();
    let resp = client.get(certbot.parse().unwrap()).await.unwrap();
    info!("Acme-challenge: {}", file);
    return Ok(Box::new(resp));
}

async fn handle_request(
    silo: String,
    hash: String,
    human: String,
    locked_args: GlobalArgs,
    locked_global_stats: GlobalStats,
    locked_silos: GlobalSilos,
    me: String,
    referer: Option<String>,
) -> Result<Box<dyn warp::Reply>, warp::Rejection> {
    let global_stats = locked_global_stats.read().await;
    let locked_stats = match global_stats.get(&silo) {
        Some(s) => s,
        None => {
            return Err(warp::reject::not_found());
        }
    };

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
        referer,
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
    locked_stats: Arc<RwLock<Stats>>,
    locked_silos: GlobalSilos,
    me: String,
    referer: Option<String>,
) -> Result<Box<dyn warp::Reply>, warp::Rejection> {
    {
        let mut stats = locked_stats.write().await;
        stats.requests += 1;
    }
    let ext = human.rsplit(".").next().unwrap().as_ref();
    let content_type = match ext {
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "gif" => "image/gif",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "image/jpeg",
    };

    if silo == "_images" {
        let mut stats = locked_stats.write().await;
        if let Some(referer) = referer {
            if referer.contains("paheal.net") {
                stats.paheal += 1;
            } else if referer.contains("google") {
                stats.google += 1;
                let target = format!("https://holly.paheal.net/_thumbs/{}/thumb.jpg", hash)
                    .parse::<Uri>()
                    .unwrap();
                return Ok(Box::new(warp::redirect::temporary(target)));
            } else {
                stats.external += 1;
            }
        } else {
            stats.norefer += 1;
        }
    }

    let owners = {
        let silos = locked_silos.read().await;
        match silos.get(&silo) {
            Some(s) => s.lookup_list(&hash, 2),
            None => {
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
    let client = Client::new();
    let res = client.get(url.to_str().unwrap().parse().unwrap()).await.unwrap();
    {
        let mut stats = locked_stats.write().await;
        stats.block_net -= 1;
    }

    if res.status() != hyper::StatusCode::OK {
        let mut stats = locked_stats.write().await;
        stats.missing += 1;
        return Err(warp::reject::not_found());
    }

    let headers = res.headers();
    let mtime =
        httpdate::parse_http_date(headers.get("last-modified").unwrap().to_str().unwrap()).unwrap();
    let body = hyper::body::to_bytes(res).await.unwrap();

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
