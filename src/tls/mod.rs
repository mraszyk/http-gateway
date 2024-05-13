pub mod acme;
pub mod cert;
pub mod resolver;
mod test;

use std::sync::Arc;

use anyhow::{anyhow, Error};
use fqdn::FQDN;
use rustls::{
    client::{ClientConfig, ClientSessionMemoryCache, Resumption},
    server::{ServerConfig, ServerSessionMemoryCache},
    sign::CertifiedKey,
    version::{TLS12, TLS13},
    RootCertStore,
};
use rustls_acme::acme::ACME_TLS_ALPN_NAME;

use crate::{
    cli::Cli,
    core::Runner,
    http::{is_http_alpn, Client, ALPN_H1, ALPN_H2},
    tls::{
        cert::{providers, Aggregator},
        resolver::{AggregatingResolver, ResolvesServerCert},
    },
};

use cert::{providers::ProvidesCertificates, storage::StoresCertificates};

pub fn prepare_server_config(
    resolver: Arc<dyn rustls::server::ResolvesServerCert>,
) -> ServerConfig {
    let mut cfg = ServerConfig::builder_with_protocol_versions(&[&TLS13, &TLS12])
        .with_no_client_auth()
        .with_cert_resolver(resolver);

    // Create custom session storage with higher limit to allow effective TLS session resumption
    cfg.session_storage = ServerSessionMemoryCache::new(131_072);
    cfg.alpn_protocols = vec![
        ALPN_H2.to_vec(),
        ALPN_H1.to_vec(),
        // Support ACME challenge ALPN too
        ACME_TLS_ALPN_NAME.to_vec(),
    ];

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
    domains: Vec<FQDN>,
    http_client: Arc<dyn Client>,
    storage: Arc<dyn StoresCertificates<Arc<CertifiedKey>>>,
    cert_resolver: Arc<dyn ResolvesServerCert>,
) -> Result<(Vec<Runner>, ServerConfig), Error> {
    let mut providers = vec![];
    let mut runners = vec![];

    // Create Dir providers
    for v in &cli.cert.dir {
        providers.push(Arc::new(providers::Dir::new(v.clone())) as Arc<dyn ProvidesCertificates>);
    }

    // Create CertIssuer providers
    for v in &cli.cert.issuer_urls {
        providers.push(
            Arc::new(providers::Syncer::new(http_client.clone(), v.clone()))
                as Arc<dyn ProvidesCertificates>,
        );
    }

    // Prepare ACME if configured
    let acme_resolver = if let Some(v) = &cli.acme.acme_challenge {
        match v {
            acme::Challenge::Alpn => {
                let domains = domains.iter().map(|x| x.to_string()).collect::<Vec<_>>();
                let (run, res) = acme::AcmeTlsAlpn::new(
                    domains,
                    cli.acme.acme_staging,
                    cli.acme.acme_cache_path.clone().unwrap(),
                )?;
                runners.push(Runner("acme_runner".into(), run));
                Some(res)
            }
        }
    } else {
        None
    };

    if acme_resolver.is_none() && providers.is_empty() {
        return Err(anyhow!(
            "No ACME or certificate providers specified - HTTPS cannot be used"
        ));
    }

    let cert_aggregator = Arc::new(Aggregator::new(providers, storage, cli.cert.poll_interval));
    runners.push(Runner("cert_aggregator".into(), cert_aggregator));

    let resolve_aggregator = Arc::new(AggregatingResolver::new(acme_resolver, vec![cert_resolver]));
    let config = prepare_server_config(resolve_aggregator);

    Ok((runners, config))
}
