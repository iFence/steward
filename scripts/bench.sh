#!/usr/bin/env bash
# Steward 基准：冷启动时间 / 常驻内存（RSS）/ 呼出延迟。
#
# 用法：
#   scripts/bench.sh             # 测量已构建的 debug 二进制
#   scripts/bench.sh --release   # 先构建 release 再测量
#
# Windows（M2 起）：请用 scripts/bench-resident.ps1，测量
# 启动→托盘就绪（含一次性 GPUI 初始化）/ 常驻 RSS / 首呼与二次呼出延迟；
# 本脚本保留给 POSIX 平台使用。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="debug"
if [[ "${1:-}" == "--release" ]]; then
  MODE="release"
  cargo build --release -p steward-app
fi

BIN="$ROOT/target/$MODE/steward-app"
[[ -f "$BIN.exe" ]] && BIN="$BIN.exe"

if [[ ! -f "$BIN" ]]; then
  echo "binary not found: $BIN (run cargo build first)" >&2
  exit 1
fi

echo "== Steward benchmark (mode: $MODE) =="

# 1. 冷启动（近似：进程存活）
START_NS=$(date +%s%N)
"$BIN" &
PID=$!
for _ in $(seq 1 200); do
  if kill -0 "$PID" 2>/dev/null; then
    break
  fi
  sleep 0.01
done
END_NS=$(date +%s%N)
COLD_MS=$(( (END_NS - START_NS) / 1000000 ))
echo "cold start (process alive): ${COLD_MS} ms"

# 2. RSS
case "$(uname -s)" in
  Linux|Darwin)
    RSS_KB=$(ps -o rss= -p "$PID" | tr -d ' ')
    RSS_MB=$(( RSS_KB / 1024 ))
    echo "rss: ${RSS_MB} MB (${RSS_KB} KB)"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    RSS_BYTES=$(powershell -NoProfile -Command "(Get-Process -Id $PID).WorkingSet64" | tr -d '\r')
    RSS_MB=$(( RSS_BYTES / 1048576 ))
    echo "rss: ${RSS_MB} MB"
    ;;
  *)
    echo "rss: N/A (unsupported platform)"
    ;;
esac

echo "== 呼出延迟（手动）: 按 Ctrl+Alt+Space 并用秒表记录窗口出现时间 =="

kill "$PID" 2>/dev/null || true
wait "$PID" 2>/dev/null || true
