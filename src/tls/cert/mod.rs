pub mod providers;
pub mod storage;

use std::{
    net::{Ipv4Addr, Ipv6Addr},
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Context, Error};
use async_trait::async_trait;
use candid::Principal;
use futures::future::join_all;
use rustls::{crypto::aws_lc_rs, sign::CertifiedKey};
use tokio::select;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use x509_parser::prelude::*;

use crate::core::Run;
use providers::ProvidesCertificates;
use storage::StorageKey;

#[derive(Clone, Debug)]
pub struct CustomDomain {
    name: String,
    canister_id: Principal,
}

// Generic certificate and a list of its SANs
#[derive(Clone, Debug)]
pub struct Cert<T: Clone> {
    san: Vec<String>,
    cert: T,
    pub custom: Option<CustomDomain>,
}

// Commonly used concrete type of the above for Rustls
pub type CertKey = Cert<Arc<CertifiedKey>>;

// Looks up custom domain canister id by hostname
pub trait LookupCanister: Sync + Send {
    fn lookup_canister(&self, hostname: &str) -> Option<Principal>;
}

// Extracts a list of SubjectAlternativeName from a single certificate, formatted as strings.
// Skips everything except DNSName and IPAddress
fn extract_san_from_der(cert: &[u8]) -> Result<Vec<String>, Error> {
    let cert = X509Certificate::from_der(cert)
        .context("Unable to parse DER-encoded certificate")?
        .1;

    for ext in cert.extensions() {
        if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
            let mut names = vec![];
            for name in &san.general_names {
                let name = match name {
                    GeneralName::DNSName(v) => (*v).to_string(),
                    GeneralName::IPAddress(v) => match v.len() {
                        4 => {
                            let b: [u8; 4] = (*v).try_into().unwrap(); // We already checked that it's 4
                            let ip = Ipv4Addr::from(b);
                            ip.to_string()
                        }

                        16 => {
                            let b: [u8; 16] = (*v).try_into().unwrap(); // We already checked that it's 16
                            let ip = Ipv6Addr::from(b);
                            ip.to_string()
                        }

                        _ => return Err(anyhow!("Invalid IP address length {}", v.len())),
                    },

                    _ => continue,
                };

                names.push(name);
            }

            if names.is_empty() {
                return Err(anyhow!(
                    "No supported names found in SubjectAlternativeName extension"
                ));
            }

            return Ok(names);
        }
    }

    Err(anyhow!("SubjectAlternativeName extension not found"))
}

// Converts raw PEM certificate chain & private key to a CertifiedKey ready to be consumed by Rustls
pub fn pem_convert_to_rustls(key: &[u8], certs: &[u8]) -> Result<CertKey, Error> {
    let (key, certs) = (key.to_vec(), certs.to_vec());

    let key = rustls_pemfile::private_key(&mut key.as_ref())?
        .ok_or_else(|| anyhow!("No private key found"))?;

    let certs = rustls_pemfile::certs(&mut certs.as_ref()).collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err(anyhow!("No certificates found"));
    }

    // Extract a list of SANs from the 1st certificate in the chain
    let san = extract_san_from_der(certs[0].as_ref())?;

    // Parse key
    let key = aws_lc_rs::sign::any_supported_type(&key)?;

    Ok(Cert {
        san,
        cert: Arc::new(CertifiedKey::new(certs, key)),
        custom: None,
    })
}

// Collects certificates from providers and stores them in a given storage
pub struct Aggregator {
    providers: Vec<Arc<dyn ProvidesCertificates>>,
    storage: Arc<StorageKey>,
}

impl Aggregator {
    pub fn new(providers: Vec<Arc<dyn ProvidesCertificates>>, storage: Arc<StorageKey>) -> Self {
        Self { providers, storage }
    }

    // Fetches certificates concurrently from all providers
    async fn fetch(&self) -> Result<Vec<CertKey>, Error> {
        let certs = join_all(
            self.providers
                .iter()
                .map(|x| async { x.get_certificates().await }),
        )
        .await;

        // Flatten them into a single vector
        let certs = certs
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        Ok(certs)
    }
}

