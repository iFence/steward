# Steward —— 架构说明

> 本文档第一版即建仓计划原文（来源：`steward-repo-plan.md`，入库时间 2026-08-19）。
> 后续架构决策变更直接在本文件迭代记录；M0 建仓时做出的具体决策见文末“决策记录”。

## 定位

> 定位：Rust + GPUI 原生主进程，TypeScript 插件系统，主打内存占用低、响应速度快。
> 本文档是可执行的建仓计划，按顺序照做即可跑起第一个可运行的骨架，并把插件规模化后的工程隐患提前落到里程碑里。

## 0. 项目基本信息

- 项目名：**Steward**
- Bundle ID：`com.hiaspirin.steward`
- Rust crate 前缀：`steward-*`（如 `steward-core-engine`）
- npm scope：`@steward/*`（如 `@steward/extension-api`）
- 许可证：MIT
- 仓库形态：**Monorepo**（Rust workspace + TS 插件 SDK/官方插件 都放一个仓库）

## 1. 仓库顶层结构

```
steward/
├── Cargo.toml                     # workspace 根
├── rust-toolchain.toml            # 固定 Rust 版本
├── .gitignore
├── LICENSE
├── README.md
├── crates/
│   ├── app/                       # 二进制入口：GPUI 窗口/托盘/全局热键/生命周期
│   ├── core-engine/                # 搜索、索引、模糊匹配、排序（无 UI 依赖，可单测）
│   ├── plugin-host/                # 插件生命周期管理、路由、权限、IPC 网关
│   ├── plugin-registry/            # 插件元数据缓存（SQLite），负责增量扫描/索引，M2 新增
│   ├── storage/                    # SQLite 封装、配置文件读写
│   ├── ui-components/              # 基于 gpui-component 的业务级 UI 组件
│   └── ipc-protocol/               # 主进程 <-> 插件运行时共享的消息协议定义
├── plugin-runtime/                # 独立的插件宿主进程（Rust 二进制，内嵌 QuickJS）
│   └── src/
│       ├── isolate_pool.rs         # 常规插件：进程内多实例池，内存/超时熔断
│       └── isolated_process.rs     # 高风险插件：独立子进程隔离，M2 后期补
├── packages/                      # TS 侧，pnpm workspace
│   ├── extension-api/              # 面向插件开发者的 TS 类型声明 + 运行时 polyfill
│   ├── create-plugin-cli/          # 脚手架 CLI（可选，后期做）
│   └── plugins/
│       ├── calculator/              # 官方示例插件 1
│       └── clipboard-history/       # 官方示例插件 2
├── docs/
│   ├── architecture.md             # 架构说明（把讨论结论固化进去）
│   ├── extension-api.md            # 插件 API 文档
│   ├── plugin-manifest-spec.md     # manifest 字段规范，含触发条件/权限声明，M2 新增
│   └── benchmarks.md               # 性能基准记录
├── scripts/                        # 构建/打包/基准测试脚本
└── .github/
    └── workflows/
        ├── ci.yml                   # rust fmt/clippy/test + ts lint/build
        └── release.yml              # 打包发布（后期再启用）
```

## 2. 技术选型清单

| 领域 | 选型 | 备注 |
|---|---|---|
| UI 框架 | `gpui`（Zed 仓库内 crate）+ `gpui-component`（Longbridge） | GPUI pre-1.0，git 依赖 + Cargo.lock 锁定 |
| 全局热键/托盘 | `global-hotkey` + `tray-icon` | 跨平台 crate |
| 模糊匹配 | `nucleo` | Helix 同款，性能优先 |
| 数据库 | `rusqlite`（bundled feature） | 同步足够快，避免引入 async ORM |
| 插件运行时 | `rquickjs`（QuickJS 绑定） | 内存优先，不追求 Node 全兼容 |
| 插件 IPC | 自定义 JSON-RPC，走 Unix Domain Socket / Windows Named Pipe | 协议定义放 `ipc-protocol` crate |
| 插件打包 | `esbuild` | TS -> 单文件 JS，产物不依赖 Node API |
| TS 包管理 | `pnpm` workspace | 单仓多插件场景省心 |
| CI | GitHub Actions | 先跑 fmt/clippy/test |

## 3. 里程碑规划

