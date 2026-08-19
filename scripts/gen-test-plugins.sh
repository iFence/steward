#!/usr/bin/env bash
# 批量生成 N 个测试插件 manifest（N=100/500/1000），M2 规模化基准用。
# 当前为占位：M2 里程碑实现后用于冷启动/搜索延迟回归。
set -euo pipefail

echo "gen-test-plugins.sh: 将在 M2 实现（规模化基准）"
