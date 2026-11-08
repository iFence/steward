import { Input, mount, signal, Text, View } from "nsh";
import { Switch } from "nsh/base";

function App() {
  const clicks = signal(0);
  const query = signal("");
  const enabled = signal(true);

  return (
    <View style={{ flex: 1, padding: 24, gap: 16, backgroundColor: "#202024" }}>
      <Text style={{ color: "#ffffff", fontSize: 28, fontWeight: "bold" }}>Steward NSH UI</Text>
      <Text style={{ color: "#79797c" }}>
        Native TSX, signals and host-controlled capabilities.
      </Text>
      <View style={{ flexDirection: "row", gap: 12, alignItems: "center" }}>
        <View
          role="button"
          onPress={() => clicks.update((count) => count + 1)}
          style={{ backgroundColor: "#863bff", borderRadius: 8, padding: 12 }}
        >
          <Text style={{ color: "#ffffff" }}>Increment</Text>
        </View>
        <Text style={{ color: "#ffffff" }}>Clicks: {clicks()}</Text>
      </View>
      <View style={{ backgroundColor: "#2c2c31", borderRadius: 8, padding: 12 }}>
        <Input
          value={query()}
          placeholder="Type here"
          onChange={(text) => query.set(text)}
          style={{ color: "#ffffff", padding: 8, backgroundColor: "#202024", borderRadius: 6 }}
        />
      </View>
      <View style={{ flexDirection: "row", gap: 12, alignItems: "center" }}>
        <Switch checked={enabled()} onChange={enabled.set} />
        <Text style={{ color: "#ffffff" }}>{enabled() ? "Enabled" : "Disabled"}</Text>
      </View>
      <Text style={{ color: "#ffffff" }}>Query: {query() || "(empty)"}</Text>
    </View>
  );
}

mount(App);
