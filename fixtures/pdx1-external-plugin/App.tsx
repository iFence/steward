import { mount, signal, Text, View } from "nsh";
import { toast } from "@steward/extension-api";

function App() {
  const clicks = signal(0);
  const status = signal("Ready");

  async function sendToast() {
    await toast({ title: "External plugin", message: "Installed SDK host call succeeded." });
    status.set("Toast delivered");
  }

  return (
    <View style={{ flex: 1, padding: 24, gap: 16, backgroundColor: "#202024" }}>
      <Text style={{ color: "#ffffff", fontSize: 28, fontWeight: "bold" }}>PDX-1 Fixture</Text>
      <Text style={{ color: "#a8d7e8" }}>{status()}</Text>
      <View
        role="button"
        onPress={() => clicks.update((count) => count + 1)}
        style={{
          alignSelf: "flex-start",
          backgroundColor: "#863bff",
          borderRadius: 8,
          padding: 12,
        }}
      >
        <Text style={{ color: "#ffffff" }}>Clicks: {clicks()}</Text>
      </View>
      <View
        role="button"
        onPress={sendToast}
        style={{
          alignSelf: "flex-start",
          backgroundColor: "#0b6e99",
          borderRadius: 8,
          padding: 12,
        }}
      >
        <Text style={{ color: "#ffffff" }}>Send test toast</Text>
      </View>
    </View>
  );
}

mount(App);
