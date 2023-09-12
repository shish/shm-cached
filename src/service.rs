use http_body_util::BodyExt;
use hyper::body::Bytes;
use hyper::{Request, StatusCode};
use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::UNIX_EPOCH;
// use tower_http::trace::TraceLayer;

use crate::*;

pub enum CacheResult {
    Hit(&'static str, std::time::SystemTime, Vec<u8>),
    Miss(&'static str, std::time::SystemTime, Vec<u8>),
    Redirect(String),
    NotFound,
}

pub async fn cache_request(
    name: &String,
    args: &Args,
    path: &str,
    referer: Option<&str>,
    locked_silos: &GlobalSilos,
    locked_stats: &GlobalStats,
) -> Result<CacheResult> {
    let parts = path.split('/').collect::<Vec<&str>>();
    if parts.len() != 4 {
        return Ok(CacheResult::NotFound);
    }
    let silo = parts[1].to_string();
    let hash = parts[2].to_string();
    let human = parts[3].to_string();
    let stats = {
        let s = locked_stats.read().await;
        match s.get(&silo) {
            Some(s) => s.clone(),
            None => return Ok(CacheResult::NotFound),
        }
    };
    let owners = {
        let silos = locked_silos.read().await;
        match silos.get(&silo) {
            Some(s) => s.lookup_list(&hash, 2),
            None => return Ok(CacheResult::NotFound),
        }
    };
    stats.inflight.fetch_add(1, Ordering::SeqCst);
    let ret =
        handle_request_inner(silo, hash, human, args, &stats, &owners, name, referer).await;
    stats.inflight.fetch_sub(1, Ordering::SeqCst);
    ret
}

async fn handle_request_inner(
    silo: String,
    hash: String,
    human: String,
    args: &Args,
    stats: &Arc<crate::stats::Stats>,
    owners: &Vec<String>,
    me: &String,
    referer: Option<&str>,
) -> Result<CacheResult> {
    stats.requests.fetch_add(1, Ordering::SeqCst);
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
        if let Some(referer) = referer {
            if referer.contains("paheal.net") {
                stats.paheal.fetch_add(1, Ordering::SeqCst);
            } else if referer.contains("google") {
                stats.google.fetch_add(1, Ordering::SeqCst);
                let target = format!("https://holly.paheal.net/_thumbs/{}/thumb.jpg", hash);
                return Ok(CacheResult::Redirect(target));
            } else {
                stats.external.fetch_add(1, Ordering::SeqCst);
            }
        } else {
            stats.norefer.fetch_add(1, Ordering::SeqCst);
        }
    }

    let path = Path::new(args.cache.as_str())
        .join(&silo)
        .join(&hash[0..2])
        .join(&hash[2..4])
        .join(&hash);
    let url = Path::new(args.backend.as_str())
        .join(&silo)
        .join(&hash)
        .join("human.jpg");
    let url = url.to_string_lossy().parse::<hyper::Uri>().unwrap();

    // ================================================================
    // If we don't own this image, redirect to the owner
    // ================================================================
    let default = "no-backup".to_string();
    let owner = owners.get(0).unwrap().clone();
    let backup = owners.get(1).unwrap_or(&default).clone();
    if owner != me.deref() && backup != me.deref() {
        /*
        if path.exists() {
            if let Err(x) = fs::remove_file(&path).await {
                error!("Failed to remove {:?}: {}", path, x);
            }
            stats.cleaned.fetch_add(1, Ordering::SeqCst);
        }
        */

        // TODO: parse this out of the image_ilink / tlink etc
        let target = format!("https://{}.paheal.net/{}/{}/image.jpg", owner, silo, hash);

        stats.redirect.fetch_add(1, Ordering::SeqCst);
        return Ok(CacheResult::Redirect(target));
    }

    // ================================================================
    // If we own this image and it's on disk, fetch from disk
    // ================================================================
    return if path.exists() {
        stats.block_disk.fetch_add(1, Ordering::SeqCst);
        let maybe_mtime_body = fetch_file(&path).await;
        stats.block_disk.fetch_sub(1, Ordering::SeqCst);

        stats.hits.fetch_add(1, Ordering::SeqCst);
        let (mtime, body) = maybe_mtime_body?;
        Ok(CacheResult::Hit(content_type, mtime, body))
    }
    // ================================================================
    // If we own this image and it's missing, fetch from upstream
    // ================================================================
    else {
        stats.block_net.fetch_add(1, Ordering::SeqCst);
        let res = fetch_url(url.clone()).await;
        stats.block_net.fetch_sub(1, Ordering::SeqCst);
        let mut res = res?;

        if res.status() != StatusCode::OK {
            stats.missing.fetch_add(1, Ordering::SeqCst);
            return Ok(CacheResult::NotFound);
        }

        let mtime = httpdate::parse_http_date(
            res.headers()
                .get(http::header::LAST_MODIFIED)
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let mut body = Vec::new();
        while let Some(next) = res.frame().await {
            let frame = next?;
            if let Some(chunk) = frame.data_ref() {
                body.extend_from_slice(chunk);
            }
        }

        let body_to_write = body.clone();
        tokio::spawn(async move {
            tokio::fs::create_dir_all(path.parent().unwrap())
                .await
                .expect("Failed to create parent dir");
            tokio::fs::write(&path, &body_to_write)
                .await
                .expect("Failed to write file");
            let mtime_secs = mtime.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
            utime::set_file_times(&path, mtime_secs, mtime_secs).expect("Failed to set mtime");
        });

        stats.misses.fetch_add(1, Ordering::SeqCst);

        Ok(CacheResult::Miss(content_type, mtime, body))
    };
}

async fn fetch_url(url: hyper::Uri) -> Result<hyper::Response<hyper::body::Incoming>> {
    let host = url.host().expect("uri has no host");
    let port = url.port_u16().unwrap_or(80);
    let addr = format!("{}:{}", host, port);
    let stream = tokio::net::TcpStream::connect(addr).await?;
    let io = hyper_util::rt::TokioIo::new(stream);

    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
    tokio::task::spawn(async move {
        if let Err(err) = conn.await {
            tracing::error!("Connection failed: {:?}", err);
        }
    });

    let authority = url.authority().unwrap().clone();

    let req = Request::builder()
        .uri(url)
        .header(hyper::header::HOST, authority.as_str())
        .body(http_body_util::Empty::<Bytes>::new())
        .unwrap();

    return Ok(sender.send_request(req).await?);
}

async fn fetch_file(path: &std::path::PathBuf) -> Result<(std::time::SystemTime, Vec<u8>)> {
    let mtime = tokio::fs::metadata(&path).await?.modified()?;
    let body = tokio::fs::read(&path).await?;
    Ok((mtime, body))
}
