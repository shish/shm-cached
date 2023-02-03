use anyhow::Result;
use axum::routing::IntoMakeService;
use axum::extract::Extension;
use axum::http::header;
use axum::http::header::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use tower_http::trace::TraceLayer;
use hyper::client::Client;
use std::path::Path;
use tokio::fs;
use std::time::{Duration, UNIX_EPOCH};
use std::collections::HashMap;
use tokio::sync::RwLock;
use std::sync::Arc;

use crate::db::spawn_db_listener;
use crate::types::*;

struct AppState {
    name: String,
    args: GlobalArgs,
    locked_stats: GlobalStats,
    locked_silos: GlobalSilos,
}

pub async fn make_service(name: String, args: &Args) -> Result<IntoMakeService<Router<()>>> {
    let arc_args = Arc::new(args.clone());
    let silos = HashMap::new();

    let locked_stats = GlobalStats::default();
    let locked_silos = Arc::new(RwLock::new(silos));

    spawn_db_listener(
        &args.dsn,
        &name,
        &args.cache,
        locked_silos.clone(),
        locked_stats.clone(),
    )
    .await?;
    if !args.no_stats {
        spawn_summary(&name, locked_stats.clone());
    }

    let app_state = Arc::new(AppState {
        name: name.clone(),
        args: arc_args,
        locked_stats: locked_stats.clone(),
        locked_silos: locked_silos.clone(),
    });

    let app: _ = Router::new()
    // GET /robots.txt -> hard-coding which silos shouldn't be crawled
    .route(
        "/robots.txt",
        get(|| async { "User-agent: *\nDisallow: /_thumbs/\nAllow: /\n" }),
    )
    // GET /<silo>/<hash>/<human> -> fetch from cache
    .route("/:silo/:hash/:human", get(handle_request))
    .layer(TraceLayer::new_for_http())
    .layer(Extension(app_state));
    Ok(app.into_make_service())
}

/// Spawn a separate future which will, every 10 seconds, send a bunch of
/// UDP packets containing summaries of how many requests we've served
fn spawn_summary(name: &str, locked_global_stats: GlobalStats) {
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

/// Increment in-progress counter, call inner request handler, decrement counter
async fn handle_request(
    axum::extract::Path((silo, hash, human)): axum::extract::Path<(String, String, String)>,
    Extension(state): Extension<Arc<AppState>>,
    headers: axum::http::header::HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let global_stats = state.locked_stats.read().await;
    let locked_stats = match global_stats.get(&silo) {
        Some(s) => s,
        None => {
            return Err((StatusCode::NOT_FOUND, "Not Found".to_string()));
        }
    };

    {
        let mut stats = locked_stats.write().await;
        stats.inflight += 1;
    }

    let referer = if headers.contains_key(axum::http::header::REFERER) {
        Some(
            headers[axum::http::header::REFERER]
                .to_str()
                .unwrap()
                .to_string(),
        )
    } else {
        None
    };
    let ret = handle_request_inner(
        silo,
        hash,
        human,
        state.args.clone(),
        locked_stats.clone(),
        state.locked_silos.clone(),
        state.name.clone(),
        referer,
    )
    .await;
    {
        let mut stats = locked_stats.write().await;
        stats.inflight -= 1;
    }
    Ok(ret)
}

async fn handle_request_inner(
    silo: String,
    hash: String,
    human: String,
    args: GlobalArgs,
    locked_stats: Arc<RwLock<Stats>>,
    locked_silos: GlobalSilos,
    me: String,
    referer: Option<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    {
        let mut stats = locked_stats.write().await;
        stats.requests += 1;
    }
    let ext = human.rsplit('.').next().unwrap();
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
                let target = format!("https://holly.paheal.net/_thumbs/{}/thumb.jpg", hash);
                let mut header_map = HeaderMap::new();
                header_map.insert(header::LOCATION, target.parse().unwrap());
                return Ok((StatusCode::TEMPORARY_REDIRECT, header_map, vec![]));
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
                return Err((StatusCode::NOT_FOUND, "Not Found".to_string()));
            }
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

    // ================================================================
    // If we don't own this image, redirect to the owner
    // ================================================================
    let default = "no-backup".to_string();
    let owner = owners.get(0).unwrap().clone();
    let backup = owners.get(1).unwrap_or(&default).clone();
    if owner != me && backup != me {
        /*
        if path.exists() {
            if let Err(x) = fs::remove_file(&path).await {
                error!("Failed to remove {:?}: {}", path, x);
            }
            let mut stats = locked_stats.write().await;
            stats.cleaned += 1;
        }
        */

        // TODO: parse this out of the image_ilink / tlink etc
        let target = format!("https://{}.paheal.net/{}/{}/image.jpg", owner, silo, hash);

        let mut stats = locked_stats.write().await;
        stats.redirect += 1;
        let mut header_map = HeaderMap::new();
        header_map.insert(header::LOCATION, target.parse().unwrap());
        return Ok((StatusCode::TEMPORARY_REDIRECT, header_map, vec![]));
    }

    // ================================================================
    // If we own this image and it's on disk, serve it
    // ================================================================
    if path.exists() {
        {
            let mut stats = locked_stats.write().await;
            stats.block_disk += 1;
        }
        let mtime_secs = utime::get_file_times(&path).unwrap().1;
        let mtime = UNIX_EPOCH + Duration::from_secs(mtime_secs as u64);
        let body = fs::read(&path).await.unwrap();
        {
            let mut stats = locked_stats.write().await;
            stats.block_disk -= 1;
        }

        let mut stats = locked_stats.write().await;
        stats.hits += 1;

        let mut header_map = HeaderMap::new();
        header_map.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
        header_map.insert(
            header::LAST_MODIFIED,
            httpdate::fmt_http_date(mtime).parse().unwrap(),
        );
        header_map.insert(
            header::CACHE_CONTROL,
            "public, max-age=31556926".parse().unwrap(),
        );
        return Ok((StatusCode::OK, header_map, body));
    }

    // ================================================================
    // If we own this image and it's missing, fetch it
    // ================================================================
    {
        let mut stats = locked_stats.write().await;
        stats.block_net += 1;
    }
    let client = Client::new();
    let res = client
        .get(url.to_str().unwrap().parse().unwrap())
        .await
        .unwrap();
    {
        let mut stats = locked_stats.write().await;
        stats.block_net -= 1;
    }

    if res.status() != StatusCode::OK {
        let mut stats = locked_stats.write().await;
        stats.missing += 1;
        return Err((StatusCode::NOT_FOUND, "Not Found".to_string()));
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
        fs::write(&path, &body_to_write)
            .await
            .expect("Failed to write file");
        let mtime_secs = mtime.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        utime::set_file_times(&path, mtime_secs, mtime_secs).expect("Failed to set mtime");
    });

    let mut stats = locked_stats.write().await;
    stats.misses += 1;

    let mut header_map = HeaderMap::new();
    header_map.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    header_map.insert(
        header::LAST_MODIFIED,
        httpdate::fmt_http_date(mtime).parse().unwrap(),
    );
    header_map.insert(
        header::CACHE_CONTROL,
        "public, max-age=31556926".parse().unwrap(),
    );
    Ok((StatusCode::OK, header_map, body.into()))
}