#[async_trait]
impl Run for Aggregator {
    async fn run(&self, token: CancellationToken) -> Result<(), Error> {
        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            select! {
                () = token.cancelled() => {
                    warn!("Aggregator exiting");
                    return Ok(());
                },

                _ = interval.tick() => {
                    let certs = match self.fetch().await {
                        Err(e) => {
                            warn!("Unable to fetch certificates: {e}");
                            continue;
                        }
                        Ok(v) => v,
                    };

                    info!("Aggregator: {} certs fetched", certs.len());
                    for v in &certs {
                        debug!("Aggregator: cert loaded: {:?}", v.san);
                    }

                    if let Err(e) = self.storage.store(certs) {
                        warn!("Error storing certificates: {e}");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
pub mod test {
    use super::*;

    // Some snakeoil certs

    pub const CERT_1: &[u8] = b"-----BEGIN CERTIFICATE-----\n\
    MIIC6TCCAdGgAwIBAgIUK60AjMl8YTJ5nWViMweY043y6/EwDQYJKoZIhvcNAQEL\n\
    BQAwDzENMAsGA1UEAwwEbm92ZzAeFw0yMzAxMDkyMTM5NTZaFw0zMzAxMDYyMTM5\n\
    NTZaMA8xDTALBgNVBAMMBG5vdmcwggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEK\n\
    AoIBAQCd/7NXWeENaITmYU+eWMJEJMZa6v74g70RpZlprQzx148U0QOKEw/r6mmd\n\
    SlbN4wsbb9lUu3zmXXpvYDAHYuOTYsDWcuNJXP/gCnPrD2wU8lJt3C5blmeU/9+0\n\
    U6/ppRmu6kf/jmm7CMBnowI0+kdvTF7sbpiUBXTDujXNsqtX0FaksILc9ZAqpUCC\n\
    2gqRcOXahzT2vnvJ2N+2bhveG+eB0/5oZcKgx0D4QgjR9k1+thWOQZUCJMg32OYS\n\
    k4e57WhOQxu9Kh5N2MU1Ff3fhCYXzg7/GhJtWyDmjt1vNBwGW9Zn0BicySdcVFPC\n\
    mRW3/rZrSpnwvsEnpIuyKGq+NMSXAgMBAAGjPTA7MAkGA1UdEwQCMAAwDwYDVR0R\n\
    BAgwBoIEbm92ZzAdBgNVHQ4EFgQUYHN6l0ihbfbLQXqnKPltmv9DWDkwDQYJKoZI\n\
    hvcNAQELBQADggEBAFBvyns/lJZ+zB4/Tmx3YUryji20XUNwhtlBC6V7rdWCXneY\n\
    kqKVgbyDZ+XAYX2eL3o1gcv+XJxQgHfL+OqHJCVbK2kkYVSCW38WNVZb+oeTp/w3\n\
    pgtmg91JcCjFEw2doqImLZLQDX6KK1gDGdTQ2dtisFcxGEkMUyjzqmZmZNzl+u7d\n\
    JeDygLfGrMleO7ij2hP2vEfgkGbbvM+JCTav0B91Rj8/CbJHBwr8/CW4BJTjsqZC\n\
    mglNb9+hY8N6XAxntoqZsFzuDyDx7ZSxeAW0yVRemrIPSgcPwpLDBFm4dCSwUHJN\n\
    ujBjp7DRCQgg8uUq+0FMQ63ioZoR5mXQ5hzmTqk=\n\
    -----END CERTIFICATE-----\n\
    ";

    pub const KEY_1: &[u8] = b"-----BEGIN PRIVATE KEY-----\n\
    MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCd/7NXWeENaITm\n\
    YU+eWMJEJMZa6v74g70RpZlprQzx148U0QOKEw/r6mmdSlbN4wsbb9lUu3zmXXpv\n\
    YDAHYuOTYsDWcuNJXP/gCnPrD2wU8lJt3C5blmeU/9+0U6/ppRmu6kf/jmm7CMBn\n\
    owI0+kdvTF7sbpiUBXTDujXNsqtX0FaksILc9ZAqpUCC2gqRcOXahzT2vnvJ2N+2\n\
    bhveG+eB0/5oZcKgx0D4QgjR9k1+thWOQZUCJMg32OYSk4e57WhOQxu9Kh5N2MU1\n\
    Ff3fhCYXzg7/GhJtWyDmjt1vNBwGW9Zn0BicySdcVFPCmRW3/rZrSpnwvsEnpIuy\n\
    KGq+NMSXAgMBAAECggEAKYtxTFAxWZW4kF1ZEqFzH3juAT0WYyE8x1WcY8mhhDvy\n\
    fv5AqH8/qgBe2gGQlp2TL5k2881C184PohaQOnj5rykB3MGj2wgNrgsBlPberBlV\n\
    rFZ/iAyh2u93EpMIx+5mNPScjumTCp+P/BBERcrjmrPhp9ii3RUcMVUWzaoj3Lhc\n\
    wa5trC1r7UqbUZeO7NaVA7cGETZLVm8U7NaL8ccb1dKASUzrC9QCy9VVekJbb2S7\n\
    h38MELR9wvTGS7s4hXQGejb8vEDuXcZzWIFg3YMkJPIyGLAEaRynfeAHm/ji48U0\n\
    zh1ba3CWE/6z6nayDPqWqrwic4Hff6Mz+SIWAz2LyQKBgQDcdeWweNRVXhVkcFUP\n\
    JNpUiLOF5j3f4nqZwk7j5hQBxcXilYO/lmrcimvhvJ3ox97GfqCkvEQM8thTnPmi\n\
    JBagynOfIaUK2qdVwS1BbZ2JpYe3k/rO+iSKtRO4mF94cHgFIafPb5qt0fFz9bDS\n\
    7D2lnWSbveMvb+mZsp/+FZx2DwKBgQC3eBhAbOSrSGuh7KOuWsav8pROMdcsESpz\n\
    j8el1iEklRsklYiNrVsztlZtNUXE2zSHeNPsGENDGlvKG8qD/vbcdTFsYa1H8Hk5\n\
    NydTLAb0/Bm256Xee1Dm5Wt2yG2aLfc9eG0trJz8VgBDhDlulnjo2kavhWIpTBNm\n\
    0WmkMQsQ+QKBgQDYXd1PlUbPgcb9DEJu2nxs+r02bQHM+TnaLhm/EdAQ7UmJV7Q2\n\
    FCpMyI2YvsU78O1zYlPHWf5vtucZKLbXqxOKOye+xgZ04KPaRf1keXBj51GLmnBN\n\
    MrMqbw0r3l/UlI02fBF2RNJKRgHzDO6+E51tLUvQjkyqAewCLI1ZkVw9gQKBgD0F\n\
    J2O+E+vX4VxwnRvvOyfn0WWUdBFHAEyBJJDGgC1vniBzz3/3iV7QpTwbPMI1eeoY\n\
    yLs8cpqN2LuGtLtkAGzgWXjHn99OXrMl4eFqwkGW22KW9vbhIs44vZ47GSDvasy6\n\
    Ee3f/DJ81AegoY1jZIFln57fCP/dOpK20aD3YsvZAoGBAKgaWVYbROCRJ6C8CQGd\n\
    yetoZ8n25E7O5JtyKSNGwiQyD0IURgLuotiBpQvCCz9HGS53E6HLzBCc4jZc3GDq\n\
    qVDS5cIgcfWAOBalBQ+JxoHsnLRGXeBBKwvaJB+EzlrV8st1dCmM4gukElBJm/PZ\n\
    TvEPeiHG81OgB1RPgUt3DVIf\n\
    -----END PRIVATE KEY-----\n\
    ";

    pub const CERT_2: &[u8] = b"-----BEGIN CERTIFICATE-----\n\
    MIIC4jCCAcqgAwIBAgIUDAdBS7aRT7YfKgt/H2VQ1b8u80kwDQYJKoZIhvcNAQEL\n\
    BQAwFzEVMBMGA1UEAwwMMzY1ODE1M2YyN2UwMB4XDTI0MDMwNzIyNTMwOVoXDTM0\n\
    MDMwNTIyNTMwOVowFzEVMBMGA1UEAwwMMzY1ODE1M2YyN2UwMIIBIjANBgkqhkiG\n\
    9w0BAQEFAAOCAQ8AMIIBCgKCAQEAyITGTjnOLGCiW51EuDl5Us7YJk6gkLWeQ+A5\n\
    FQtUaVqjaLKHVZlNnuqFsQ7Y58GKOPzlO1nECfTgv6xUr0i8bhQhoB8GjWdKvhA6\n\
    zxPXOMCDIIW8JuYKCbG67ygVxBx5ER5fNq2GMmyMfmLoLfejPVqWyoV9e9RIY7Vi\n\
    wmiToXXI6vFETom3w7rMhKjJGXR+3/om7i531zmzOFY0jDS0lPMsaNwNQhL3GFfA\n\
    bXjNyBJLYakHsga8VDZcsM5uoS7Zf4ogpFiLczk5DlYvnSdCDhO2KVUe4XwY5oqJ\n\
    IPLL97/uL1tpB9v7D6EX6gGWBMjJpExnggeKDDjXSc16DOUT9wIDAQABoyYwJDAJ\n\
    BgNVHRMEAjAAMBcGA1UdEQQQMA6CDDM2NTgxNTNmMjdlMDANBgkqhkiG9w0BAQsF\n\
    AAOCAQEAPzgUej2SaXnR+0tCFygFALkU33DJMBFU/8JF8HYrm3pgaa4y+okVt6zq\n\
    y1wUCeFejlLB2/AlajPshLJzsmHy6HRH/VKpkL5WkcGSqiFiKr3K+FEpsXtgemiF\n\
    sJP7g0zi8qHPDDUHyHA5idDJzBt0E7UvFO9Dtx4IPkLm1rF7xSQiRl/SzNI9U4py\n\
    7DnY8dtqYhUa2gaYMkZ1Y2BTzzBy6hjl3PnDfCPzTlzMT63Jxj3jFgqO3TGtkj0F\n\
    mrym8qHmCWHHsBdqr0LuD1kzmHoW13PtLzKixzDfyaPsx53ChxJmw7w3K5paFpVU\n\
    PNTlQReyX3nOvb85CynvGgZ3/FQxnw==\n\
    -----END CERTIFICATE-----\n\
    ";

    pub const KEY_2: &[u8] = b"-----BEGIN PRIVATE KEY-----\n\
    MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDIhMZOOc4sYKJb\n\
    nUS4OXlSztgmTqCQtZ5D4DkVC1RpWqNosodVmU2e6oWxDtjnwYo4/OU7WcQJ9OC/\n\
    rFSvSLxuFCGgHwaNZ0q+EDrPE9c4wIMghbwm5goJsbrvKBXEHHkRHl82rYYybIx+\n\
    Yugt96M9WpbKhX171EhjtWLCaJOhdcjq8UROibfDusyEqMkZdH7f+ibuLnfXObM4\n\
    VjSMNLSU8yxo3A1CEvcYV8BteM3IEkthqQeyBrxUNlywzm6hLtl/iiCkWItzOTkO\n\
    Vi+dJ0IOE7YpVR7hfBjmiokg8sv3v+4vW2kH2/sPoRfqAZYEyMmkTGeCB4oMONdJ\n\
    zXoM5RP3AgMBAAECggEADB25vdBQXO4Z4V9HX7pZUl+dP/NQUG4o+gD6cgMVPqhz\n\
    Z0giVVHGFuwk1+YFxTs0luzxDP0Hk3JwgiRvmYfTmvMsdPhq9PBg28svQoP4ZT18\n\
    ruJl1BPiV2Od4AWUCx2NUzN6nVsu2K0mcByZ2u0zt+lZYzNdubXCCgRTy1t2UDMq\n\
    QYhpJAm+yE3TwaAucxV+7T3aD4S23RVcz4N1hnLu90EmPQ6TBHGFC4eproSd8TJ0\n\
    rj2caRPlSast/j1oBwyCfwX6VC/jQU7zv9RaVHK3Y0LN9rlfBCCjWzH1cCjvUpkH\n\
    q7fklHM+BzEB3pZzUAjB7aamDe3eR3xCrbO7QHUiwQKBgQD7W429aXLUQ60pXOpg\n\
    k/56lkW7K9g/SFZJs0lXpyVLNImRcu4NQOl/upm1ADaaI15PPO165UjMjm7N6Tfc\n\
    IZe6tXaGlRIyzURjz4T5f7oko75hJWCW4jCV/6N6e00Y8bldnWkoNdVUSWVOF79c\n\
    ouT4rMn5td9ZAELfqA1c8WhNywKBgQDMONlE7S12Ppd1rfQwdKgNa4d428Hlschl\n\
    lZUSCkRUjF1a8oP5mnf66ySf+QFEVYzRLFeQgTcPej4DDS/EYERF3bLNowroWDzo\n\
    +gbbjuC2oQMFyhMwwcYdSdsfD0FmxVs79tKvu0gsDB005uzEmXs4gQo0nNc9oUJe\n\
    bBE/fLDNBQKBgHXiKkd6/O+wDbYobYN95Qt5DpsJpRGIy28lNnB1Y3gx25LrY9mz\n\
    Z88PpKbOwsznaYOf/4BzqADHjA/mINyMpKxcDopvv2kz+68T1DlvPc2RPegxr2sU\n\
    CdVPX0xCJ5ZbR6Qv/vFszfAJvAkz+ftoKhq2bsM+GNGU3cgm+J1uWoyhAoGAP06w\n\
    K6nKmgk1MonGVO8U2XQn/tNA/E9sa/E+0OTV4c/RcMwVFV9JKkOSivTJ68EJch5o\n\
    1qb3xpiCeLexwxKEl5PuRcjxLK2N1DsNvSpBhtvK8BSAdnDbVWD7yFkWUSGE8sXE\n\
    8i0AZocq1qdvZlKd3BpEa6LjJnvC8zpU7nVc6XECgYEAner1t7zPWvu3L3YiddCZ\n\
    RZw1UnyRTs+OVmmDfWVkkWHpdEQWMHmtJvESp0l7mvOQKtWrco/FT4fOYHrDp0mz\n\
    /xbEEBoYlUOLQPLMqcdP056Qh5BLq8dw/yv9v2KdfVd/yfu97ekQULHQcMetlIed\n\
    v1tiHPlW4461iUonC6zsOVI=\n\
    -----END PRIVATE KEY-----\n\
    ";

    struct TestProvider(CertKey);

    #[async_trait]
    impl ProvidesCertificates for TestProvider {
        async fn get_certificates(&self) -> Result<Vec<CertKey>, Error> {
            Ok(vec![self.0.clone()])
        }
    }

    #[test]
    fn test_pem_convert_to_rustls() -> Result<(), Error> {
        let cert = pem_convert_to_rustls(KEY_1, CERT_1)?;
        assert_eq!(cert.san, vec!["novg"]);
        let cert = pem_convert_to_rustls(KEY_2, CERT_2)?;
        assert_eq!(cert.san, vec!["3658153f27e0"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_aggregator() -> Result<(), Error> {
        let prov1 = TestProvider(pem_convert_to_rustls(KEY_1, CERT_1)?);
        let prov2 = TestProvider(pem_convert_to_rustls(KEY_2, CERT_2)?);

        let storage = Arc::new(StorageKey::new());
        let aggregator = Aggregator::new(vec![Arc::new(prov1), Arc::new(prov2)], storage);
        let certs = aggregator.fetch().await?;

        assert_eq!(certs.len(), 2);
        assert_eq!(certs[0].san, vec!["novg"]);
        assert_eq!(certs[1].san, vec!["3658153f27e0"]);

        Ok(())
    }
}
