import { mount, signal, Text, View } from "nsh";
import { Input } from "nsh/base";
import { deleteSecret, fetchHttp, getSecret, setSecret } from "@steward/extension-api";

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function App() {
  const status = signal("Check Steward's scoped HTTPS path.");
  const details = signal("");
  const tokenInput = signal("");

  async function checkNetwork() {
    try {
      status.set("Requesting api.github.com/rate_limit...");
      const token = await getSecret("github-token");
      const headers: Record<string, string> = { Accept: "application/vnd.github+json" };
      if (token) headers.Authorization = `Bearer ${token}`;
      const response = await fetchHttp({
        url: "https://api.github.com/rate_limit",
        method: "GET",
        headers,
        timeoutMs: 10_000,
        maxResponseBytes: 256 * 1024,
      });
      const payload = JSON.parse(response.body) as {
        rate?: { remaining?: number; limit?: number };
      };
      status.set(`HTTPS ${response.status} in ${response.elapsedMs} ms`);
      details.set(
        `GitHub API requests remaining: ${payload.rate?.remaining ?? "unknown"} / ${
          payload.rate?.limit ?? "unknown"
        }`,
      );
    } catch (error) {
      status.set(message(error));
      details.set("");
    }
  }

  async function saveToken() {
    const token = tokenInput().trim();
    if (!token) {
      status.set("Enter a GitHub token before saving.");
      return;
    }
    try {
      await setSecret("github-token", token);
      tokenInput.set("");
      status.set("GitHub token saved in the system-protected secret store.");
    } catch (error) {
      status.set(message(error));
    }
  }

  async function clearToken() {
    try {
      const deleted = await deleteSecret("github-token");
      status.set(deleted ? "Saved GitHub token deleted." : "No saved GitHub token was present.");
    } catch (error) {
      status.set(message(error));
    }
  }

  return (
    <View style={{ flex: 1, padding: 24, gap: 14, backgroundColor: "#10252d" }}>
      <Text style={{ color: "#ffffff", fontSize: 28, fontWeight: "bold" }}>Network Check</Text>
      <Text style={{ color: "#a8d7e8" }}>{status()}</Text>
      <Text style={{ color: "#e4f6fc" }}>{details()}</Text>
      <Input
        testID="github-token"
        value={tokenInput()}
        placeholder="Optional GitHub token"
        onChange={(event) => tokenInput.set(event.value)}
        style={{ color: "#ffffff", padding: 8, backgroundColor: "#173b47", borderRadius: 6 }}
      />
      <View style={{ flexDirection: "row", gap: 10 }}>
        <View
          role="button"
          onPress={saveToken}
          style={{ backgroundColor: "#185E73", borderRadius: 8, padding: 8 }}
        >
          <Text style={{ color: "#ffffff" }}>Save token</Text>
        </View>
        <View
          role="button"
          onPress={clearToken}
          style={{ backgroundColor: "#3b3030", borderRadius: 8, padding: 8 }}
        >
          <Text style={{ color: "#ffffff" }}>Clear token</Text>
        </View>
      </View>
      <View
        role="button"
        onPress={checkNetwork}
        style={{
          backgroundColor: "#0B6E99",
          borderRadius: 8,
          padding: 10,
          alignSelf: "flex-start",
        }}
      >
        <Text style={{ color: "#ffffff" }}>Run HTTPS check</Text>
      </View>
    </View>
  );
}

mount(App);