### M0：骨架能跑起来（1-2 周）
- [x] `cargo new` workspace，`app` crate 用 GPUI 弹出一个空窗口
- [x] 全局热键唤起/隐藏窗口，验证呼出延迟
- [x] 基础 CI 跑通（`cargo fmt --check` / `cargo clippy` / `cargo test`）
- [x] `docs/architecture.md` 写好，固化架构结论

**验收标准**：冷启动到窗口可交互 < 50ms（release 构建、macOS 目标；当前在 Windows 开发机上记录基线数据）

### M1：应用启动器 MVP（核心卖点验证阶段，重点投入）
- [x] `core-engine`：扫描本机已安装应用，建索引
- [x] 接入 `nucleo` 做模糊匹配 + 结果排序
- [x] `ui-components`：搜索框 + 虚拟滚动结果列表
- [x] `storage`：SQLite 存索引缓存和使用频率
- [~] 内存占用/响应延迟基准测试，写进 `docs/benchmarks.md`，和 uTools/Raycast 对比（Windows 开发机 debug 数据已记录；uTools/Raycast 对比待 release 数据积累后补齐）

**验收标准**：内存/速度数据要好看且可复现，这是对外最大的说服力来源

### M2：插件系统 v1 ——按“规模化后不塌”的标准设计，不只是“跑通一个插件”

这一阶段直接把 1000 插件规模下会暴露的问题设计进去，避免后面返工。

**基础链路**
- [ ] `ipc-protocol`：定义主进程 <-> 插件运行时消息格式（JSON-RPC）
- [ ] `plugin-runtime`：独立二进制，内嵌 rquickjs，能加载 JS 文件并调用
- [ ] `packages/extension-api`：暴露 `List`、`ActionPanel`、`showToast` 等最小 API 集
- [ ] 用 `packages/plugins/calculator` 跑通端到端：TS -> esbuild -> QuickJS -> UI 渲染

**规模化对策（4 项必须做，不是可选项）**
- [ ] **元数据缓存**（`plugin-registry`）：插件 manifest 解析结果缓存进 SQLite，只有版本变化才重新扫描解析；启动时直接读缓存，杜绝全量文件 I/O 扫描拖慢冷启动
- [ ] **触发条件路由**（`plugin-manifest-spec.md` + `plugin-host`）：manifest 声明命令名 / 关键字前缀 / 正则触发条件，搜索时先做路由过滤，绝不对未匹配插件发起唤起；对声明“动态参与”的插件加响应超时熔断（如 100ms 未返回则跳过本次渲染）
- [ ] **分级隔离**（`isolate_pool.rs` + `isolated_process.rs`）：默认插件走进程内多实例池，设置 QuickJS 堆内存上限 + 执行超时，超限直接 kill 该实例；声明了网络/文件系统权限或依赖较重的插件升级为独立子进程隔离
- [ ] **最小权限模型**：manifest 声明所需能力，主进程按声明开放对应 host function，插件默认零权限

**验收标准**：模拟安装 500-1000 个插件（可脚本批量生成测试 manifest），验证冷启动时间、搜索响应延迟不随安装量线性劣化，只随“实际激活数”变化

### M3：插件 API 覆盖面 + Node 兼容 polyfill + UI 打磨
- [ ] 补齐 `Detail`、`Form`、`LocalStorage`、`Clipboard` 等 API
- [ ] 覆盖最常用的 20-30 个 Node 内置模块 polyfill（fs、path、buffer、http 等），明确写文档说明不支持 native binding
- [ ] 做第二个官方插件（`clipboard-history`）验证 API 够不够用
- [ ] 主题/深色模式、动画细节打磨

### M4：Windows 支持
- [ ] 视 GPUI/gpui-component 在 Windows 上的成熟度决定方案
- [ ] IPC 层的 Named Pipe 分支补齐并测试
- [ ] 非 Windows 平台窗口显隐（`App::hide`/`activate` 回退）打磨

### M5：插件生态基础设施
- [ ] `create-plugin-cli` 脚手架
- [ ] 插件签名/来源校验 + 自动化静态分析（依赖白名单、API 调用范围检查）
- [ ] 简单的插件市场（本地 manifest 索引即可）

## 4. 工程规范

- **Rust**：`rustfmt` + `clippy -D warnings` 强制 CI 检查；commit 用 Conventional Commits
- **TS**：`eslint` + `prettier`，插件包统一走 `esbuild` 构建，产物体积和依赖在 CI 里做检查
- **性能基准**：从 M0 开始建立 `scripts/bench.sh`，记录冷启动时间、常驻内存（RSS）、呼出延迟三个核心指标，每个里程碑跑一次存进 `docs/benchmarks.md`
- **规模化基准**（M2 起新增）：`scripts/gen-test-plugins.sh` 批量生成 N 个测试插件 manifest（N=100/500/1000），跑冷启动和搜索延迟回归测试，防止插件数量增长后悄悄劣化

