# Steward

> 定位：Rust + GPUI 原生主进程，TypeScript 插件系统，主打内存占用低、响应速度快。

Steward 是一个启动器/插件平台骨架：原生主进程基于 [GPUI](https://gpui.rs)（直接从 Zed 仓库 git 引用），插件系统计划使用内嵌 QuickJS 的独立宿主进程，TS 插件通过 esbuild 打成单文件 JS。

当前仓库状态：M0 骨架 —— GPUI 空窗口 + 全局热键呼出/隐藏已经跑通，插件链路（M2）为占位。

## 仓库结构

```
steward/
├── Cargo.toml                 # Rust workspace 根
├── crates/
│   ├── app/                   # 二进制入口：GPUI 窗口/热键/生命周期
│   ├── core-engine/           # 搜索、索引、模糊匹配、排序（无 UI 依赖）
│   ├── plugin-host/           # 插件生命周期管理、路由、权限、IPC 网关
│   ├── plugin-registry/       # 插件元数据缓存（SQLite）
│   ├── storage/               # SQLite 封装、配置文件读写
│   ├── ui-components/         # 基于 gpui-component 的业务级 UI 组件
│   └── ipc-protocol/          # 主进程 <-> 插件运行时共享的消息协议定义
├── plugin-runtime/            # 独立插件宿主进程（内嵌 QuickJS）
├── packages/                  # TS 侧（pnpm workspace）
│   ├── extension-api/         # 面向插件开发者的 TS 类型声明 + 运行时 polyfill
│   └── plugins/               # 官方示例插件
├── docs/                      # 架构/API/manifest/基准文档
├── scripts/                   # 构建/打包/基准测试脚本
└── .github/workflows/         # CI / release
```

## 快速开始

前置要求：Rust stable、Node.js ≥ 20、pnpm。

```bash
# 原生应用（M0）
cargo run -p steward-app

# TS 侧
pnpm install
pnpm build
```

M0 快捷键：`Ctrl+Alt+Space` 呼出/隐藏窗口，`Esc` 隐藏窗口，关闭窗口退出应用。

## 文档

- [架构说明](docs/architecture.md)
- [插件 API 草案](docs/extension-api.md)
- [插件 manifest 规范草案](docs/plugin-manifest-spec.md)
- [性能基准](docs/benchmarks.md)

## 里程碑

- [x] M0：骨架能跑起来（GPUI 窗口 + 全局热键）
- [ ] M1：应用启动器 MVP（应用扫描 + nucleo 模糊匹配 + SQLite 索引）
- [ ] M2：插件系统 v1（JSON-RPC IPC + QuickJS 运行时 + 规模化对策）
- [ ] M3：插件 API 覆盖面 + Node polyfill + UI 打磨
- [ ] M4：Windows 支持完善
- [ ] M5：插件生态基础设施（脚手架、签名校验、插件市场）

## 许可证

[MIT](LICENSE)
