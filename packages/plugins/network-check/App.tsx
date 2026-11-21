import { mount, signal, Text, View } from "nsh";
import { fetchHttp } from "@steward/extension-api";

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function App() {
  const status = signal("Check Steward's scoped HTTPS path.");
  const details = signal("");

  async function checkNetwork() {
    try {
      status.set("Requesting api.github.com/rate_limit...");
      const response = await fetchHttp({
        url: "https://api.github.com/rate_limit",
        method: "GET",
        headers: { Accept: "application/vnd.github+json" },
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

  return (
    <View style={{ flex: 1, padding: 24, gap: 14, backgroundColor: "#10252d" }}>
      <Text style={{ color: "#ffffff", fontSize: 28, fontWeight: "bold" }}>Network Check</Text>
      <Text style={{ color: "#a8d7e8" }}>{status()}</Text>
      <Text style={{ color: "#e4f6fc" }}>{details()}</Text>
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
