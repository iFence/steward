import { mount, openUrl, signal, Text, View } from "nsh";
import { Input } from "nsh/base";
import {
  openPath,
  pickFolder,
  preference,
  readText,
  runProcess,
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
      const root = await pickFolder(true);
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

  async function git(
    repository: string,
    arguments_: string[],
    timeoutMs = 30_000,
  ): Promise<string> {
    const result = await runProcess({
      id: `git-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 9)}`,
      executable: "git.exe",
      arguments: arguments_,
      workingDirectory: repository,
      timeoutMs,
      maxOutputBytes: 1024 * 1024,
    });
    if (result.exitCode !== 0) {
      throw new Error(result.stderr.trim() || result.stdout.trim() || "Git command failed");
    }
    return result.stdout.trim();
  }

  async function loadGitState() {
    if (!selected()) return;
    try {
      status.set("Reading repository state with controlled Git execution...");
      const summary = await git(selected(), ["status", "--porcelain=v1", "--branch"]);
      const remotes = await git(selected(), ["remote", "-v"]);
      let unpushed = "No upstream";
      try {
        const upstream = await git(selected(), [
          "rev-parse",
          "--abbrev-ref",
          "--symbolic-full-name",
          "@{u}",
        ]);
        const count = await git(selected(), ["rev-list", "--count", `${upstream}..HEAD`]);
        unpushed = `${count} unpushed commit${count === "1" ? "" : "s"}`;
      } catch {
        unpushed = "No upstream configured";
      }
      const clean = summary.split(/\r?\n/).filter((line) => line && !line.startsWith("##")).length;
      status.set(
        `${clean === 0 ? "Clean" : `${clean} changed files`}; ${unpushed}; ${
          remotes ? "remotes loaded" : "no remotes"
        }`,
      );
    } catch (error) {
      status.set(message(error));
    }
  }

  async function cloneRepository() {
    if (!remote()) {
      status.set("Select a repository with an HTTPS remote first.");
      return;
    }
    try {
      const destination = await pickFolder(true);
      if (!destination) {
        status.set("Clone destination selection cancelled.");
        return;
      }
      const remotePath = remote()!
        .replace(/[?#].*$/, "")
        .replace(/\/+$/, "");
      const name =
        remotePath
          .split("/")
          .pop()
          ?.replace(/\.git$/i, "") || "repository";
      status.set(`Cloning ${remote()}...`);
      await git(destination, ["clone", "--", remote()!, `${destination}\\${name}`], 120_000);
      status.set(`Cloned ${remote()} into ${destination}\\${name}`);
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
      <View style={{ flexDirection: "row", gap: 10 }}>
        <Action label="Git state" onPress={loadGitState} />
        <Action label="Clone" onPress={cloneRepository} />
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
