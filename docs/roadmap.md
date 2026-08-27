# Steward Plugin Platform & AI Ecosystem Roadmap

> 说明：本文档是愿景 + 计划。M0–M5 与仓库 `docs/architecture.md`（架构 ground truth）对齐，M6–M8（AI Capability / MCP / AI Search）为产品核心能力的正式规划，M9–M10 为后期扩展。当前仓库定位为 Windows 优先的启动器 + 插件平台，本文档在此基础上规划长期演进。

## 项目定位

Steward 是基于 Rust + GPUI 构建的新一代 AI Native Desktop Platform（长期愿景）。

AI 能力（Capability）与 MCP 集成是产品的核心组成部分，不是可选愿景：插件系统提供底座，AI/MCP 在其上构成差异化能力。

当前阶段目标：

* 小而美的 Windows 启动器：呼出即应、内存占用低、长期稳定
* 可扩展的插件平台：TypeScript 插件（QuickJS）不拖慢核心

融合目标（长期）：

* Raycast 的效率工具生态
* Vicinae 的开源插件兼容路线
* uTools 的插件扩展模式
* MCP / AI Agent Tool 生态
* Rust 原生性能、安全和跨平台能力

最终目标：

> 构建一个支持传统插件、AI Agent、搜索调研能力和自动化工作流的桌面级 Plugin Operating System。

## 设计理念

传统桌面插件：

```text
Plugin
  |
Command
  |
UI
```

未来 AI 桌面插件：

```text
Capability
  |
  |-------------------------------------------------
  |                  |                           |
UI Extension      AI Tool                   Automation
  |                  |                           |
GPUI             MCP Server                Workflow
```

Steward 不仅运行插件，而是管理：

* 用户能力
* AI 能力
* 数据能力
* 外部服务能力

## 当前状态

| 里程碑 | 状态 |
|---|---|
| M0 Core Foundation | DONE |
| M1 Launcher MVP | DONE |
| M2 Plugin System v1 | DONE |

## 总体架构

```text
                        Steward
                           |
                   Plugin Kernel
               (plugin-host / plugin-registry)
                           |
                Capability Protocol
                 (ipc-protocol, JSON-RPC)
                           |
     -------------------------------------------------
     |                    |                         |
  QuickJS               WASM                    Node
 Native Plugin       Native Module        Compatibility
 (M2, rquickjs)        (M10)              Sidecar (M9)
     |
     |
 GPUI UI Runtime
                           |
                   AI Capability Layer
     -------------------------------------------------
     |                    |                         |
    MCP                AI Search              Agent Skill
 Server             Research Tool            Workflow
```

## Runtime 策略

### QuickJS Runtime（M2）

定位：Steward 原生插件运行环境。

用途：

* 工具插件 / UI 插件 / 自动化插件 / AI 助手插件
* 优势：小体积、快启动、安全沙箱
* 产物：TS -> esbuild 单文件 JS -> QuickJS（不依赖 Node API）

### WASM Runtime（M10，愿景）

定位：高性能能力扩展。

用途：

* AI 推理、向量计算、图片处理、音视频、数据分析
* 支持语言：Rust / C/C++ / Go / Zig
* 依赖政策：届时在根 `Cargo.toml` 的 `[workspace.dependencies]` 引入运行时依赖

### Node Runtime（M9，兼容层，非核心）

定位：外部生态兼容层。

用途：兼容 Raycast Extension、Vicinae Extension、npm 生态插件。

架构：

```text
Steward
  |
Node Sidecar
  |
Raycast / Vicinae Extension
```

避免：

* 主程序体积膨胀
* Node 常驻内存
* 安全风险

说明：M3 阶段先在 QuickJS 内提供 20–30 个常用 Node 内置模块 polyfill（fs / path / buffer / http 等，不支持 native binding）；Node Sidecar 仅作为 M9 兼容层引入。

## 里程碑规划

### M0 - Core Foundation（DONE）

仓库定义：workspace 骨架、GPUI 启动栏/窗口、全局热键、系统托盘、基础 CI。

### M1 - Application Launcher MVP（DONE）

