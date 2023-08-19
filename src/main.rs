use anyhow::Result;
use clap::Parser;
use rustls::ServerConfig;
use rustls_acme::caches::DirCache;
use rustls_acme::AcmeConfig;
use std::sync::Arc;
use tokio_stream::StreamExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod db;
mod types;
mod service;

use crate::service::make_service;
use crate::types::*;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "shm_cached=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let fqdn = match args.fqdn.clone() {
        Some(fqdn) => fqdn,
        None => gethostname::gethostname().into_string().unwrap(),
    };
    let name = match args.name.clone() {
        Some(name) => name,
        None => fqdn.split('.').next().unwrap().to_owned(),
    };

    tracing::info!(
        "shm-cached {} built on {} - running on {} ({})",
        env!("VERGEN_GIT_SHA").chars().take(7).collect::<String>(),
        env!("VERGEN_BUILD_DATE"),
        fqdn,
        name,
    );

    let service = make_service(name, &args).await?;

    let http = if let Some(port) = args.port {
        let http_addr = std::net::SocketAddr::from((args.address, port));
        tracing::debug!("listening on {}", http_addr);
        Some(axum_server::bind(http_addr).serve(service.clone()))
    } else { None };

    let https = if let (Some(sport), Some(tls)) = (args.sport, args.tls.clone()) {
        // Let's Encrypt support
        let acceptor = {
            let mut state = AcmeConfig::new([fqdn])
                .contact([format!("mailto:{}", tls)])
                .cache_option(Some(DirCache::new(format!("{}/.tls", args.cache))))
                .directory_lets_encrypt(true)
                .state();
            let rustls_config = ServerConfig::builder()
                .with_safe_defaults()
                .with_no_client_auth()
                .with_cert_resolver(state.resolver());
            let acceptor = state.axum_acceptor(Arc::new(rustls_config));
            tokio::spawn(async move {
                loop {
                    match state.next().await.unwrap() {
                        Ok(ok) => tracing::info!("acme event: {:?}", ok),
                        Err(err) => tracing::error!("acme error: {:?}", err),
                    }
                }
            });
            acceptor
        };

        let https_addr = std::net::SocketAddr::from((args.address, sport));
        tracing::debug!("listening on {}", https_addr);
        Some(
            axum_server::bind(https_addr)
                .acceptor(acceptor)
                .serve(service.clone()),
        )
    } else {
        None
    };

    drop_privs(&args.user)?;

    match(http, https) {
        (Some(http), Some(https)) => {
            let _ = futures::future::join(http, https).await;
        },
        (Some(http), None) => {
            http.await?;
        },
        (None, Some(https)) => {
            https.await?;
        },
        (None, None) => {
            return Err(anyhow::anyhow!("No listener provided, use -p and / or -s for HTTP or HTTPS listener"));
        }
    }

    Ok(())
}

fn drop_privs(user: &Option<String>) -> Result<()> {
    if let Some(user) = user {
        tracing::info!("Dropping from root to {}", user);
        privdrop::PrivDrop::default()
            // .chroot("/var/empty")
            .user(user)
            .apply()?;
    }
    Ok(())
}
