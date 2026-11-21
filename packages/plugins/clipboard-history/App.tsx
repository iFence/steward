import { effect, keymap, mount, List, signal, Text, View } from "nsh";
import { Input } from "nsh/base";
import {
  backToLauncher,
  clipboardHistory,
  copyText,
  launchParams,
  onCommand,
  onTheme,
  onVisibility,
  setPageTitle,
  storageGet,
  storageSet,
  type ClipboardEntry,
} from "@steward/extension-api";
import { clipboardCopyFor, type ClipboardCopy } from "./clipboard-i18n";

const PIN_KEY = "pinned";
const PREVIEW_LENGTH = 260;
const CONTROL_SIZE = 34;

interface Palette {
  accent: string;
  background: string;
  border: string;
  card: string;
  foreground: string;
  muted: string;
  selected: string;
}

function paletteFor(dark: boolean, accent: string): Palette {
  return dark
    ? {
        accent,
        background: "#202024",
        border: "#3a3941",
        card: "#2c2c31",
        foreground: "#f7f5fb",
        muted: "#9a98a3",
        selected: withAlpha(accent, "32"),
      }
    : {
        accent,
        background: "#f7f7f9",
        border: "#dddde4",
        card: "#ffffff",
        foreground: "#202026",
        muted: "#71717a",
        selected: withAlpha(accent, "20"),
      };
}

function withAlpha(color: string, alpha: string): string {
  return /^#[0-9a-f]{6}$/i.test(color) ? `${color}${alpha}` : color;
}

function commandQuery(params: ReturnType<typeof launchParams>): string {
  const input = params.input.trim();
  if (!input || input.toLowerCase() === params.command.toLowerCase()) return "";
  const prefix = `${params.command} `;
  return input.toLowerCase().startsWith(prefix.toLowerCase())
    ? input.slice(prefix.length).trim()
    : input;
}

function normalizedPreview(text: string): string {
  const compact = text.replace(/\s+/g, " ").trim();
  if (!compact) return "";
  return compact.length > PREVIEW_LENGTH ? `${compact.slice(0, PREVIEW_LENGTH - 1)}…` : compact;
}

function codePointCount(text: string): number {
  return Array.from(text).length;
}

function ageLabel(copiedAt: number, now: number, copy: ClipboardCopy): string {
  const seconds = Math.max(0, Math.floor(now / 1000 - copiedAt));
  if (seconds < 60) return copy.justNow;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return copy.minutesAgo(minutes);
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return copy.hoursAgo(hours);
  return copy.daysAgo(Math.floor(hours / 24));
}

function parsePinned(value: string | null): string[] {
  if (!value) return [];
  try {
    const parsed = JSON.parse(value) as unknown;
    return Array.isArray(parsed)
      ? [...new Set(parsed.filter((entry): entry is string => typeof entry === "string"))]
      : [];
  } catch {
    return [];
  }
}

