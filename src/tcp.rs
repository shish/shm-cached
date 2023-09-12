use anyhow::Result;
use futures::StreamExt;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::{Request, Response, StatusCode};
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;

use crate::service::{cache_request, CacheResult};
use crate::stats::GlobalStats;
use crate::Args;
use crate::*;

pub async fn tcp_server(args: &Args, app: App) -> Result<Option<tokio::task::JoinHandle<()>>> {
    if let Some(port) = args.port {
        let http_addr = std::net::SocketAddr::from((args.address, port));
        let listener = tokio::net::TcpListener::bind(http_addr).await?;
        tracing::info!("Listening on http://{}", http_addr);
        Ok(Some(tokio::task::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                serve_stream(hyper_util::rt::TokioIo::new(tcp), app.clone()).unwrap();
            }
        })))
    } else {
        Ok(None)
    }
}

pub async fn tls_server(
    args: &Args,
    app: App,
    fqdn: &String,
) -> Result<Option<tokio::task::JoinHandle<()>>> {
    if let (Some(sport), Some(mailto)) = (args.sport, args.tls.clone()) {
        let https_addr = std::net::SocketAddr::from((args.address, sport));
        let listener = tokio::net::TcpListener::bind(https_addr).await?;
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        let cache = tokio_rustls_acme::caches::DirCache::new(format!("{}/.tls", args.cache));
        let mut incoming = tokio_rustls_acme::AcmeConfig::new([fqdn])
            .contact_push(format!("mailto:{}", mailto))
            .cache(cache)
            .directory_lets_encrypt(true)
            .incoming(incoming, Vec::new());

        tracing::info!("Listening on https://{}", https_addr);
        Ok(Some(tokio::task::spawn(async move {
            while let Some(tls) = incoming.next().await {
                let tls = tls.unwrap();
                serve_stream(hyper_util::rt::TokioIo::new(tls), app.clone()).unwrap();
            }
        })))
    } else {
        Ok(None)
    }
}

fn serve_stream<T: Unpin + tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Sync + 'static>(
    io: hyper_util::rt::TokioIo<T>,
    app: App,
) -> anyhow::Result<()> {
    // Spin up a new task in Tokio so we can continue to listen for new TCP connection on the
    // current task without waiting for the processing of the HTTP1 connection we just received
    // to finish
    tokio::task::spawn(async move {
        // Handle the connection from the client using HTTP1 and pass any
        // HTTP requests received on that connection to the `hello` function
        if let Err(err) = hyper::server::conn::http1::Builder::new()
            .serve_connection(io, app)
            .await
        {
            tracing::debug!("Error serving connection: {:?}", err);
        }
    });

    Ok(())
}

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
        let res = match req.uri().path() {
            "/robots.txt" => Ok(Response::builder()
                .body(fb("User-agent: *\nDisallow: /_thumbs/\nAllow: /\n"))
                .unwrap()),
            "/stats.json" => {
                let global_stats = self.locked_stats.read().await;
                let data = serde_json::to_string(&global_stats.deref()).unwrap();
                Ok(Response::builder().body(fb(data)).unwrap())
            }
            path => cache_request(
                &self.name,
                &self.args,
                path,
                req.headers()
                    .get(http::header::REFERER)
                    .map(|v| v.to_str().unwrap()),
                &self.locked_silos,
                &self.locked_stats,
            )
            .await
            .map(cache_result_to_http),
        };

        res
    }
}

fn cache_result_to_http(cr: CacheResult) -> Response<Full<Bytes>> {
    match cr {
        CacheResult::Hit(content_type, mtime, body) => Response::builder()
            .status(StatusCode::OK)
            .header(http::header::CONTENT_TYPE, content_type)
            .header(http::header::LAST_MODIFIED, httpdate::fmt_http_date(mtime))
            .header(http::header::CACHE_CONTROL, "public, max-age=31556926")
            .body(fb(body))
            .unwrap(),
        CacheResult::Miss(content_type, mtime, body) => Response::builder()
            .status(StatusCode::OK)
            .header(http::header::CONTENT_TYPE, content_type)
            .header(http::header::LAST_MODIFIED, httpdate::fmt_http_date(mtime))
            .header(http::header::CACHE_CONTROL, "public, max-age=31556926")
            .body(fb(body))
            .unwrap(),
        CacheResult::Redirect(target) => Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(http::header::LOCATION, &target)
            .body(fb(format!("Redirecting to {}", target)))
            .unwrap(),
        CacheResult::NotFound => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(fb("Not Found"))
            .unwrap(),
    }
}
