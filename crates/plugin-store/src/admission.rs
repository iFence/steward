use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use serde::{Deserialize, Serialize};
use steward_plugin_contract::HOST_API_VERSION;

use crate::PackageInspection;

/// Version of Steward's automated public-marketplace admission policy.
pub const PUBLIC_ADMISSION_POLICY_VERSION: u16 = 1;

/// One automated public-marketplace admission decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionReview {
    pub policy_version: u16,
    pub accepted: bool,
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub build: u64,
    pub publisher_key_id: Option<String>,
    pub required_host_api: Option<String>,
    pub host_api_version: String,
    pub denied_capabilities: Vec<String>,
    pub capability_groups: Vec<AdmissionCapabilityGroup>,
    pub checks: Vec<AdmissionCheck>,
}

/// Capability declarations grouped for policy-review display.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionCapabilityGroup {
    pub permission: String,
    pub sensitive: bool,
    pub scopes: Vec<String>,
}

/// One stable, machine-readable admission check result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionCheck {
    pub code: String,
    pub passed: bool,
    pub detail: String,
}

pub(crate) fn review(inspection: &PackageInspection) -> AdmissionReview {
    let current_host = Version::parse(HOST_API_VERSION).expect("host API version must be semver");
    let required_host = inspection
        .min_host_version
        .as_deref()
        .and_then(|version| Version::parse(version).ok());
    let wildcard_permissions = inspection
        .declared_capabilities
        .iter()
        .filter(|capability| capability.legacy_wildcard)
        .map(|capability| capability.permission.clone())
        .collect::<BTreeSet<_>>();
    let malformed_declarations = inspection
        .declared_capabilities
        .iter()
        .filter(|capability| {
            capability.permission.trim().is_empty() || capability.scope.trim().is_empty()
        })
        .map(|capability| capability.permission.clone())
        .collect::<BTreeSet<_>>();
    let mut capability_findings = inspection.policy.warnings.clone();
    capability_findings.extend(
        wildcard_permissions.iter().map(|permission| {
            format!("public admission rejects wildcard permission '{permission}'")
        }),
    );
    capability_findings.extend(
        malformed_declarations.iter().map(|permission| {
            format!("public admission rejects malformed permission '{permission}'")
        }),
    );
    capability_findings.sort();
    capability_findings.dedup();

    let public_contract = !inspection.legacy_compatibility
        && required_host.is_some_and(|required| required <= current_host);
    let checks = vec![
        AdmissionCheck {
            code: "signature".into(),
            passed: inspection.signature_valid,
            detail: inspection
                .signature_error
                .clone()
                .unwrap_or_else(|| "package signature is valid".into()),
        },
        AdmissionCheck {
            code: "trust_chain".into(),
            passed: inspection.trusted,
            detail: inspection
                .trust_error
                .clone()
                .unwrap_or_else(|| "package is trusted by a configured NSH root".into()),
        },
        AdmissionCheck {
            code: "publisher_identity".into(),
            passed: inspection.trusted
                && inspection
                    .publisher_key_id
                    .as_deref()
                    .is_some_and(|publisher| !publisher.is_empty()),
            detail: if inspection.trusted {
                inspection
                    .publisher_key_id
                    .clone()
                    .unwrap_or_else(|| "trusted publisher identity is missing".into())
            } else {
                "no trusted publisher identity is available".into()
            },
        },
        AdmissionCheck {
            code: "release_policy".into(),
            passed: inspection.policy.passed,
            detail: if inspection.policy.errors.is_empty() {
                "package passes Steward's release policy".into()
            } else {
                inspection.policy.errors.join("; ")
            },
        },
        AdmissionCheck {
            code: "capability_scope".into(),
            passed: capability_findings.is_empty(),
            detail: if capability_findings.is_empty() {
                "all declared permissions are named and inside the release profile".into()
            } else {
                capability_findings.join("; ")
            },
        },
        AdmissionCheck {
            code: "public_contract".into(),
            passed: public_contract,
            detail: if public_contract {
                format!(
                    "plugin requires host API {}; running host API is {HOST_API_VERSION}",
                    inspection.min_host_version.as_deref().unwrap_or("")
                )
            } else if inspection.legacy_compatibility {
                "legacy official compatibility is not accepted by public admission".into()
            } else {
                format!(
                    "minHostVersion '{}' is missing, invalid, or newer than {HOST_API_VERSION}",
                    inspection.min_host_version.as_deref().unwrap_or("")
                )
            },
        },
    ];
    let accepted = checks.iter().all(|check| check.passed);

    AdmissionReview {
        policy_version: PUBLIC_ADMISSION_POLICY_VERSION,
        accepted,
        plugin_id: inspection.app_id.clone(),
        name: inspection.name.clone(),
        version: inspection.version.clone(),
        build: inspection.build,
        publisher_key_id: inspection.publisher_key_id.clone(),
        required_host_api: inspection.min_host_version.clone(),
        host_api_version: HOST_API_VERSION.into(),
        denied_capabilities: inspection.policy.denied_capabilities.clone(),
        capability_groups: capability_groups(inspection),
        checks,
    }
}

fn capability_groups(inspection: &PackageInspection) -> Vec<AdmissionCapabilityGroup> {
    let mut groups = BTreeMap::<String, (bool, BTreeSet<String>)>::new();
    for capability in &inspection.declared_capabilities {
        let group = groups
            .entry(capability.permission.clone())
            .or_insert_with(|| (false, BTreeSet::new()));
        group.0 |= capability.sensitive;
        group.1.insert(capability.scope.clone());
    }
    groups
        .into_iter()
        .map(
            |(permission, (sensitive, scopes))| AdmissionCapabilityGroup {
                permission,
                sensitive,
                scopes: scopes.into_iter().collect(),
            },
        )
        .collect()
}
