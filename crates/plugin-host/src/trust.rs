use std::path::PathBuf;

use anyhow::Result;
use nsh_engine::{verify_package, LoadedPackage};
use nsh_package::{PackageReceipt, PublicKey, TrustStore};

pub(crate) struct PackageTrust {
    store: TrustStore,
    roots: Vec<String>,
}

impl PackageTrust {
    pub(crate) fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            store: TrustStore::open(dir)
                .map_err(|error| anyhow::anyhow!("open NSH trust store: {error}"))?,
            roots: Vec::new(),
        })
    }

    pub(crate) fn add_root_base64(&mut self, key: &str) -> Result<()> {
        let key = key.trim();
        let public_key = PublicKey::from_base64(key)
            .map_err(|error| anyhow::anyhow!("invalid NSH trust root: {error}"))?;
        self.store.add_root(public_key);
        if !self.roots.iter().any(|root| root == key) {
            self.roots.push(key.to_owned());
        }
        Ok(())
    }

    pub(crate) fn roots(&self) -> &[String] {
        &self.roots
    }

    pub(crate) fn has_roots(&self) -> bool {
        !self.roots.is_empty()
    }

    pub(crate) fn accept_receipt(&mut self, receipt: &PackageReceipt) -> Result<(), String> {
        self.store
            .accept_receipt(receipt)
            .map_err(|error| error.to_string())
    }
}

pub(crate) fn verify_with_roots(
    roots: &[String],
    bytes: &[u8],
    data_dir: &std::path::Path,
) -> Result<LoadedPackage, String> {
    let mut store = TrustStore::open(data_dir.join("nsh-trust"))
        .map_err(|error| format!("open NSH trust store: {error}"))?;
    for root in roots {
        let key = PublicKey::from_base64(root).map_err(|error| error.to_string())?;
        store.add_root(key);
    }
    verify_package(&store, bytes, &data_dir.join("nsh-assets"))
}
