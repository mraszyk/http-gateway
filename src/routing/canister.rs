use std::{str::FromStr, sync::Arc};

use anyhow::{anyhow, Context, Error};
use candid::Principal;
use fqdn::{Fqdn, FQDN};

use crate::tls::cert::LooksupCustomDomain;

// Alias for a canister under all served domains.
// E.g. an alias 'nns' would resolve under both 'nns.ic0.app' and 'nns.icp0.io'
#[derive(Clone)]
pub struct CanisterAlias(FQDN, Principal);

impl FromStr for CanisterAlias {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        const INVALID_ALIAS_FORMAT: &str = "Invalid alias format, must be '<alias>:<canister_id>'";

        match value.split_once(':') {
            Some((alias, principal)) => {
                if alias.is_empty() {
                    return Err(anyhow!(INVALID_ALIAS_FORMAT));
                }

                Ok(Self(
                    FQDN::from_str(alias).context("unable to parse alias as FQDN")?,
                    Principal::from_str(principal)
                        .context("unable to parse canister id as Principal")?,
                ))
            }

            None => Err(anyhow!(INVALID_ALIAS_FORMAT)),
        }
    }
}

// Combination of canister id and whether we need to verify the response
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Canister {
    pub id: Principal,
    pub domain: FQDN,
    pub verify: bool,
}

// Resolves hostname to a canister id
pub trait ResolvesCanister: Send + Sync {
    fn resolve_canister(&self, host: &Fqdn) -> Option<Canister>;
}

pub struct CanisterResolver {
    domains: Vec<FQDN>,
    aliases: Vec<(FQDN, Canister)>,
    custom_domains: Arc<dyn LooksupCustomDomain>,
}

impl CanisterResolver {
    pub fn new(
        domains: Vec<FQDN>,
        aliases_in: Vec<CanisterAlias>,
        custom_domains: Arc<dyn LooksupCustomDomain>,
    ) -> Result<Self, Error> {
        let mut aliases = vec![];
        // Generate a list of all alias+domain combinations
        for a in aliases_in {
            for d in &domains {
                aliases.push((
                    FQDN::from_str(&format!("{}.{d}", a.0))?,
                    Canister {
                        id: a.1,
                        domain: d.clone(),
                        verify: true,
                    },
                ));
            }
        }

        Ok(Self {
            domains,
            aliases,
            custom_domains,
        })
    }

    // Iterate over aliases and see if given host is a subdomain of any.
    // Host is a subdomain of itself also so 'nns.ic0.app' will match the alias 'nns' and domain 'ic0.app'.
    // This will also match any subdomains of the alias - TODO discuss
    fn resolve_alias(&self, host: &Fqdn) -> Option<Canister> {
        self.aliases
            .iter()
            .find(|x| host.is_subdomain_of(&x.0))
            .map(|x| x.1.clone())
    }

    // Tries to resolve canister id from <id>.<domain> or <id>.raw.<domain> formatted hostname
    fn resolve_domain(&self, host: &Fqdn) -> Option<Canister> {
        let mut labels = host.labels();

        // Split by '--' if it has one and ignore the preceeding part
        let canister = labels.next()?.split("--").last()?;

        // Check if the first part of the hostname parses as Principal
        let id = Principal::from_text(canister).ok()?;

        // Check if the next part is "raw" then we don't need to verify the response
        let mut labels = labels.peekable();
        let verify = if labels.peek() == Some(&"raw") {
            // Consume "raw"
            labels.next();
            false
        } else {
            true
        };

        // Construct the remaining part of the domain
        let domain = FQDN::from_str(&labels.collect::<Vec<_>>().join(".")).ok()?;

        // Check if the domain is known
        if !self.domains.iter().any(|x| x == &domain) {
            return None;
        }

        Some(Canister { id, domain, verify })
    }
}

impl ResolvesCanister for CanisterResolver {
    fn resolve_canister(&self, host: &Fqdn) -> Option<Canister> {
        // Try to resolve canister using different sources
        self.resolve_alias(host)
            .or_else(|| self.resolve_domain(host))
            .or_else(|| {
                let id = self.custom_domains.lookup_custom_domain(host)?;
                Some(Canister {
                    id,
                    domain: host.to_owned(),
                    verify: true,
                })
            })
    }
}

#[cfg(test)]
mod test {
    use fqdn::fqdn;

    use super::*;
    use crate::tls::cert::storage::test::{create_test_storage, TEST_CANISTER_ID};

    #[test]
    fn test_canister_alias() -> Result<(), Error> {
        // Bad principal
        let a = CanisterAlias::from_str("foo:bar");
        assert!(a.is_err());

        let a = CanisterAlias::from_str("foo:");
        assert!(a.is_err());

        // Bad alias
        let a = CanisterAlias::from_str(":aaaaa-aa");
        assert!(a.is_err());

        let a = CanisterAlias::from_str("|||:aaaaa-aa");
        assert!(a.is_err());

        // All is empty
        let a = CanisterAlias::from_str(":");
        assert!(a.is_err());

        // No delimiter
        let a = CanisterAlias::from_str("blah");
        assert!(a.is_err());

        // All is good
        let a = CanisterAlias::from_str("foo:aaaaa-aa");
        assert!(a.is_ok());

        Ok(())
    }