## 5. 待验证风险清单（建仓后前两周优先去趟一遍）

1. GPUI 在目标平台（先 macOS，再 Linux）上的窗口置顶/失焦隐藏/多显示器行为是否符合预期
2. `rquickjs` 沙箱内 host function 回调（Rust 侧）的性能开销，是否拖慢插件响应
3. `gpui-component` 的组件覆盖面是否够用，哪些控件需要自己补
4. 插件崩溃/死循环时的隔离和超时熔断机制的具体实现方式（`isolate_pool.rs` 的核心）
5. QuickJS 堆内存上限设置的合理默认值（过小影响正常插件，过大失去熔断意义）

## 决策记录

### 2026-08-19（M0 建仓）

- `gpui` 不从 crates.io 引入，直接从 Zed 仓库 git 引用（`git = "https://github.com/zed-industries/zed", package = "gpui"`），提交由 `Cargo.lock` 锁定（当前 `7a7c3e1d`）。
- `gpui_platform`（Zed 仓库，`features = ["font-kit"]`）与 `gpui-component`（Longbridge 仓库）同样走 git，与 git 版 gpui 保持类型一致（crates.io 版 gpui-component 0.5.1 绑定 crates.io gpui 0.2.2，混用会产生两份 gpui 类型冲突）。
- Rust 工具链固定 `1.95.0`（gpui 锁定的提交需要比 1.92 更新的编译器，`rust-toolchain.toml` 显式锁定）。
- M0 窗口显隐：Windows 上通过原生 HWND `ShowWindow(SW_HIDE/SW_SHOW)` 实现（GPUI Windows 后端 `App::hide` 是空操作）；其他平台回退 `App::hide`/`App::activate`，M4 打磨。
- M0 全局热键：`global-hotkey` 注册 `Ctrl+Alt+Space`，事件经 crossbeam channel 由 GPUI 前台任务每 10ms 轮询桥接（GPUI 无系统级热键 API，回调线程不能直接操作 GPUI 状态）。
- Windows 上所有构建（含 debug）均启用 `windows_subsystem = "windows"`，避免启动时闪现控制台窗口；debug 下 `eprintln!` 无控制台输出属预期，M3 起引入 tracing 文件日志。
- 图标：`assets/steward.png`（1254×1254）+ Pillow 生成多尺寸 `assets/icon.ico`（16/24/32/48/64/128/256），并写入 `steward-app` 的 `[package.metadata.bundle]`。
- 许可证：MIT，版权持有者写 `Steward contributors`。
- 依赖版本（crates.io，建仓当日确认）：`rusqlite` 0.40（bundled）、`nucleo` 0.5、`rquickjs` 0.12、`global-hotkey` 0.8、`tray-icon` 0.24。
- `create-plugin-cli` 按计划“后期做”，建仓时不建包；`release.yml` 仅 `workflow_dispatch` 占位。

### 2026-08-19（M0.1 托盘 + 快速启动栏）

- 启动静默：主窗口以 `show: false` 创建（Windows 后端不会应用 GPUI 计算的 placement），呼出时由 `platform::show` 自行按主显示器工作区居中（`GetDpiForWindow` 换算物理像素，`GetMonitorInfoW` 取工作区），再 `ShowWindow(SW_SHOW)` + `SetForegroundWindow`（含 AttachThreadInput 抢焦点技巧）。
- 快速启动栏形态：`WindowKind::PopUp`（Windows 上即 `WS_EX_TOOLWINDOW | WS_EX_TOPMOST`，无任务栏入口、置顶）+ `appears_transparent: true`（无系统标题栏）+ `is_resizable: false`，尺寸 960×56 居中于主屏；内容区为横向 flex：标题、搜索占位框、两个官方插件占位 chip、快捷键提示。
- 托盘生命周期：应用真正的外壳是托盘图标（Windows 用 `Icon::from_resource(1)` 读取 embed-resource 嵌入的 ICO，macOS 用 `image` 解码内置 PNG）；左键单击与全局热键同路（切换显隐），右键菜单含“显示/隐藏”“退出”；关闭快速启动栏窗口（Alt+F4）不退出应用，下次呼出自动重建。
- 图标嵌入：`crates/app/app.rc`（`1 ICON` + `32512 ICON` 双写）+ `embed-resource` 编译进 exe，Explorer/任务栏/托盘共用 `assets/icon.ico`；托盘事件/菜单事件与热键事件合并进同一个 10ms 前台轮询任务，回调线程不直接触碰 GPUI 状态。
- 托盘目标平台：仅 Windows/macOS 启用（`tray-icon` 为 target 依赖）；Linux 默认不启用，避免 CI 引入 gtk/libappindicator 系统依赖，M4 再评估。

