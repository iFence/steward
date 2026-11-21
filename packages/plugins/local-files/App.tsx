import { mount, signal, Text, View } from "nsh";
import {
  installedApplications,
  openPath,
  openWith,
  pickFile,
  pickFolder,
  readText,
  revealPath,
  walk,
  type InstalledApplication,
  type WalkEntry,
} from "@steward/extension-api";

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function App() {
  const root = signal("");
  const entries = signal<WalkEntry[]>([]);
  const preview = signal("");
  const status = signal("Choose a folder to inspect.");
  const applications = signal<InstalledApplication[]>([]);
  let firstEntry: WalkEntry | undefined;

  async function chooseFolder() {
    try {
      const selected = await pickFolder(false);
      if (!selected) {
        status.set("Folder selection cancelled.");
        return;
      }
      root.set(selected);
      status.set("Scanning selected folder...");
      const result = await walk({
        root: selected,
        maxResults: 5_000,
        maxDepth: 24,
        timeoutMs: 15_000,
      });
      entries.set(result.entries);
      status.set(
        `${result.entries.length} entries${result.truncated ? " (limited)" : ""} in ${result.elapsedMs} ms`,
      );
    } catch (error) {
      status.set(message(error));
    }
  }

  async function chooseFile() {
    try {
      const selected = await pickFile();
      if (!selected) {
        status.set("File selection cancelled.");
        return;
      }
      const text = await readText(selected, 64 * 1024);
      preview.set(text.slice(0, 4_000));
      status.set(`Read ${text.length} characters from ${selected}`);
    } catch (error) {
      status.set(message(error));
    }
  }

  async function showApplications() {
    try {
      const apps = await installedApplications();
      applications.set(apps.slice(0, 20));
      status.set(`Found ${apps.length} installed applications.`);
    } catch (error) {
      status.set(message(error));
    }
  }

  async function inspectEntry() {
    firstEntry = entries()[0];
    if (!firstEntry) return;
    try {
      await revealPath(firstEntry.path);
      status.set(`Revealed ${firstEntry.relativePath}`);
    } catch (error) {
      status.set(message(error));
    }
  }

  async function openEntry() {
    firstEntry = entries()[0];
    if (!firstEntry) return;
    try {
      await openPath(firstEntry.path);
      status.set(`Opened ${firstEntry.relativePath}`);
    } catch (error) {
      status.set(message(error));
    }
  }

  async function openFirstWithEditor() {
    firstEntry = entries()[0];
    const application = applications()[0];
    if (!firstEntry || !application) {
      status.set("Scan a folder and load applications first.");
      return;
    }
    try {
      await openWith(application.id, firstEntry.path);
      status.set(`Opened ${firstEntry.relativePath} with ${application.name}`);
    } catch (error) {
      status.set(message(error));
    }
  }

  return (
    <View style={{ flex: 1, padding: 24, gap: 14, backgroundColor: "#172027" }}>
      <Text style={{ color: "#ffffff", fontSize: 28, fontWeight: "bold" }}>
        Local File Inspector
      </Text>
      <Text style={{ color: "#9fb7c4" }}>{status()}</Text>
      <Text style={{ color: "#d7e7ee" }}>Root: {root() || "(not selected)"}</Text>
      <View style={{ flexDirection: "row", gap: 10, flexWrap: "wrap" }}>
        <Action label="Choose folder" onPress={chooseFolder} />
        <Action label="Choose file" onPress={chooseFile} />
        <Action label="Open first entry" onPress={openEntry} />
        <Action label="Reveal first entry" onPress={inspectEntry} />
        <Action label="Load applications" onPress={showApplications} />
        <Action label="Open first with app" onPress={openFirstWithEditor} />
      </View>
      <View style={{ flexDirection: "row", gap: 16, flex: 1 }}>
        <View style={{ width: 400, gap: 8 }}>
          <Text style={{ color: "#ffffff", fontWeight: "bold" }}>Entries</Text>
          <Each of={entries().slice(0, 80)} key={(entry) => entry.path}>
            {(entry) => (
              <Text style={{ color: "#b8d3df", fontSize: 12 }}>
                {entry().directory ? "[dir] " : "[file] "}
                {entry().relativePath}
              </Text>
            )}
          </Each>
        </View>
        <View style={{ flex: 1, gap: 8 }}>
          <Text style={{ color: "#ffffff", fontWeight: "bold" }}>Text preview</Text>
          <Text style={{ color: "#b8d3df", fontSize: 12 }}>
            {preview() || "(no file selected)"}
          </Text>
          <Text style={{ color: "#ffffff", fontWeight: "bold", marginTop: 12 }}>Applications</Text>
          <Each of={applications()} key={(application) => application.id}>
            {(application) => (
              <Text style={{ color: "#b8d3df", fontSize: 12 }}>{application().name}</Text>
            )}
          </Each>
        </View>
      </View>
    </View>
  );
}

function Action(props: { label: string; onPress: () => void | Promise<void> }) {
  return (
    <View
      role="button"
      onPress={props.onPress}
      style={{ backgroundColor: "#176B87", borderRadius: 8, padding: 10 }}
    >
      <Text style={{ color: "#ffffff" }}>{props.label}</Text>
    </View>
  );
}

mount(App);