仓库定义：应用扫描（Windows 优先）、nucleo 模糊匹配、SQLite 索引与使用频率、性能基准（`docs/benchmarks.md`）。

### M2 - Plugin System v1（DONE）

目标：建立统一插件基础设施，按"规模化后不塌"的标准设计。

**基础链路**

- [x] `ipc-protocol`：定义主进程 <-> 插件运行时消息格式（JSON-RPC）
- [x] `plugin-runtime`：独立二进制，内嵌 rquickjs，能加载 JS 文件并调用
- [x] `packages/extension-api`（npm 包名 `@steward/extension-api`）：暴露 `List` / `ActionPanel` / `showToast` 最小 API 集
- [x] `packages/plugins/calendar` 跑通端到端：TS -> esbuild -> QuickJS -> UI 渲染

**规模化对策（4 项必须做）**

- [x] 元数据缓存（`plugin-registry`）：manifest 解析结果缓存进 SQLite，只有版本变化才重新扫描解析；启动时直接读缓存
- [x] 触发条件路由（`plugin-host` + manifest）：命令名 / 关键字前缀 / 正则 / dynamic 触发，先路由过滤，动态参与插件 100ms 响应超时熔断
- [x] 分级隔离（`isolate_pool.rs` + `isolated_process.rs`）：默认进程内多实例池（QuickJS 堆内存上限 + 执行超时，超限 kill 该实例）；声明网络/文件系统权限或依赖较重的插件升级为独立子进程
- [x] 最小权限模型：manifest 声明所需能力，主进程按声明开放对应 host function，插件默认零权限

**验收标准**：模拟安装 500–1000 个插件，冷启动时间与搜索响应延迟不随安装量线性劣化，只随"实际激活数"变化。

### M3 - Plugin UI Framework & API Coverage（TODO）

目标：声明式 UI 框架 + 插件 API 覆盖面。

- [ ] 声明式 UI：React Style DSL -> Virtual UI Tree -> GPUI Renderer；首批组件 `List` / `Detail` / `Form` / `Grid` / `ActionPanel` / `SearchBar`
- [ ] API 覆盖：`Detail` / `Form` / `LocalStorage` / `Clipboard` 等
- [ ] 覆盖 20–30 个常用 Node 内置模块 polyfill（fs / path / buffer / http 等），明确不支持 native binding
- [ ] 第二个官方插件 `clipboard-history` 验证 API 可用性
- [ ] 主题 / 深色模式 / 动画细节打磨

### M4 - Windows Support（TODO）

- [ ] 评估 GPUI / gpui-component 在 Windows 上的成熟度并定方案
- [ ] IPC 层 Named Pipe 分支补齐并测试
- [ ] Windows 平台窗口显隐（`App::hide` / `activate` 回退）打磨；非 Windows 平台 stub

### M5 - Plugin Ecosystem Infrastructure v1（TODO）

- [ ] `create-plugin-cli` 脚手架
- [ ] 插件签名 / 来源校验 + 自动化静态分析（依赖白名单、API 调用范围检查）
- [ ] 简单的插件市场 v1（本地 manifest 索引即可）

### M6 - AI Capability System（TODO，核心能力）

这是 Steward 与 Raycast/Vicinae 最大的区别：让插件不仅提供 UI，还提供 AI 可调用能力。

- [ ] Capability 模型：插件声明并暴露 AI 可调用函数
- [ ] 示例：`github-plugin` 同时提供 Search UI 与 `github.search_repository()` / `github.create_issue()` / `github.analyze_pr()`，供 AI Agent 调用

### M7 - MCP Compatibility Layer（TODO，核心能力）

- [ ] 支持 MCP Server / MCP Client / Tool Discovery
- [ ] 架构：MCP Server -> Capability Adapter -> Steward Tool Registry -> AI Agent
- [ ] 示例：安装 `github-mcp`，注册 `github.search` / `github.issue.create` / `github.pr.review`，AI 可自动调用

### M8 - AI Search & Research Framework（TODO，核心能力）

目标：打造 Steward 核心 AI 调研能力，区别于普通搜索。