    #[test]
    fn test_resolver() -> Result<(), Error> {
        let aliases = [
            "personhood:g3wsl-eqaaa-aaaan-aaaaa-cai",
            "identity:rdmx6-jaaaa-aaaaa-aaadq-cai",
            "nns:qoctq-giaaa-aaaaa-aaaea-cai",
        ]
        .into_iter()
        .map(|x| CanisterAlias::from_str(x).unwrap())
        .collect::<Vec<_>>();

        let domains = vec![fqdn!("ic0.app"), fqdn!("icp0.io"), fqdn!("foo")];
        let storage = create_test_storage();

        let resolver = CanisterResolver::new(
            domains.clone(),
            aliases.clone(),
            Arc::new(storage) as Arc<dyn LooksupCustomDomain>,
        )?;

        // Check aliases
        for d in &domains {
            // Ensure all aliases resolve with all domains
            for a in &aliases {
                let canister = resolver.resolve_alias(&fqdn!(&format!("{}.{d}", a.0)));

                assert_eq!(
                    canister,
                    Some(Canister {
                        id: a.1,
                        domain: d.clone(),
                        verify: true
                    })
                );
            }

            // Ensure that non-existant aliases do not resolve
            assert_eq!(
                resolver.resolve_alias(&FQDN::from_str(&format!("foo.{d}"))?),
                None
            );

            assert_eq!(
                resolver.resolve_alias(&FQDN::from_str(&format!("bar.{d}"))?),
                None
            );
        }

        // Check domains
        let id = Principal::from_text("aaaaa-aa").unwrap();

        // No canister ID
        assert_eq!(resolver.resolve_domain(&fqdn!("ic0.app")), None);
        assert_eq!(resolver.resolve_domain(&fqdn!("raw.ic0.app")), None);

        // Normal
        assert_eq!(
            resolver.resolve_domain(&fqdn!("aaaaa-aa.ic0.app")),
            Some(Canister {
                id,
                domain: fqdn!("ic0.app"),
                verify: true
            })
        );
        assert_eq!(
            resolver.resolve_domain(&fqdn!("aaaaa-aa.icp0.io")),
            Some(Canister {
                id,
                domain: fqdn!("icp0.io"),
                verify: true
            })
        );

        // Raw
        assert_eq!(
            resolver.resolve_domain(&fqdn!("aaaaa-aa.raw.ic0.app")),
            Some(Canister {
                id,
                domain: fqdn!("ic0.app"),
                verify: false
            })
        );
        assert_eq!(
            resolver.resolve_domain(&fqdn!("aaaaa-aa.raw.icp0.io")),
            Some(Canister {
                id,
                domain: fqdn!("icp0.io"),
                verify: false
            })
        );

        // foo-- <canister_id>
        assert_eq!(
            resolver.resolve_domain(&fqdn!("foo--aaaaa-aa.ic0.app")),
            Some(Canister {
                id,
                domain: fqdn!("ic0.app"),
                verify: true
            })
        );

        assert_eq!(
            resolver.resolve_domain(&fqdn!("foo--bar--aaaaa-aa.ic0.app")),
            Some(Canister {
                id,
                domain: fqdn!("ic0.app"),
                verify: true
            })
        );

        // Nested subdomain should not match (?)
        assert_eq!(
            resolver.resolve_domain(&fqdn!("aaaaa-aa.foo.ic0.app")),
            None
        );
        assert_eq!(
            resolver.resolve_domain(&fqdn!("aaaaa-aa.foo.icp0.io")),
            None
        );

        // Check the trait
        // Resolve from alias
        assert_eq!(
            resolver.resolve_canister(&fqdn!("nns.ic0.app")),
            Some(Canister {
                id: Principal::from_text("qoctq-giaaa-aaaaa-aaaea-cai").unwrap(),
                domain: fqdn!("ic0.app"),
                verify: true
            })
        );

        // Resolve from hostname
        assert_eq!(
            resolver.resolve_canister(&fqdn!("aaaaa-aa.ic0.app")),
            Some(Canister {
                id,
                domain: fqdn!("ic0.app"),
                verify: true
            })
        );

        assert_eq!(
            resolver.resolve_canister(&fqdn!("aaaaa-aa.raw.ic0.app")),
            Some(Canister {
                id,
                domain: fqdn!("ic0.app"),
                verify: false
            })
        );

        // Resolve custom domain
        assert_eq!(
            resolver.resolve_canister(&fqdn!("foo.baz")),
            Some(Canister {
                id: Principal::from_text(TEST_CANISTER_ID).unwrap(),
                domain: fqdn!("foo.baz"),
                verify: true,
            })
        );

        // Something that's not there
        assert_eq!(resolver.resolve_canister(&fqdn!("blah.blah")), None);

        Ok(())
    }
}
