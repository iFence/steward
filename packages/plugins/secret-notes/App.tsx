import { mount, signal, Text, View } from "nsh";
import { Input } from "nsh/base";
import { deleteSecret, getSecret, listSecrets, setSecret } from "@steward/extension-api";

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function App() {
  const key = signal("");
  const value = signal("");
  const keys = signal<string[]>([]);
  const status = signal("Secrets are encrypted for the current Windows user.");

  async function save() {
    if (!key().trim() || !value()) {
      status.set("Enter both a key and value.");
      return;
    }
    try {
      await setSecret(key().trim(), value());
      value.set("");
      status.set(`Saved ${key().trim()}.`);
      await refresh();
    } catch (error) {
      status.set(message(error));
    }
  }

  async function load() {
    if (!key().trim()) {
      status.set("Enter a key to load.");
      return;
    }
    try {
      const secret = await getSecret(key().trim());
      status.set(secret === null ? `No secret named ${key().trim()}.` : `Found ${key().trim()}.`);
    } catch (error) {
      status.set(message(error));
    }
  }

  async function remove() {
    if (!key().trim()) {
      status.set("Enter a key to delete.");
      return;
    }
    try {
      const deleted = await deleteSecret(key().trim());
      status.set(deleted ? `Deleted ${key().trim()}.` : `No secret named ${key().trim()}.`);
      await refresh();
    } catch (error) {
      status.set(message(error));
    }
  }

  async function refresh() {
    try {
      keys.set(await listSecrets());
    } catch (error) {
      status.set(message(error));
    }
  }

  return (
    <View style={{ flex: 1, padding: 24, gap: 12, backgroundColor: "#24221c" }}>
      <Text style={{ color: "#ffffff", fontSize: 28, fontWeight: "bold" }}>Secret Notes</Text>
      <Text style={{ color: "#d8cfb8" }}>{status()}</Text>
      <Input
        testID="secret-key"
        value={key()}
        placeholder="Secret key"
        onChange={(event) => key.set(event.value)}
        style={{ color: "#ffffff", padding: 8, backgroundColor: "#343126", borderRadius: 6 }}
      />
      <Input
        testID="secret-value"
        value={value()}
        placeholder="Secret value"
        onChange={(event) => value.set(event.value)}
        style={{ color: "#ffffff", padding: 8, backgroundColor: "#343126", borderRadius: 6 }}
      />
      <View style={{ flexDirection: "row", gap: 8 }}>
        <Action label="Save" onPress={save} />
        <Action label="Load" onPress={load} />
        <Action label="Delete" onPress={remove} />
        <Action label="Refresh" onPress={refresh} />
      </View>
      <Text style={{ color: "#d8cfb8" }}>
        Saved keys: {keys().length ? keys().join(", ") : "(none listed)"}
      </Text>
    </View>
  );
}

function Action(props: { label: string; onPress: () => void }) {
  return (
    <View role="button" onPress={props.onPress} style={{ backgroundColor: "#665b3d", padding: 8 }}>
      <Text style={{ color: "#ffffff" }}>{props.label}</Text>
    </View>
  );
}

mount(App);
