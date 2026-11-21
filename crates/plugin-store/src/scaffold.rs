use std::path::PathBuf;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use steward_plugin_registry::{
    PluginCommand, PluginManifest, Trigger, TriggerType, MANIFEST_FILE, SCHEMA_VERSION,
};

use crate::unique_suffix;

#[derive(Debug, Clone)]
pub struct ScaffoldOptions {
    pub id: String,
    pub name: String,
    pub version: String,
    pub command: Option<String>,
    pub directory: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScaffoldReport {
    pub plugin_id: String,
    pub directory: PathBuf,
    pub files: Vec<PathBuf>,
}

pub fn scaffold_plugin(options: ScaffoldOptions) -> Result<ScaffoldReport> {
    if options.directory.exists() {
        anyhow::bail!(
            "scaffold target already exists: {}",
            options.directory.display()
        );
    }
    let command = options
        .command
        .clone()
        .unwrap_or_else(|| options.id.rsplit('.').next().unwrap_or("run").to_owned());
    let manifest = PluginManifest {
        schema_version: SCHEMA_VERSION,
        id: options.id.clone(),
        name: options.name.clone(),
        version: options.version.clone(),
        icon: None,
        package: steward_plugin_registry::PACKAGE_FILE.into(),
        commands: vec![PluginCommand {
            name: command.clone(),
            title: options.name.clone(),
            keywords: vec![command.clone()],
            trigger: Trigger {
                kind: TriggerType::Command,
                value: None,
            },
        }],
    };
    manifest
        .validate()
        .map_err(|error| anyhow::anyhow!("plugin metadata: {error}"))?;
    let parent = options
        .directory
        .parent()
        .context("scaffold target must have a parent directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let stage = parent.join(format!(
        ".{}-scaffold-{}",
        options
            .directory
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("plugin"),
        unique_suffix()
    ));
    std::fs::create_dir(&stage).with_context(|| format!("create {}", stage.display()))?;

    let nsh = serde_json::json!({
        "name": options.name,
        "id": options.id,
        "version": options.version,
        "build": 1,
        "capabilities": {}
    });
    let package_slug = options
        .id
        .rsplit('.')
        .next()
        .unwrap_or("plugin")
        .to_ascii_lowercase();
    let package_json = serde_json::json!({
        "name": format!("@steward/plugin-{package_slug}"),
        "version": options.version,
        "private": true,
        "description": format!("{} Steward NSH plugin", options.name),
        "main": "App.tsx",
        "scripts": {
            "build": "nsh build App.tsx --release -o dist",
            "typecheck": "nsh check App.tsx",
            "test": "nsh test ."
        }
    });
    let files = [
        (
            MANIFEST_FILE,
            serde_json::to_vec_pretty(&manifest).context("serialize plugin.json")?,
        ),
        (
            "nsh.json",
            serde_json::to_vec_pretty(&nsh).context("serialize nsh.json")?,
        ),
        (
            "package.json",
            serde_json::to_vec_pretty(&package_json).context("serialize package.json")?,
        ),
        ("tsconfig.json", TSCONFIG.as_bytes().to_vec()),
        ("App.tsx", app_source(&options.name, &command).into_bytes()),
        ("README.md", readme(&options.id, &command).into_bytes()),
        (".gitignore", b".nsh/\ndist/\nnode_modules/\n".to_vec()),
    ];
    for (name, bytes) in &files {
        std::fs::write(stage.join(name), bytes)
            .with_context(|| format!("write {}", stage.join(name).display()))?;
    }
    if let Err(error) = std::fs::rename(&stage, &options.directory) {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(error)
            .with_context(|| format!("activate scaffold at {}", options.directory.display()));
    }
    Ok(ScaffoldReport {
        plugin_id: options.id,
        directory: options.directory.clone(),
        files: files
            .iter()
            .map(|(name, _)| options.directory.join(name))
            .collect(),
    })
}

fn app_source(name: &str, _command: &str) -> String {
    let name = serde_json::to_string(name).expect("serialize plugin name");
    APP_SOURCE.replace("__PLUGIN_NAME__", &name)
}

const APP_SOURCE: &str = r##"import { mount, signal, Text, View } from "nsh";

function App() {
  const clicks = signal(0);

  return (
    <View style={{ flex: 1, padding: 24, gap: 16, backgroundColor: "#202024" }}>
      <Text style={{ color: "#ffffff", fontSize: 28, fontWeight: "bold" }}>__PLUGIN_NAME__</Text>
      <Text style={{ color: "#79797c" }}>
        Your Steward plugin is ready. Edit App.tsx to start building.
      </Text>
      <View
        role="button"
        onPress={() => clicks.update((count) => count + 1)}
        style={{ alignSelf: "flex-start", backgroundColor: "#863bff", borderRadius: 8, padding: 12 }}
      >
        <Text style={{ color: "#ffffff" }}>Clicks: {clicks()}</Text>
      </View>
    </View>
  );
}

mount(App);
"##;

fn readme(plugin_id: &str, command: &str) -> String {
    format!(
        r#"# {plugin_id}

Generated Steward NSH plugin scaffold.

```powershell
nsh check App.tsx
nsh test .
nsh build App.tsx --release -o dist
```

Type `{command}` in Steward after signing, packaging, and installing the
plugin.
"#
    )
}

const TSCONFIG: &str = r#"{
  "extends": "./.nsh/tsconfig.json",
  "include": ["App.tsx"]
}
"#;
