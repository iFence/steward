import { mount, signal, Text, View } from "nsh";
import { Input, Switch } from "nsh/base";
import {
  backToLauncher,
  confirm,
  launchParams,
  onProgress,
  onVisibility,
  preference,
  reportProgress,
  setPageTitle,
  toast,
} from "@steward/extension-api";

function App() {
  const clicks = signal(0);
  const query = signal("");
  const enabled = signal(true);
  const title = preference("title", "Steward NSH UI");
  const showHints = preference("showHints", true);
  const density = preference<"compact" | "comfortable">("density", "comfortable");
  const workspace = preference("workspace", "");
  const editor = preference("editor", "");
  const feedback = signal("");
  const visible = signal(true);
  const presentation = launchParams().presentation;
  const gap = density === "compact" ? 8 : 16;
  onProgress((update) => feedback.set(update.message));
  onVisibility((nextVisible) => visible.set(nextVisible));

  async function showToast() {
    await toast({ title: "Steward", message: "Hosted toast delivered" });
    feedback.set("Toast sent");
  }

  async function requestConfirmation() {
    await reportProgress({
      id: "showcase",
      message: "Preparing confirmation",
      current: 1,
      total: 1,
    });
    const confirmed = await confirm({
      title: "Confirm action",
      message: "Continue with the showcase action?",
      confirmLabel: "Continue",
    });
    feedback.set(confirmed ? "Confirmed" : "Cancelled");
  }

  async function returnToLauncher() {
    await setPageTitle(title);
    await backToLauncher();
  }

  return (
    <View style={{ flex: 1, padding: 24, gap, backgroundColor: "#202024" }}>
      <Text style={{ color: "#ffffff", fontSize: 28, fontWeight: "bold" }}>{title}</Text>
      {showHints ? (
        <Text style={{ color: "#79797c" }}>
          Native TSX, signals, hosted preferences and host-controlled capabilities.
        </Text>
      ) : null}
      <Text style={{ color: "#79797c" }}>
        Presentation: {presentation} / {visible() ? "visible" : "inactive"}
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
          onChange={(event) => query.set(event.value)}
          style={{ color: "#ffffff", padding: 8, backgroundColor: "#202024", borderRadius: 6 }}
        />
      </View>
      <View style={{ flexDirection: "row", gap: 12, alignItems: "center" }}>
        <Switch checked={enabled()} onChange={(event) => enabled.set(event.checked)} />
        <Text style={{ color: "#ffffff" }}>{enabled() ? "Enabled" : "Disabled"}</Text>
      </View>
      <Text style={{ color: "#ffffff" }}>Query: {query() || "(empty)"}</Text>
      <Text style={{ color: "#79797c" }}>
        Workspace: {workspace || "(not set)"} / Editor: {editor || "(not set)"}
      </Text>
      <View style={{ flexDirection: "row", gap: 12 }}>
        <View
          role="button"
          onPress={showToast}
          style={{ backgroundColor: "#2c2c31", borderRadius: 8, padding: 10 }}
        >
          <Text style={{ color: "#ffffff" }}>Show toast</Text>
        </View>
        <View
          role="button"
          onPress={requestConfirmation}
          style={{ backgroundColor: "#2c2c31", borderRadius: 8, padding: 10 }}
        >
          <Text style={{ color: "#ffffff" }}>Confirm</Text>
        </View>
        <View
          role="button"
          onPress={returnToLauncher}
          style={{ backgroundColor: "#2c2c31", borderRadius: 8, padding: 10 }}
        >
          <Text style={{ color: "#ffffff" }}>Back</Text>
        </View>
        <Text style={{ color: "#ffffff" }}>{feedback()}</Text>
      </View>
    </View>
  );
}

mount(App);
