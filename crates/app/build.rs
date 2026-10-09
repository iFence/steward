//! Embeds the Windows resources the shipped executable needs.
//!
//! Two files, compiled in order:
//!
//! - `app.rc` — the icons (`assets/icon.ico`), which Explorer, the taskbar, the
//!   tray and the MSI all read.
//! - `version.rc` — the `VS_VERSION_INFO` *template*, so the binary reports its
//!   product name and version instead of showing up as a naked file name. Its
//!   `@VERSION@` / `@VERSION_COMMA@` placeholders are filled in here from the
//!   package version, and the filled copy is compiled from `OUT_DIR`: the
//!   version has one source of truth (`Cargo.toml`), so a shipped build cannot
//!   keep reporting the version it was first written with.

#[cfg(target_os = "windows")]
fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=version.rc");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_required()
        .expect("failed to embed the icon resources (app.rc)");
    let version_rc = render_version_rc();
    embed_resource::compile(&version_rc, embed_resource::NONE)
        .manifest_required()
        .expect("failed to embed the version resource (version.rc)");
}

/// Fill `version.rc` in from the package version and return the path of the
/// generated file.
///
/// `CARGO_PKG_VERSION` is the package version Cargo resolved for this crate
/// (`version.workspace = true` in `Cargo.toml`), e.g. `0.2.0`.
#[cfg(target_os = "windows")]
fn render_version_rc() -> std::path::PathBuf {
    let version = env!("CARGO_PKG_VERSION");
    let template =
        std::fs::read_to_string("version.rc").expect("failed to read the version.rc template");
    let rendered = template
        .replace("@VERSION_COMMA@", &version_fields(version))
        .replace("@VERSION@", version);
    let path =
        std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR is set")).join("version.rc");
    std::fs::write(&path, rendered).expect("failed to write the generated version.rc");
    path
}

/// The `FILEVERSION` / `PRODUCTVERSION` fields, as the resource compiler wants
/// them: four comma-separated 16-bit numbers, e.g. `0,2,0,0`.
///
/// A pre-release or build suffix describes the same product version as far as
/// the version resource is concerned, so `0.2.0-rc.1` reports `0,2,0,0`; a part
/// that is not a number at all counts as zero rather than failing the build.
#[cfg(target_os = "windows")]
fn version_fields(version: &str) -> String {
    let release = version.split(['-', '+']).next().unwrap_or("");
    let mut parts = release
        .split('.')
        .map(|part| {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u16>().unwrap_or(0)
        })
        .collect::<Vec<_>>();
    parts.resize(4, 0);
    parts
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Other platforms have no Win32 resources to embed.
#[cfg(not(target_os = "windows"))]
fn main() {}