### 2026-08-19（M0.2 单一输入框快速启动栏）

- 宽度 960 → 760、高度 56 → 60，垂直位置从屏幕正中改为上部约 1/3（`position_centered` 的 `y = work.top + (工作区高 - 条高) / 3`），水平仍居中。
- 去掉标题/占位 chip/快捷键提示等所有杂项文字，整条 760×60 就是一个输入框：单一 `0x232332` 背景 + `text_sm` + 占位符“搜索应用或输入命令...”（走 i18n `search-placeholder`）。
- 文本输入：根元素 `on_key_down` 直接处理字符（`keystroke.key_char`）、空格、退格、删除、左右/Home/End，光标用 2px 竖条 + `repeat_synced` 动画闪烁；Esc 在按键层直接隐藏（keybinding 仅作兜底）。
- 拖动：`window_control_area(WindowControlArea::Drag)` 直接挂在根元素上，整条即原生 HTCAPTION 拖动区（Windows 由 DefWindowProc 起模态移动循环），输入不再需要点击（每次呼出自动聚焦）。
- 焦点：`FocusHandle` 在 `main` 中创建并存入共享 `LauncherState`，每次 show（含隐藏后重新呼出、窗口被 Alt+F4 关闭后重建）都重新 `focus()`，修复“隐藏后再呼出无法输入”的问题。

### 2026-08-19（i18n 国际化）

- 采用与 Zed 一致的 Fluent 技术栈：`i18n-embed` 0.16（`fluent-system` + `desktop-requester`）＋ `i18n-embed-fl` 0.10 ＋ `rust-embed` 8 ＋ `unic-langid` 0.9；`.ftl` 资源放 `crates/app/i18n/{语言}/main.ftl`，编译期经 `rust-embed` 内嵌。
- 支持语言：zh / en / fr / de / ru / ja / ko（7 种）。活动语言默认取系统语言，`DesktopLanguageRequester::requested_languages()` 末尾兜底 `en`，保证系统语言不在支持集时回退英文。
- 范围：i18n 仅覆盖原生宿主自有文案（当前全部在 `crates/app/src/main.rs`：搜索占位符 + 两个托盘菜单项）；插件提供的 UI 文案由插件自身携带，暂不经宿主 Fluent 翻译（M2 再议）。
- 品牌名 `Steward` 为专有名词，不随语言翻译。
- 诊断性 `eprintln!` / `anyhow` 上下文保持英文（面向开发，不参与 i18n）。

### 2026-08-19（M1 应用启动器 MVP）

- 应用扫描（Windows 优先）：`core-engine` 的 `WinAppsScanner` 遍历当前用户与全体用户的开始菜单 `Programs` 目录，递归收集 `.lnk`，经 ShellLink COM（`IShellLinkW`/`IPersistFile`）解析目标，只保留直接指向 `.exe` 的去重条目；`GetPath` 不带 `SLGP_RAWPATH`（flags 0），让 ShellLink 展开 `%windir%` 等环境变量，保证路径可启动、可取图标；其他平台返回空 `NoopScanner`（M4 补齐）。`platform_scanner()` 为平台分发入口。
- 冷启动加速（缓存优先）：启动时先跑一次扫描建索引用 `mark_seen` 写回 SQLite；若扫描为空（如平台未实现）则回退读 `cached_apps()`，保证冷启动不因扫描失败而阻塞。
- 模糊匹配 + 频率加权排序：`nucleo::Matcher::new(Config::DEFAULT)`（大小写不敏感、拉丁归一化）逐条 `fuzzy_match`；排序分 = 模糊分 + `20 * ln(1 + 使用次数)`，空查询按使用次数倒序。满足“Windows 优先 + 接通启动 + 模糊分×频率加权”的验收口径。
- 结果列表 UI：`ui-components` 用 `gpui-component` 的 `ListState`/`ListDelegate` 做虚拟滚动（固定行高 48px，最多展示 8 行后滚动）。一个关键约束：`Context<T>` 只实现 `AppContext` 而非 `VisualContext`，因此结果更新走 `Entity::update`（无需 window），只有选中/确认需要 window 时再以参数传入；`set_results` 无 window 即可刷新。
- Enter 启动并在委托回调里 `upsert_usage` 记账：回调只触碰 `Rc` 共享的索引与缓存、绝不操作 UI，故可在 `ListState` 的 update 内安全触发；启动后由 app 层 `after_confirm` 统一复位查询、重置窗口高度并隐藏。
- 动态窗口高度：搜索时按可见结果数实时 `SetWindowPos` 改高度（保留左上角、只增高度向下展开，物理像素按 `GetDpiForWindow` 换算）；呼出时复用 `LauncherState::height()` 重建窗口尺寸。
- 错误处理从简：`launch` 用 `Command::new(path).spawn()` 分离子进程；非 Windows 平台启动为 stub（`anyhow::bail!`）。