function App() {
  const params = launchParams();
  const copy = clipboardCopyFor(params.locale);
  const embedded = Boolean(params.pluginId) && params.presentation === "embedded";
  const entries = signal<ClipboardEntry[]>([]);
  const pinned = signal<string[]>([]);
  const query = signal(commandQuery(params));
  const selectedId = signal("");
  const status = signal(copy.loading);
  const loading = signal(true);
  const accent = signal(params.accent);
  const dark = signal(params.dark);
  const now = signal(Date.now());
  let refreshGeneration = 0;

  const ordered = () => {
    const filter = query().trim().toLowerCase();
    const pinnedIds = new Set(pinned());
    return entries()
      .filter((entry) => !filter || entry.text.toLowerCase().includes(filter))
      .slice()
      .sort((left, right) => {
        const leftPinned = pinnedIds.has(left.id) ? 1 : 0;
        const rightPinned = pinnedIds.has(right.id) ? 1 : 0;
        return rightPinned - leftPinned || right.copied_at - left.copied_at;
      });
  };

  const selectedEntry = () => ordered().find((entry) => entry.id === selectedId()) ?? ordered()[0];

  async function refresh() {
    const generation = ++refreshGeneration;
    if (!entries().length) status.set(copy.loading);
    try {
      const items = await clipboardHistory();
      if (generation !== refreshGeneration) return;
      entries.set(items);
      now.set(Date.now());
      status.set(items.length ? "" : copy.emptyDescription);
    } catch {
      if (generation === refreshGeneration) status.set(copy.unavailable);
    } finally {
      if (generation === refreshGeneration) loading.set(false);
    }
  }

  async function copyEntry(entry: ClipboardEntry | undefined) {
    if (!entry) return;
    selectedId.set(entry.id);
    try {
      await copyText(entry.text);
      status.set(copy.copied);
    } catch {
      status.set(copy.copyFailed);
    }
  }

  function togglePin(id: string) {
    const next = pinned().includes(id)
      ? pinned().filter((entry) => entry !== id)
      : [...pinned(), id];
    pinned.set(next);
    void storageSet(PIN_KEY, JSON.stringify(next)).catch(() => {
      status.set(copy.pinFailed);
    });
  }

  function moveSelection(delta: number) {
    const items = ordered();
    if (!items.length) return;
    const current = items.findIndex((entry) => entry.id === selectedId());
    const index = current < 0 ? 0 : (current + delta + items.length) % items.length;
    selectedId.set(items[index].id);
  }

  function handlePanelKey(key: string) {
    if (key === "ArrowDown" || key === "Down") moveSelection(1);
    if (key === "ArrowUp" || key === "Up") moveSelection(-1);
    if (key === "Enter") void copyEntry(selectedEntry());
  }

  async function returnToLauncher() {
    try {
      await backToLauncher();
    } catch {
      // Escape and the host-owned close path remain available.
    }
  }

  void refresh();
  void storageGet(PIN_KEY)
    .then((value) => pinned.set(parsePinned(value)))
    .catch(() => pinned.set([]));

  onCommand(({ input }) => {
    query.set(commandQuery({ ...params, input }));
    void refresh();
  });
  onTheme((theme) => {
    accent.set(theme.accent);
    dark.set(theme.dark);
  });
  onVisibility((visible) => {
    if (visible) void refresh();
  });

  effect(() => {
    const items = ordered();
    if (!items.some((entry) => entry.id === selectedId())) {
      selectedId.set(items[0]?.id ?? "");
    }
  });

  effect(() => {
    if (!embedded) return;
    void setPageTitle(copy.title).catch(() => {});
  });

  keymap.bind({
    enter: () => void copyEntry(selectedEntry()),
    down: () => moveSelection(1),
    up: () => moveSelection(-1),
  });

  const currentPalette = () => paletteFor(dark(), accent());
  const colors: Palette = {
    get accent() {
      return currentPalette().accent;
    },
    get background() {
      return currentPalette().background;
    },
    get border() {
      return currentPalette().border;
    },
    get card() {
      return currentPalette().card;
    },
    get foreground() {
      return currentPalette().foreground;
    },
    get muted() {
      return currentPalette().muted;
    },
    get selected() {
      return currentPalette().selected;
    },
  };

  const visiblePinnedCount = () => ordered().filter((entry) => pinned().includes(entry.id)).length;

  return (
    <View
      style={{
        backgroundColor: colors.background,
        flex: 1,
        minHeight: 0,
        minWidth: 0,
        overflow: "hidden",
      }}
      testID="clipboard-root"
    >
      <View style={{ flex: 1, gap: 14, minHeight: 0, minWidth: 0, padding: 24 }}>
        <View
          style={{
            alignItems: "center",
            flexDirection: "row",
            flexShrink: 0,
            gap: 12,
            minWidth: 0,
          }}
        >
          {embedded ? (
            <View
              accessibilityLabel={copy.back}
              accessible
              onPress={returnToLauncher}
              role="button"
              style={{
                alignItems: "center",
                backgroundColor: colors.card,
                borderColor: colors.border,
                borderRadius: 9,
                borderWidth: 1,
                height: CONTROL_SIZE,
                justifyContent: "center",
                width: CONTROL_SIZE,
              }}
              testID="clipboard-back"
            >
              <Text
                style={{
                  color: colors.foreground,
                  fontSize: 20,
                  lineHeight: 20,
                  textAlign: "center",
                  textAlignVertical: "center",
                }}
              >
                ‹
              </Text>
            </View>
          ) : null}
          <View
            style={{
              alignItems: "center",
              backgroundColor: colors.accent,
              borderRadius: 10,
              height: 38,
              justifyContent: "center",
              width: 38,
            }}
          >
            <View style={{ backgroundColor: "#ffffff", borderRadius: 2, height: 3, width: 17 }} />
            <View
              style={{
                backgroundColor: "#ffffff",
                borderRadius: 2,
                height: 3,
                marginTop: 4,
                opacity: 0.82,
                width: 12,
              }}
            />
          </View>
          <View style={{ flex: 1, gap: 3, minWidth: 0 }}>
            <Text
              style={{
                color: colors.foreground,
                fontSize: 27,
                fontWeight: "bold",
                numberOfLines: 1,
              }}
            >
              {copy.title}
            </Text>
            <Text style={{ color: colors.muted, fontSize: 12, numberOfLines: 1 }}>
              {copy.summary(ordered().length, visiblePinnedCount())}
            </Text>
          </View>
          <View
            accessibilityLabel={copy.refresh}
            accessible
            disabled={loading()}
            onPress={() => void refresh()}
            role="button"
            style={{
              alignItems: "center",
              backgroundColor: colors.card,
              borderColor: colors.border,
              borderRadius: 9,
              borderWidth: 1,
              height: CONTROL_SIZE,
              justifyContent: "center",
              width: CONTROL_SIZE,
            }}
            testID="clipboard-refresh"
          >
            <Text style={{ color: loading() ? colors.muted : colors.foreground, fontSize: 17 }}>
              ↻
            </Text>
          </View>
        </View>

        <View style={{ flexShrink: 0 }}>
          <Input
            autoFocus
            maxLength={256}
            onChange={(event) => query.set(event.value)}
            onKeyPress={(event) => handlePanelKey(event.key)}
            placeholder={copy.searchPlaceholder}
            style={{
              backgroundColor: colors.card,
              borderColor: colors.border,
              borderRadius: 10,
              borderWidth: 1,
              color: colors.foreground,
              fontSize: 15,
              paddingHorizontal: 14,
              paddingVertical: 11,
              placeholderColor: colors.muted,
            }}
            submitBehavior="submit"
            testID="clipboard-search"
            value={query()}
          />
        </View>

        <List
          contentStyle={{ gap: 8, padding: 2 }}
          each={ordered()}
          empty={
            <View
              style={{
                alignItems: "center",
                flex: 1,
                justifyContent: "center",
                paddingHorizontal: 40,
                paddingVertical: 70,
              }}
              testID="clipboard-empty"
            >
              <Text
                style={{
                  color: colors.foreground,
                  fontSize: 18,
                  fontWeight: "bold",
                  textAlign: "center",
                }}
              >
                {query().trim() ? copy.noMatchesTitle : copy.emptyTitle}
              </Text>
              <Text
                style={{
                  color: colors.muted,
                  fontSize: 13,
                  marginTop: 7,
                  textAlign: "center",
                }}
              >
                {query().trim() ? copy.noMatchesDescription : copy.emptyDescription}
              </Text>
            </View>
          }
          keyExtractor={(entry) => entry.id}
          style={{ flex: 1, minHeight: 0, minWidth: 0 }}
          testID="clipboard-list"
        >
          {(entry) => {
            const item = entry();
            const selected = item.id === selectedId();
            const isPinned = pinned().includes(item.id);
            const preview = normalizedPreview(item.text);
            return (
              <View
                style={{
                  backgroundColor: selected ? colors.selected : colors.card,
                  borderColor: selected ? colors.accent : colors.border,
                  borderRadius: 10,
                  borderWidth: 1,
                  padding: 13,
                }}
              >
                <View style={{ alignItems: "flex-start", flexDirection: "row", gap: 12 }}>
                  <View
                    accessibilityLabel={`${preview || copy.whitespaceOnly}, ${ageLabel(
                      item.copied_at,
                      now(),
                      copy,
                    )}`}
                    accessible
                    onPress={() => void copyEntry(item)}
                    role="button"
                    style={{
                      flex: 1,
                      gap: 9,
                      minWidth: 0,
                    }}
                    testID={`clipboard-entry-${item.id}`}
                  >
                    <Text
                      style={{
                        color: colors.foreground,
                        fontSize: 14,
                        lineHeight: 20,
                        minWidth: 0,
                        numberOfLines: 3,
                      }}
                    >
                      {preview || copy.whitespaceOnly}
                    </Text>
                    <View
                      style={{
                        alignItems: "center",
                        flexDirection: "row",
                        justifyContent: "space-between",
                        minWidth: 0,
                      }}
                    >
                      <Text style={{ color: colors.muted, fontSize: 11 }}>
                        {ageLabel(item.copied_at, now(), copy)} ·{" "}
                        {copy.characters(codePointCount(item.text))}
                      </Text>
                      <Text
                        style={{
                          color:
                            status() === copy.copied && selected ? colors.accent : colors.muted,
                          fontSize: 11,
                          fontWeight: "bold",
                        }}
                      >
                        {status() === copy.copied && selected ? copy.copied : copy.copy}
                      </Text>
                    </View>
                  </View>
                  <View
                    accessibilityLabel={isPinned ? copy.unpin : copy.pin}
                    accessible
                    onPress={() => togglePin(item.id)}
                    role="button"
                    style={{
                      backgroundColor: isPinned ? withAlpha(colors.accent, "24") : "transparent",
                      borderColor: isPinned ? colors.accent : colors.border,
                      borderRadius: 7,
                      borderWidth: 1,
                      paddingHorizontal: 9,
                      paddingVertical: 5,
                    }}
                    testID={`clipboard-pin-${item.id}`}
                  >
                    <Text
                      style={{
                        color: isPinned ? colors.accent : colors.muted,
                        fontSize: 11,
                        fontWeight: "bold",
                      }}
                    >
                      {isPinned ? copy.pinned : copy.pin}
                    </Text>
                  </View>
                </View>
              </View>
            );
          }}
        </List>

        <View
          style={{
            alignItems: "center",
            flexDirection: "row",
            flexShrink: 0,
            justifyContent: "space-between",
            minHeight: 18,
            minWidth: 0,
          }}
        >
          <Text style={{ color: colors.muted, flex: 1, fontSize: 11, numberOfLines: 1 }}>
            {status()}
          </Text>
          <Text style={{ color: colors.muted, fontSize: 11 }}>{copy.hint(embedded)}</Text>
        </View>
      </View>
    </View>
  );
}

mount(App);
