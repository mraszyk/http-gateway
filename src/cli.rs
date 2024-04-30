use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use clap::{Args, Parser};
use fqdn::FQDN;
use hickory_resolver::config::CLOUDFLARE_IPS;
use humantime::parse_duration;
use reqwest::Url;

use crate::{
    core::{AUTHOR_NAME, SERVICE_NAME},
    http::dns,
    routing::canister::CanisterAlias,
};

#[derive(Parser)]
#[clap(name = SERVICE_NAME)]
#[clap(author = AUTHOR_NAME)]
pub struct Cli {
    #[command(flatten, next_help_heading = "HTTP Client")]
    pub http_client: HttpClient,

    #[command(flatten, next_help_heading = "DNS Resolver")]
    pub dns: Dns,

    #[command(flatten, next_help_heading = "HTTP Server")]
    pub http_server: HttpServer,

    #[command(flatten, next_help_heading = "Certificates")]
    pub cert: Cert,

    #[command(flatten, next_help_heading = "Domains")]
    pub domain: Domain,

    #[command(flatten, next_help_heading = "Policy")]
    pub policy: Policy,

    #[command(flatten, next_help_heading = "Metrics")]
    pub metrics: Metrics,

    #[command(flatten, next_help_heading = "Misc")]
    pub misc: Misc,
}

// Clap does not support prefixes due to macro limitations
// so we have to add them manually (long = "...")
//
// Also 'id = ...' in some fields below is needed because clap requires unique field names
// https://github.com/clap-rs/clap/issues/4556

#[derive(Args)]
pub struct HttpClient {
    /// Timeout for HTTP connection phase
    #[clap(long = "http-client-timeout-connect", default_value = "5s", value_parser = parse_duration)]
    pub timeout_connect: Duration,

    /// Timeout for whole HTTP call
    #[clap(long = "http-client-timeout", default_value = "60s", value_parser = parse_duration)]
    pub timeout: Duration,

    /// TCP Keepalive interval
    #[clap(long = "http-client-tcp-keepalive", default_value = "15s", value_parser = parse_duration)]
    pub tcp_keepalive: Duration,

    /// HTTP2 Keepalive interval
    #[clap(long = "http-client-http2-keepalive", default_value = "10s", value_parser = parse_duration)]
    pub http2_keepalive: Duration,

    /// HTTP2 Keepalive timeout
    #[clap(long = "http-client-http2-keepalive-timeout", default_value = "5s", value_parser = parse_duration)]
    pub http2_keepalive_timeout: Duration,
}

#[derive(Args)]
pub struct Dns {
    /// List of DNS servers to use
    #[clap(long = "dns-servers", default_values_t = CLOUDFLARE_IPS)]
    pub servers: Vec<IpAddr>,

    /// DNS protocol to use (clear/tls/https)
    #[clap(long = "dns-protocol", default_value = "tls")]
    pub protocol: dns::Protocol,

    /// TLS name to expect for TLS and HTTPS protocols (e.g. "dns.google" or "cloudflare-dns.com")
    #[clap(long = "dns-tls-name", default_value = "cloudflare-dns.com")]
    pub tls_name: String,

    /// Cache size for the resolver (in number of DNS records)
    #[clap(long = "dns-cache-size", default_value = "2048")]
    pub cache_size: usize,
}

#[derive(Args)]
pub struct HttpServer {
    /// Where to listen for HTTP
    #[clap(long = "http-server-listen-plain", default_value = "[::1]:8080")]
    pub http: SocketAddr,

    /// Where to listen for HTTPS
    #[clap(long = "http-server-listen-tls", default_value = "[::1]:8443")]
    pub https: SocketAddr,

    /// Backlog of incoming connections to set on the listening socket.
    #[clap(long = "http-server-backlog", default_value = "2048")]
    pub backlog: u32,

    /// Maximum number of HTTP2 streams that the client is allowed to create in a single connection
    #[clap(long = "http-server-http2-max-streams", default_value = "128")]
    pub http2_max_streams: u32,

    /// Keepalive interval for HTTP2 connections
    #[clap(long = "http-server-http2-keepalive-interval", id = "HTTP_SERVER_HTTP2_KEEPALIVE_INTERVAL", default_value = "20s", value_parser = parse_duration)]
    pub http2_keepalive_interval: Duration,

    /// Keepalive timeout for HTTP2 connections
    #[clap(long = "http-server-http2-keepalive-timeout", id = "HTTP_SERVER_HTTP2_KEEPALIVE_TIMEOUT", default_value = "10s", value_parser = parse_duration)]
    pub http2_keepalive_timeout: Duration,

    /// How long to wait for the existing connections to finish before shutting down
    #[clap(long = "http-server-grace-period", default_value = "10s", value_parser = parse_duration)]
    pub grace_period: Duration,
}

#[derive(Args)]
pub struct Cert {
    /// Read certificates from given directories, each certificate should be a pair .pem + .key files with the same base name
    #[clap(long = "cert-provider-dir")]
    pub dir: Vec<PathBuf>,

    /// Request certificates from the 'certificate-issuer' instances reachable over given URLs.
    /// Also proxies the `/registrations` path to those issuers.
    #[clap(long = "cert-provider-issuer-url")]
    pub issuer_urls: Vec<Url>,

    /// How frequently to poll providers for certificates
    #[clap(long = "cert-poll-interval", default_value = "10s", value_parser = parse_duration)]
    pub poll_interval: Duration,
}

#[derive(Args)]
pub struct Domain {
    /// List of domains that we serve system subnets from
    #[clap(long = "domain-system")]
    pub domains_system: Vec<FQDN>,

    /// List of domains that we serve app subnets from
    #[clap(long = "domain-app")]
    pub domains_app: Vec<FQDN>,

    /// List of canister aliases in format '<alias>:<canister_id>'
    #[clap(long = "domain-alias")]
    pub canister_aliases: Vec<CanisterAlias>,
}

#[derive(Args)]
pub struct Policy {
    /// Path to a list of pre-isolation canisters, one canister per line
    #[clap(long = "policy-pre-isolation-canisters")]
    pub pre_isolation_canisters: Option<PathBuf>,

    /// Denylist URL
    #[clap(long = "policy-denylist-url")]
    pub denylist_url: Option<Url>,

    /// Path to a list of whitelisted canisters
    #[clap(long = "policy-denylist-allowlist")]
    pub denylist_allowlist: Option<PathBuf>,

    /// Path to a local denylist cache for initial seeding
    #[clap(long = "policy-denylist-seed")]
    pub denylist_seed: Option<PathBuf>,

    /// How frequently to poll denlylist for updates
    #[clap(long = "policy-denylist-poll-interval", default_value = "1m", value_parser = parse_duration)]
    pub denylist_poll_interval: Duration,
}

#[derive(Args)]
pub struct Metrics {
    /// Where to listen for Prometheus metrics scraping
    #[clap(long = "metrics-listen")]
    pub listen: Option<SocketAddr>,
}

#[derive(Args)]
pub struct Misc {
    /// Path to a GeoIP database
    #[clap(long = "geoip-db")]
    pub geoip_db: Option<PathBuf>,
}
