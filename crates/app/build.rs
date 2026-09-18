//! Embeds the Windows resources the shipped executable needs.
//!
//! Two files, compiled in order:
//!
//! - `app.rc` — the icons (`assets/icon.ico`), which Explorer, the taskbar, the
//!   tray and the MSI all read.
//! - `version.rc` — the `VS_VERSION_INFO` block, so the binary reports its
//!   product name and version instead of showing up as a naked file name.

fn main() {
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=app.rc");
        println!("cargo:rerun-if-changed=version.rc");
        println!("cargo:rerun-if-changed=../../assets/icon.ico");
        embed_resource::compile("app.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed the icon resources (app.rc)");
        embed_resource::compile("version.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed the version resource (version.rc)");
    }
}
