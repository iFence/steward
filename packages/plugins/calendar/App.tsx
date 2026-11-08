import { Clipboard } from "nsh/clipboard";
import { mount, signal, Text, View } from "nsh";
import { launchParams, onCommand } from "@steward/extension-api";

const WEEKDAYS = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

function resolveMonth(input: string): { year: number; month: number } {
  const now = new Date();
  let year = now.getFullYear();
  let month = now.getMonth() + 1;
  const absolute = input.match(/^(\d{4})(?:-(\d{1,2}))?$/);
  const offset = input.match(/^[+-]?\d+$/);
  if (absolute) {
    year = Number(absolute[1]);
    month = absolute[2] ? Number(absolute[2]) : month;
  } else if (offset) {
    month += Number(offset[0]);
    year += Math.floor((month - 1) / 12);
    month = ((((month - 1) % 12) + 12) % 12) + 1;
  }
  return { year, month: Math.max(1, Math.min(12, month)) };
}

function iso(year: number, month: number, day: number): string {
  return `${year}-${String(month).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
}

function App() {
  const initial = resolveMonth(
    launchParams()
      .input.replace(/^calendar\s*/i, "")
      .trim(),
  );
  const year = signal(initial.year);
  const month = signal(initial.month);
  const copied = signal("");

  onCommand(({ input }) => {
    const next = resolveMonth(input.replace(/^calendar\s*/i, "").trim());
    year.set(next.year);
    month.set(next.month);
  });

  const days = () => {
    const first = new Date(year(), month() - 1, 1);
    const offset = (first.getDay() + 6) % 7;
    const count = new Date(year(), month(), 0).getDate();
    const cells: Array<number | null> = Array(offset).fill(null);
    for (let day = 1; day <= count; day += 1) cells.push(day);
    return cells;
  };

  return (
    <View style={{ flex: 1, padding: 24, gap: 16, backgroundColor: "#202024" }}>
      <View style={{ flexDirection: "row", alignItems: "center", gap: 12 }}>
        <Text style={{ color: "#ffffff", fontSize: 26, fontWeight: "bold" }}>
          {new Date(year(), month() - 1, 1).toLocaleString(launchParams().locale, {
            month: "long",
            year: "numeric",
          })}
        </Text>
        <View style={{ flex: 1 }} />
        <Text style={{ color: "#8f8f95" }}>{copied()}</Text>
      </View>
      <View style={{ flexDirection: "row", flexWrap: "wrap" }}>
        {WEEKDAYS.map((label) => (
          <View style={{ width: "14.285%", padding: 6 }}>
            <Text style={{ color: "#79797c", fontSize: 12 }}>{label}</Text>
          </View>
        ))}
        <Each of={days()}>
          {(day) => (
            <View style={{ width: "14.285%", padding: 3 }}>
              {day() === null ? (
                <View style={{ height: 46 }} />
              ) : (
                <View
                  role="button"
                  onPress={async () => {
                    const value = iso(year(), month(), day()!);
                    await Clipboard.writeText(value);
                    copied.set(`Copied ${value}`);
                  }}
                  style={{
                    height: 46,
                    alignItems: "center",
                    justifyContent: "center",
                    borderRadius: 8,
                    backgroundColor: "#2c2c31",
                  }}
                >
                  <Text style={{ color: "#ffffff", fontSize: 15 }}>{day()}</Text>
                </View>
              )}
            </View>
          )}
        </Each>
      </View>
    </View>
  );
}

mount(App);
