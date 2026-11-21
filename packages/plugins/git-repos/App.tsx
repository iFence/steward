import { mount, openUrl, signal, Text, View } from "nsh";
import { Input } from "nsh/base";
import {
  openPath,
  pickFolder,
  preference,
  readText,
  revealPath,
  storageGet,
  storageSet,
  walk,
} from "@steward/extension-api";

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function repositoryRoot(gitEntry: string): string {
  return gitEntry.replace(/[\\/]\.git$/i, "");
}

function remoteUrl(config: string): string | null {
  const match = /^\s*url\s*=\s*(.+)\s*$/m.exec(config);
  if (!match) return null;
  const value = match[1].trim();
  if (/^https?:\/\//i.test(value)) return value;
  const scp = /^git@([^:]+):(.+)$/.exec(value);
  if (scp) return `https://${scp[1]}/${scp[2].replace(/\.git$/i, "")}`;
  return null;
}

function App() {
  const scanDepth = Number(preference("scanDepth", "24"));
  const repositories = signal<string[]>([]);
  const favorites = signal<string[]>([]);
  const selected = signal("");
  const remote = signal<string | null>(null);
  const query = signal("");
  const status = signal("Choose a folder to discover repositories.");

  async function scan() {
    try {
      const root = await pickFolder(false);
      if (!root) {
        status.set("Folder selection cancelled.");
        return;
      }
      status.set("Scanning for .git entries...");
      const result = await walk({
        id: `repo-scan-${Date.now().toString(36)}`,
        root,
        maxResults: 20_000,
        maxDepth: Number.isFinite(scanDepth) ? Math.max(1, Math.min(64, scanDepth)) : 24,
        timeoutMs: 25_000,
      });
      const found = Array.from(
        new Set(
          result.entries
            .filter((entry) => /(^|[\\/])\.git$/i.test(entry.relativePath))
            .map((entry) => repositoryRoot(entry.path)),
        ),
      ).sort((left, right) => left.localeCompare(right));
      repositories.set(found);
      const saved = await storageGet("favorites");
      favorites.set(saved ? (JSON.parse(saved) as string[]) : []);
      status.set(
        `${found.length} repositories${result.truncated ? " (scan limited)" : ""} in ${result.elapsedMs} ms`,
      );
    } catch (error) {
      status.set(message(error));
    }
  }

  async function select(repository: string) {
    selected.set(repository);
    remote.set(null);
    try {
      const config = await readText(`${repository}\\.git\\config`, 256 * 1024);
      remote.set(remoteUrl(config));
      status.set(`Loaded ${repository}`);
    } catch (error) {
      status.set(message(error));
    }
  }

  async function toggleFavorite(repository: string) {
    const next = favorites().includes(repository)
      ? favorites().filter((value) => value !== repository)
      : [...favorites(), repository];
    favorites.set(next);
    try {
      await storageSet("favorites", JSON.stringify(next));
    } catch (error) {
      status.set(message(error));
    }
  }

  async function openRepository() {
    if (!selected()) return;
    try {
      await openPath(selected());
      status.set(`Opened ${selected()}`);
    } catch (error) {
      status.set(message(error));
    }
  }

  async function revealRepository() {
    if (!selected()) return;
    try {
      await revealPath(selected());
      status.set(`Revealed ${selected()}`);
    } catch (error) {
      status.set(message(error));
    }
  }

  async function openRemote() {
    if (!remote()) {
      status.set("No HTTP remote is available for this repository.");
      return;
    }
    try {
      const result = await openUrl(remote()!);
      status.set(result.opened ? `Opened ${remote()}` : "No browser accepted the remote URL.");
    } catch (error) {
      status.set(message(error));
    }
  }

  const filterText = query().trim().toLowerCase();
  const filtered = () =>
    [...repositories()].filter(
      (repository) => !filterText || repository.toLowerCase().includes(filterText),
    );

  return (
    <View style={{ flex: 1, padding: 24, gap: 14, backgroundColor: "#211a18" }}>
      <Text style={{ color: "#ffffff", fontSize: 28, fontWeight: "bold" }}>Git Repositories</Text>
      <Text style={{ color: "#d7b8ae" }}>{status()}</Text>
      <View style={{ flexDirection: "row", gap: 10 }}>
        <Action label="Choose folder" onPress={scan} />
        <Action label="Open" onPress={openRepository} />
        <Action label="Reveal" onPress={revealRepository} />
        <Action label="Open remote" onPress={openRemote} />
      </View>
      <Input
        value={query()}
        placeholder="Filter repositories"
        onChange={(event) => query.set(event.value)}
        style={{ color: "#ffffff", padding: 8, backgroundColor: "#302522", borderRadius: 6 }}
      />
      <Text style={{ color: "#d7b8ae" }}>
        Showing {filtered().length} of {repositories().length} repositories · query:{" "}
        {query() || "(empty)"}
      </Text>
      <View style={{ flex: 1, gap: 6 }}>
        <Each of={filtered().slice(0, 100)} key={(repository) => repository}>
          {(repository) => (
            <View
              role="button"
              onPress={() => select(repository())}
              style={{
                backgroundColor: selected() === repository() ? "#F05133" : "#302522",
                borderRadius: 6,
                padding: 10,
                flexDirection: "row",
                justifyContent: "space-between",
              }}
            >
              <Text style={{ color: "#ffffff" }}>{repository()}</Text>
              <Text
                role="button"
                style={{ color: favorites().includes(repository()) ? "#ffd166" : "#d7b8ae" }}
                onPress={() => toggleFavorite(repository())}
              >
                {favorites().includes(repository()) ? "Favorite" : "Favorite?"}
              </Text>
            </View>
          )}
        </Each>
      </View>
    </View>
  );
}

function Action(props: { label: string; onPress: () => void | Promise<void> }) {
  return (
    <View
      role="button"
      onPress={props.onPress}
      style={{ backgroundColor: "#F05133", borderRadius: 8, padding: 10 }}
    >
      <Text style={{ color: "#ffffff" }}>{props.label}</Text>
    </View>
  );
}

mount(App);