- [ ] 流程：User Question -> Research Agent -> Search Capability -> Data Collection -> Analysis -> Knowledge Output
- [ ] `SearchProvider` 统一抽象（`trait SearchProvider { fn search(query: String) }`），支持 Web Search / Local File Search / Code Search / Knowledge Base / Vector Search
- [ ] Research Plugin：Source Collector / Document Parser / Summarizer / Knowledge Extractor / Citation Manager，形成 Search -> Collect -> Analyze -> Generate 流水线

### M9 - Raycast / Vicinae Ecosystem Compatibility（TODO，后期扩展）

目标：最大化利用已有生态。

- [ ] 架构：Raycast Extension -> @raycast/api -> Compatibility Layer -> Steward API
- [ ] Node Sidecar（非核心）承载兼容层；第一阶段支持 `showToast` / `Clipboard` / `open` / `preferences` / `storage` / `network`
- [ ] 目标：兼容 70%–80% 工具类 Extension

### M10 - WASM Runtime + Plugin Marketplace（TODO，后期扩展）

- [ ] WASM 高性能扩展：AI 推理、向量计算、图片/音视频处理、数据分析；支持 Rust / C/C++ / Go / Zig
- [ ] Marketplace 支持 QuickJS Plugin / WASM Plugin / MCP Tool / AI Skill
- [ ] 插件类型：Extension / Capability / Agent Skill / Workflow

## Plugin Manifest（与仓库 `docs/plugin-manifest-spec.md` 对齐）

示例：

```json
{
  "id": "com.example.calendar",
  "name": "Calendar",
  "version": "1.0.0",
  "commands": [
    {
      "name": "calendar",
      "title": "Calendar",
      "trigger": { "type": "command" }
    }
  ],
  "permissions": ["clipboard.write"],
  "isolation": "shared-pool"
}
```

要点：

* `trigger.type`：`command` / `prefix` / `regex` / `dynamic`（dynamic 必须带响应超时熔断）
* `isolation`：`shared-pool`（默认）/ `dedicated-process`
* `permissions`：白名单枚举，默认零权限（M2 扩展）
* 兼容 package.json / Raycast extension.json / Vicinae extension manifest 属于 M9 兼容层目标，不是 v1 承诺

## 最终生态模型

```text
                  Steward
                     |
              Capability Kernel
 -----------------------------------------------------
 |              |                |                  |
Plugin        MCP              AI Skill          Workflow
 |              |                |                  |
QuickJS       Tool             Agent             Automation
                     |
                  WASM
                     |
              Native Performance
```

## 与现有产品关系

### Raycast

学习：Extension Model、Command Workflow、Developer Experience
兼容：Extension API（M9）

### Vicinae

学习：开源架构、Raycast 生态复制方式、Extension Marketplace
兼容：Vicinae Extensions（M9）

### uTools

学习：国内插件生态、快速调用体验、桌面入口模式

### MCP / AI Agent

扩展：AI 工具调用、自动研究、工作流执行（M6–M8）

## 开发优先级

### 第一阶段：插件平台

```
M2 Plugin System -> M3 UI Framework & API -> M4 Windows Support -> M5 Ecosystem Infra v1
```

形成：Steward Native Plugin Platform

### 第二阶段：AI 核心能力（正式规划）

说明：M6–M8 属于产品核心能力层，非愿景；插件平台（M2–M5）稳定后立即启动。

```
M6 AI Capability -> M7 MCP -> M8 AI Search
```

形成：AI Native Desktop Platform

### 第三阶段：生态扩展

```
M9 Compatibility -> M10 WASM + Marketplace
```

形成：Steward Plugin Ecosystem

## 长期目标

Steward 不定位为"另一个 Raycast"，而定位为：

> AI Native Desktop Operating System

核心能力：

* Rust Native
* GPUI UI
* QuickJS Plugin
* WASM Extension
* Raycast / Vicinae Compatibility
* MCP Integration
* AI Search & Research
* Agent Automation

最终：

> 让任何桌面能力，都可以被用户调用，也可以被 AI Agent 调用。
