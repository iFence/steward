import { Clipboard } from "nsh/clipboard";
import { mount, signal, Text, View } from "nsh";
import { Storage } from "nsh/storage";
import { clipboardHistory, launchParams, type ClipboardEntry } from "@steward/extension-api";

const PIN_KEY = "pinned";

function App() {
  const entries = signal<ClipboardEntry[]>([]);
  const pinned = signal<string[]>([]);
  const status = signal("Loading clipboard history...");

  void clipboardHistory()
    .then((items) => {
      entries.set(items);
      status.set(items.length ? "" : "Clipboard history is empty.");
    })
    .catch(() => status.set("Clipboard history is unavailable in this test host."));
  void Storage.getItem(PIN_KEY).then((value) => {
    try {
      pinned.set(value ? (JSON.parse(value) as string[]) : []);
    } catch {
      pinned.set([]);
    }
  });

  const ordered = () =>
    [...entries()].sort((a, b) => {
      const ap = pinned().includes(a.id) ? 1 : 0;
      const bp = pinned().includes(b.id) ? 1 : 0;
      return bp - ap || b.copied_at - a.copied_at;
    });

  const togglePin = (id: string) => {
    const next = pinned().includes(id)
      ? pinned().filter((entry) => entry !== id)
      : [...pinned(), id];
    pinned.set(next);
    void Storage.setItem(PIN_KEY, JSON.stringify(next));
  };

  return (
    <View style={{ flex: 1, padding: 24, gap: 12, backgroundColor: "#202024" }}>
      <Text style={{ color: "#ffffff", fontSize: 26, fontWeight: "bold" }}>Clipboard History</Text>
      <Text style={{ color: "#79797c" }}>
        {status() || `${ordered().length} entries · ${launchParams().locale}`}
      </Text>
      <Each of={ordered()}>
        {(entry) => (
          <View
            style={{
              padding: 12,
              borderRadius: 8,
              backgroundColor: "#2c2c31",
              flexDirection: "row",
              alignItems: "center",
              gap: 12,
            }}
          >
            <View
              role="button"
              style={{ flex: 1 }}
              onPress={async () => {
                await Clipboard.writeText(entry().text);
                status.set("Copied to clipboard");
              }}
            >
              <Text style={{ color: "#ffffff" }} numberOfLines={2}>
                {entry().text}
              </Text>
            </View>
            <View role="button" onPress={() => togglePin(entry().id)} style={{ padding: 8 }}>
              <Text style={{ color: pinned().includes(entry().id) ? "#863bff" : "#79797c" }}>
                {pinned().includes(entry().id) ? "Pinned" : "Pin"}
              </Text>
            </View>
          </View>
        )}
      </Each>
    </View>
  );
}

mount(App);
