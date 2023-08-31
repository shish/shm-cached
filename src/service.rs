use http_body_util::Full;
use hyper::body::Bytes;
use hyper::{Request, Response, StatusCode};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::time::{Duration, UNIX_EPOCH};
use http_body_util::BodyExt;
// use tower_http::trace::TraceLayer;

use crate::stats::GlobalStats;
use crate::*;

#[derive(Clone)]
pub struct App {
    pub name: String,
    pub args: GlobalArgs,
    pub locked_stats: GlobalStats,
    pub locked_silos: GlobalSilos,
}

impl hyper::service::Service<Request<hyper::body::Incoming>> for App {
    type Response = Response<Full<Bytes>>;
    type Error = anyhow::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<hyper::body::Incoming>) -> Self::Future {
        let s2 = self.clone();
        Box::pin(async move { s2.async_call(req).await })
    }
}

fn fb(data: impl Into<Bytes>) -> Full<Bytes> {
    Full::new(Bytes::from(data.into()))
}

impl App {
    async fn async_call(
        &self,
        req: Request<hyper::body::Incoming>,
    ) -> Result<Response<Full<Bytes>>> {
        fn mk_response(s: String) -> Result<Response<Full<Bytes>>> {
            Ok(Response::builder().body(fb(s)).unwrap())
        }

        tracing::info!("req = {:?}", req);
        let res = match req.uri().path() {
            // "/" => mk_response(format!("home! counter = {:?}", 42)),
            "/robots.txt" => mk_response(format!("User-agent: *\nDisallow: /_thumbs/\nAllow: /\n")),
            "/stats.json" => {
                let global_stats = self.locked_stats.read().await;
                let stats: HashMap<String, crate::stats::Stats> = futures::future::join_all(
                    global_stats
                        .iter()
                        .map(|(k, v)| async move { (k.clone(), v.read().await.clone()) }),
                )
                .await
                .into_iter()
                .collect();
                mk_response(serde_json::to_string(&stats).unwrap())
            }
            _ => {
                let parts = req.uri().path().split('/').collect::<Vec<&str>>();
                if parts.len() != 4 {
                    return Ok(Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(fb("Bad Path"))
                        .unwrap());
                }
                let silo = parts[1].to_string();
                let hash = parts[2].to_string();
                let human = parts[3].to_string();

                let global_stats = self.locked_stats.read().await;
                let locked_stats = match global_stats.get(&silo) {
                    Some(s) => s,
                    None => {
                        return Ok(Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .body(fb("Not Found"))
                            .unwrap());
                    }
                };

                {
                    let mut stats = locked_stats.write().await;
                    stats.inflight += 1;
                }

                let referer = if req.headers().contains_key(http::header::REFERER) {
                    Some(
                        req.headers()[http::header::REFERER]
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
                    self.args.clone(),
                    locked_stats.clone(),
                    self.locked_silos.clone(),
                    self.name.clone(),
                    referer,
                )
                .await;
                {
                    let mut stats = locked_stats.write().await;
                    stats.inflight -= 1;
                }
                ret
            }
        };

        res
    }
}

async fn handle_request_inner(
    silo: String,
    hash: String,
    human: String,
    args: GlobalArgs,
    locked_stats: Arc<RwLock<crate::stats::Stats>>,
    locked_silos: GlobalSilos,
    me: String,
    referer: Option<String>,
) -> Result<Response<Full<Bytes>>> {
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
                return Ok(Response::builder()
                    .status(StatusCode::TEMPORARY_REDIRECT)
                    .header(http::header::LOCATION, target)
                    .body(fb(""))
                    .unwrap());
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
                return Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(fb("Not Found"))
                    .unwrap());
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
    let url = url.to_string_lossy().parse::<hyper::Uri>().unwrap();

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
        return Ok(Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(http::header::LOCATION, target)
            .body(fb(""))
            .unwrap());
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
        let body = tokio::fs::read(&path).await.unwrap();
        {
            let mut stats = locked_stats.write().await;
            stats.block_disk -= 1;
        }

        let mut stats = locked_stats.write().await;
        stats.hits += 1;

        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(http::header::CONTENT_TYPE, content_type)
            .header(http::header::LAST_MODIFIED, httpdate::fmt_http_date(mtime))
            .header(http::header::CACHE_CONTROL, "public, max-age=31556926")
            .body(fb(body))
            .unwrap());
    }

    // ================================================================
    // If we own this image and it's missing, fetch it
    // ================================================================
    {
        let mut stats = locked_stats.write().await;
        stats.block_net += 1;
    }
    let mut res = fetch_url(url).await?;
    {
        let mut stats = locked_stats.write().await;
        stats.block_net -= 1;
    }

    if res.status() != StatusCode::OK {
        let mut stats = locked_stats.write().await;
        stats.missing += 1;
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(fb("Not Found"))
            .unwrap());
    }

    let mtime =
        httpdate::parse_http_date(res.headers().get(http::header::LAST_MODIFIED).unwrap().to_str().unwrap()).unwrap();
    // let body = hyper::body::to_bytes(res).await.unwrap();
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

    let mut stats = locked_stats.write().await;
    stats.misses += 1;

    return Ok(Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, content_type)
        .header(http::header::LAST_MODIFIED, httpdate::fmt_http_date(mtime))
        .header(http::header::CACHE_CONTROL, "public, max-age=31556926")
        .body(fb(body))
        .unwrap());
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