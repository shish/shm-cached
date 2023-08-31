use anyhow::Result;
use futures::StreamExt;

use crate::service::App;
use crate::Args;

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
        let mut incoming = tokio_rustls_acme::AcmeConfig::new([fqdn])
            .contact_push(format!("mailto:{}", mailto))
            .cache(tokio_rustls_acme::caches::DirCache::new(
                "./rustls_acme_cache",
            ))
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
            tracing::error!("Error serving connection: {:?}", err);
        }
    });

    Ok(())
}
