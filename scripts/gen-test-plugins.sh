#!/usr/bin/env bash
# 批量生成 N 个测试插件（N=100/500/1000），M2 规模化基准用。
# 验证：冷启动直接读 `plugins.db` 缓存、搜索只唤醒匹配插件——延迟应随
# "实际激活数"变化，而不是随安装量线性劣化。
#
# 用法：scripts/gen-test-plugins.sh <count> <target-dir>
set -euo pipefail

count="${1:?usage: gen-test-plugins.sh <count> <target-dir>}"
root="${2:?usage: gen-test-plugins.sh <count> <target-dir>}"

mkdir -p "$root"
for ((i = 0; i < count; i++)); do
  id="com.bench.plugin$(printf '%04d' "$i")"
  dir="$root/$id"
  mkdir -p "$dir/dist"
  # 三类触发条件按 i 轮转，覆盖 command / prefix / dynamic 路由。
  case $((i % 3)) in
    0) trigger='{ "type": "command" }' ;;
    1) trigger='{ "type": "prefix", "value": "p'$i' " }' ;;
    2) trigger='{ "type": "dynamic" }' ;;
  esac
  cat > "$dir/plugin.json" <<EOF
{
  "id": "$id",
  "name": "Bench $i",
  "version": "1.0.0",
  "commands": [
    { "name": "cmd$i", "title": "Bench $i", "trigger": $trigger }
  ],
  "permissions": [],
  "isolation": "shared-pool"
}
EOF
  cat > "$dir/dist/index.js" <<'EOF'
var __stewardPlugin = (() => {
    function command(name, input) {
        return { type: "list", items: [{ id: "one", title: name, subtitle: input }] };
    }
    return { command: command };
})();
EOF
done

echo "generated $count test plugins under $root"
echo "point STEWARD_PLUGINS_DIR at $root and run the app to benchmark"