### 2026-08-19（M1 下拉样式修复）

- DPI 感知：应用此前未声明 DPI 感知，在 200% 缩放（192 DPI）显示器上被系统按 96 DPI 虚拟化放大/模糊。现于 `main` 启动时调用 `SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)`，让启动器像其他应用一样**跟随系统缩放**：尺寸均为逻辑像素（760×60 条、48px 行、字号 rem），GPUI 按显示器 scale 渲染为物理像素（200% 下 1520×120/888）。窗口创建尺寸被 GPUI Windows `check_given_bounds` 在显示器边角误判拒绝时，show 时由平台层强制设置 `760 × 窗口 DPI/96` 物理宽 + 当前结果高度。
- 动态窗口高度：`SetWindowPos` 在窗口可见时直接改高会让 DirectX 渲染器 viewport 失步，下拉区域呈现清屏白色（即"白色区域遮挡"的真身）；改为调用 GPUI 自身的 `window.resize()`（经前台 executor 异步执行），渲染器同步正常。窗口高度公式（`60 + 48 × min(结果数, 8)`，按 DPI 换算）不变。
- 结果列表渲染：`gpui-component` 的 `List`/虚拟列表在 Windows 上渲染损坏（行文字缺失或变淡、下拉右下角纯白 quad）。M1 结果数上限 8 行，不需要虚拟化，`results_list` 改为简单 div 堆叠列表（滚动容器 + 行高/字号按 DPI 缩放），选中行为 `#89b4fa` 20% 透明度 + 蓝色边框，悬停为 `#313244`；`ResultList`/`ResultListDelegate` 公开 API 保持兼容。
- 应用图标：`app_icons`（Windows）用 `SHGetFileInfoW(SHGFI_ICON | SHGFI_LARGEICON)` 取 exe 的 shell 大图标，`DrawIconEx` 画进 32 位 DIB 保留透明通道，转 RGBA 后用 `image` 编码 PNG 包装为 `gpui::Image`；按路径缓存，只对可见 8 行提取（入口先展开路径中的 `%VAR%` 作防御），行左侧以 24 逻辑 px 渲染；行右侧不再显示可执行文件路径，改为本地化的类型标签（`application`，如"应用"），经 `ResultListDelegate::type_label` 传入。
- 中文输入：启动器查询框实现 GPUI `EntityInputHandler`（查询为单行文档，IME 组字走 `replace_and_mark_text_in_range`、提交走 `replace_text_in_range`），渲染时组字区加下划线；`LauncherInputElement` 在 paint 阶段注册输入处理器。Windows 上 GPUI 的 IME 上下文关联依赖 WM_PAINT 且会因输入处理器被临时取走而误禁用，故每帧调用 `ImmAssociateContextEx(IACE_DEFAULT)` 重新关联，保证拼音等输入法可组字；组字期间编辑/导航键（含 Enter/Esc）交给输入法，不作启动/隐藏处理。IME 提交在光标处插入、组字只替换组字区（带单测覆盖"你好 + 说话 → 你好说话"），不会整体替换查询。
- 布局与视觉：结果列表包在 `h(result_height) + mx(margin)` 固定高度容器内，输入行不再被压缩；`init_theme` 切换 `ThemeMode::Dark`；行内应用名与路径均省略号截断；保持不透明矩形窗口（圆角/透明化留待后续）。呼出时自动执行一次空查询（按使用频率排序），打开即显示常用应用。
