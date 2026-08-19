fn main() {
    #[cfg(target_os = "windows")]
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_required()
        .expect("failed to embed Steward resources (app.rc)");
}
