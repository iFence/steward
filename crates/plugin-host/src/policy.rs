use nsh_engine::{Capabilities, LoadedPackage};

/// Applies Steward's release capability ceiling to a verified package.
///
/// NSH intersects this grant with the capabilities declared by the signed
/// package, so a package cannot grant itself anything it did not declare.
pub(crate) fn apply_release_policy(package: &mut LoadedPackage) {
    let mut granted = Capabilities::default();
    granted.clipboard = true;
    granted.fs.read = vec!["${dataDir}".into()];
    granted.fs.write = vec!["${dataDir}".into()];
    granted.host = vec!["steward.*".into()];
    let mut open_url = nsh_core::access::OpenUrlGrant::default();
    open_url.schemes = vec!["https".into(), "mailto".into()];
    granted.open_url = Some(open_url);
    package.bundle.policy.granted = Some(granted);
}
