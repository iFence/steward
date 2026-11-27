use std::path::PathBuf;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use steward_plugin_contract::HOST_API_VERSION;
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
    pub sdk_dependency: Option<String>,
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
    let sdk_dependency = options
        .sdk_dependency
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(default_sdk_dependency);
    if sdk_dependency.starts_with("workspace:")
        || sdk_dependency.contains("/src/host.ts")
        || sdk_dependency.contains("\\src\\host.ts")
    {
        anyhow::bail!(
            "SDK dependency must name a released package, not workspace or raw source: {sdk_dependency}"
        );
    }
    let manifest = PluginManifest {
        schema_version: SCHEMA_VERSION,
        id: options.id.clone(),
        name: options.name.clone(),
        author: None,
        version: options.version.clone(),
        min_host_version: Some(HOST_API_VERSION.into()),
        icon: None,
        package: steward_plugin_registry::PACKAGE_FILE.into(),
        preferences: Default::default(),
        commands: vec![PluginCommand {
            name: command.clone(),
            title: options.name.clone(),
            keywords: vec![command.clone()],
            trigger: Trigger {
                kind: TriggerType::Command,
                value: None,
            },
            presentation: None,
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
        "capabilities": {
            "host": ["steward.feedback.toast"]
        }
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
        "dependencies": {
            "@steward/extension-api": sdk_dependency
        },
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
        (
            "App.test.tsx",
            test_source(&options.id, &command).into_bytes(),
        ),
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

fn default_sdk_dependency() -> String {
    format!(
        "https://github.com/iFence/steward/releases/latest/download/steward-extension-api-{HOST_API_VERSION}.tgz"
    )
}

fn test_source(plugin_id: &str, command: &str) -> String {
    let plugin_id = serde_json::to_string(plugin_id).expect("serialize plugin id");
    let command = serde_json::to_string(command).expect("serialize plugin command");
    TEST_SOURCE
        .replace("__PLUGIN_ID__", &plugin_id)
        .replace("__PLUGIN_COMMAND__", &command)
}

const APP_SOURCE: &str = r##"import { mount, signal, Text, View } from "nsh";
import { toast } from "@steward/extension-api";

function App() {
  const clicks = signal(0);
  const status = signal("Ready");

  async function sendToast() {
    await toast({ title: "Plugin ready", message: "The host call was successful." });
    status.set("Toast delivered");
  }

  return (
    <View style={{ flex: 1, padding: 24, gap: 16, backgroundColor: "#202024" }}>
      <Text style={{ color: "#ffffff", fontSize: 28, fontWeight: "bold" }}>__PLUGIN_NAME__</Text>
      <Text style={{ color: "#79797c" }}>
        Your Steward plugin is ready. Edit App.tsx to start building.
      </Text>
      <Text style={{ color: "#a8d7e8" }}>{status()}</Text>
      <View
        role="button"
        onPress={() => clicks.update((count) => count + 1)}
        style={{ alignSelf: "flex-start", backgroundColor: "#863bff", borderRadius: 8, padding: 12 }}
      >
        <Text style={{ color: "#ffffff" }}>Clicks: {clicks()}</Text>
      </View>
      <View
        role="button"
        onPress={sendToast}
        style={{ alignSelf: "flex-start", backgroundColor: "#0b6e99", borderRadius: 8, padding: 12 }}
      >
        <Text style={{ color: "#ffffff" }}>Send test toast</Text>
      </View>
    </View>
  );
}

mount(App);
"##;

const TEST_SOURCE: &str = r##"import { expect, open, test } from "nsh/test";

test("handles an interaction and a mocked host call", async () => {
  const app = await open("./App.tsx", {
    host: {
      params: {
        pluginId: __PLUGIN_ID__,
        command: __PLUGIN_COMMAND__,
        input: "",
        locale: "en-US",
        accent: "#863bff",
        dark: true,
      },
      handlers: {
        "steward.feedback.toast": {},
      },
    },
  });

  await app.press("text:Clicks: 0");
  expect(app.texts()).toContain("Clicks: 1");

  await app.press("text:Send test toast");
  expect(app.texts()).toContain("Toast delivered");
  expect(app.host.calls()).toContainEqual({
    name: "steward.feedback.toast",
    data: {
      title: "Plugin ready",
      message: "The host call was successful.",
    },
  });
});
"##;

fn readme(plugin_id: &str, command: &str) -> String {
    format!(
        r#"# {plugin_id}

Generated Steward NSH plugin scaffold.

```powershell
pnpm install
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
  "include": ["App.tsx", "App.test.tsx"]
}
"#;
