use std::path::PathBuf;

use crate::tls::cert::{pem_convert_to_rustls, providers::ProvidesCertificates, CertKey};
use anyhow::{Context, Error};
use async_trait::async_trait;
use tokio::fs::read_dir;
use tracing::info;

// It searches for .pem files in the given directory and tries to find the
// corresponding .key files with the same base name.
// After that it loads & parses each pair.
#[derive(derive_new::new)]
pub struct Provider {
    path: PathBuf,
}

#[async_trait]
impl ProvidesCertificates for Provider {
    async fn get_certificates(&self) -> Result<Vec<CertKey>, Error> {
        let mut files = read_dir(&self.path).await?;

        let mut certs = vec![];
        while let Some(v) = files.next_entry().await? {
            // Skip non-file entries
            if !v.file_type().await?.is_file() {
                continue;
            }

            // Skip non-pem files
            if !v
                .path()
                .extension()
                .map_or(false, |x| x.eq_ignore_ascii_case("pem"))
            {
                continue;
            }

            // Guess key file name
            let path = v.path();
            let base = path.file_stem().unwrap().to_string_lossy();
            let keyfile = self.path.join(format!("{base}.key"));

            // Load key & cert
            let chain = tokio::fs::read(v.path()).await?;
            let key = tokio::fs::read(&keyfile).await.context(format!(
                "Corresponding key file '{}' for '{}' could not be read",
                keyfile.to_string_lossy(),
                v.path().to_string_lossy()
            ))?;

            let cert = pem_convert_to_rustls(&key, &chain)
                .context("unable to parse certificate/key pair")?;

            certs.push(cert);
        }

        info!(
            "Dir provider ({}): {} certs loaded",
            self.path.to_string_lossy(),
            certs.len()
        );

        Ok(certs)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::tls::cert::test::{CERT_1, KEY_1};

    #[tokio::test]
    async fn test() -> Result<(), Error> {
        let dir = tempfile::tempdir()?;

        let keyfile = dir.path().join("foobar.key");
        std::fs::write(keyfile, KEY_1)?;

        let certfile = dir.path().join("foobar.pem");
        std::fs::write(certfile, CERT_1)?;

        // Some junk to be ignored
        std::fs::write(dir.path().join("foobar.baz"), b"foobar")?;

        let prov = Provider::new(dir.path().to_path_buf());
        let certs = prov.get_certificates().await?;

        assert_eq!(certs.len(), 1);
        assert_eq!(certs[0].san, vec!["novg"]);

        Ok(())
    }
}
