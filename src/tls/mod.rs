mod cert;
mod test;

use std::sync::Arc;

use anyhow::{anyhow, Error};
use rustls::{
    client::{ClientConfig, ClientSessionMemoryCache, Resumption},
    server::{ResolvesServerCert, ServerConfig, ServerSessionMemoryCache},
    version::{TLS12, TLS13},
    RootCertStore,
};

use crate::{
    cli::Cli,
    core::Run,
    http,
    tls::cert::{providers, storage::Storage, Aggregator},
};

use cert::providers::ProvidesCertificates;

pub const ALPN_H1: &[u8] = b"http/1.1";
pub const ALPN_H2: &[u8] = b"h2";
pub const ALPN_HTTP: &[&[u8]] = &[ALPN_H1, ALPN_H2];

pub fn prepare_server_config(resolver: Arc<dyn ResolvesServerCert>) -> ServerConfig {
    let mut cfg = ServerConfig::builder_with_protocol_versions(&[&TLS13, &TLS12])
        .with_no_client_auth()
        .with_cert_resolver(resolver);

    // Create custom session storage with higher limit to allow effective TLS session resumption
    cfg.session_storage = ServerSessionMemoryCache::new(131_072);
    cfg.alpn_protocols = vec![ALPN_H2.to_vec(), ALPN_H1.to_vec()];

    cfg
}

pub fn prepare_client_config() -> ClientConfig {
    let root_store = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.into(),
    };

    // TODO no revocation checking currently
    let mut cfg = ClientConfig::builder_with_protocol_versions(&[&TLS13, &TLS12])
        .with_root_certificates(root_store)
        .with_no_client_auth();

    // Session resumption
    let store = ClientSessionMemoryCache::new(2048);
    cfg.resumption = Resumption::store(Arc::new(store));
    cfg.alpn_protocols = vec![ALPN_H2.to_vec(), ALPN_H1.to_vec()];

    cfg
}

// Prepares the stuff needed for serving TLS
pub fn setup(
    cli: &Cli,
    http_client: Arc<dyn http::Client>,
) -> Result<(Arc<dyn Run>, ServerConfig), Error> {
    let mut providers = vec![];

    for v in &cli.cert.dir {
        providers.push(Arc::new(providers::Dir::new(v.clone())) as Arc<dyn ProvidesCertificates>);
    }

    for v in &cli.cert.syncer_urls {
        providers.push(
            Arc::new(providers::Syncer::new(http_client.clone(), v.clone()))
                as Arc<dyn ProvidesCertificates>,
        );
    }

    if providers.is_empty() {
        return Err(anyhow!(
            "No certificate providers specified - HTTPS cannot be used"
        ));
    }

    let storage = Arc::new(Storage::new());
    let aggregator = Arc::new(Aggregator::new(providers, storage.clone()));
    let config = prepare_server_config(storage);

    Ok((aggregator, config))
}
