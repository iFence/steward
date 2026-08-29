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
│       ├── calendar/                # 官方示例插件 1
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
- [x] 内存占用/响应延迟基准测试，写进 `docs/benchmarks.md`，和 uTools/Raycast 对比（Windows 开发机 release 数据已记录于 M1 基线；uTools/Raycast 对比待多机 release 数据积累后补齐）

**验收标准**：内存/速度数据要好看且可复现，这是对外最大的说服力来源

### M2：插件系统 v1 ——按“规模化后不塌”的标准设计，不只是“跑通一个插件”

这一阶段直接把 1000 插件规模下会暴露的问题设计进去，避免后面返工。

**基础链路**
- [x] `ipc-protocol`：定义主进程 <-> 插件运行时消息格式（JSON-RPC）
- [x] `plugin-runtime`：独立二进制，内嵌 rquickjs，能加载 JS 文件并调用
- [x] `packages/extension-api`：暴露 `List`、`ActionPanel`、`showToast` 等最小 API 集
- [x] 用 `packages/plugins/calendar` 跑通端到端：TS -> esbuild -> QuickJS -> UI 渲染

**规模化对策（4 项必须做，不是可选项）**
- [x] **元数据缓存**（`plugin-registry`）：插件 manifest 解析结果缓存进 SQLite，只有版本变化才重新扫描解析；启动时直接读缓存，杜绝全量文件 I/O 扫描拖慢冷启动
- [x] **触发条件路由**（`plugin-manifest-spec.md` + `plugin-host`）：manifest 声明命令名 / 关键字前缀 / 正则触发条件，搜索时先做路由过滤，绝不对未匹配插件发起唤起；对声明“动态参与”的插件加响应超时熔断（如 100ms 未返回则跳过本次渲染）
- [x] **分级隔离**（`isolate_pool.rs` + `isolated_process.rs`）：默认插件走进程内多实例池，设置 QuickJS 堆内存上限 + 执行超时，超限直接 kill 该实例；声明了网络/文件系统权限或依赖较重的插件升级为独立子进程隔离
- [x] **最小权限模型**：manifest 声明所需能力，主进程按声明开放对应 host function，插件默认零权限

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

### 2026-08-26（URL 链接用默认浏览器打开）

- 启动器支持把形如 `https://github.com`、`172.20.2.14:1230`（IPv4:端口）的查询当作"用浏览器打开"命令：结果列表顶部插入一条链接行（左侧是本地化的 "Open in Browser" 标签、右侧是 "Command" 类型标签，无图标），确认后调用默认浏览器打开，而不是只当成应用名搜索。
- 识别逻辑放在 `steward-core-engine` 新增的 `link` 模块（无 UI 依赖、可单测，与内置计算器同法）：带 `://` 且 scheme 为 http/https/ftp/ftps 的查询原样打开；无 scheme 的输入若为合法 host（IPv4、`localhost` 或带点的域名，均可选 `:端口`）——`host:端口`（如 `172.20.2.14:1230`）原样透传、不补前缀，这类地址多为纯 HTTP 的内网服务，补 `https://` 反而打不开；无端口的裸 host 才补 `https://`。带空格的查询、单点号外的词、非法端口、纯数字小数（如 `1.2.3`，非 IPv4）一律不触发，避免劫持普通搜索。
- 打开走 `launch::open_url`：Windows 复用 `ShellExecuteW` 的 `open` verb，由 shell 按 URL 关联路由到默认浏览器，无需定位浏览器可执行文件；无 scheme 的地址在打开时补 `http://`（同浏览器地址栏对裸 `host:端口` 的处理），否则 shell 会把裸串当文件路径而不交给浏览器；非 Windows 暂与 `launch` 一样 stub。
- `ResultItem` 增加 `Link { url, label, command_label }` 变体，渲染复用 Action 行布局（左主文右副文：`label` 即 "Open in Browser" 在左、`command_label` 即 "Command" 在右，`url` 不展示、仅供确认时打开）；确认回调增加 `Link` 分支，与 App 行一致地不追踪使用频率（URL 命中大多是一次性，暂不进 usage 表）。
- i18n 新增 `open-in-browser`（7 语言）。

### 2026-08-23（毛玻璃在高亮背景下的自适应蒙层）

- 症状：启动器用 `WindowBackgroundAppearance::Blurred`（Windows Acrylic / macOS vibrancy）后，亮色背景（白色 Word/浏览器窗口）上方的毛玻璃合成面偏浅——`BACKGROUND 0x202024` 以 `SCRIM_ALPHA 0.55` 叠在纯白模糊背景上合成亮度约 0.46，白字对比度仅 ~1.9:1，查询文字看不清。深色背景下合成面≈`BACKGROUND`（暗），无此问题；因此只在背景本身很亮时失效。
- 方案：蒙层不再固定 0.55，改为**按每次呼出时窗口背后背景的平均亮度自适应**（`LauncherState.scrim_alpha`，`render` 用 `self.state.borrow().scrim_alpha` 绘制）。Windows 平台 `platform::show` 改为：`position_centered` 定位 → `sample_backdrop_brightness` 采样 → `ShowWindow(SW_SHOW)` → `force_foreground`，返回 `Option<f32>`（非 Windows 返回 `None`）；采样在窗口仍隐藏时进行，读到的是真实背景而非启动器自身。`show_window` 用 `adaptive_scrim_alpha(brightness)` 算出本次透明度写入共享状态，下一帧生效。
- 采样：`GetDC(NULL)` 取虚拟屏 DC，按窗口矩形在 12×6 网格上 `GetPixel`，跳过 `CLR_INVALID`（0xFFFFFFFF）样本，各点转 Rec.709 相对亮度（sRGB 线性化）取平均。`GetWindowRect` 与屏幕 DC 同为物理像素坐标系，多显示器一致。实测对屏幕 DC 逐个 `GetPixel` 一次呼出约 2.2s，会卡住呼出，故改为**单次 `BitBlt`（SRCCOPY）把窗口矩形整块拷贝进 `CreateCompatibleBitmap`/`CreateCompatibleDC` 的内存位图**，再对内存位图做同样的 12×6 `GetPixel` 采样——GDI 硬件加速的块拷贝 + 内存读取，呼出恢复亚 100ms（`steward-app` 基准首呼 55ms）。
- 选取规则：`adaptive_scrim_alpha` 求"合成面亮度 = α×BACKGROUND 亮度 + (1-α)×背景亮度 ≤ `SCRIM_TARGET_LUMINANCE 0.10`"的最小 α，夹在 `SCRIM_ALPHA 0.55`（地板，暗背景下保持原有毛玻璃观感）与 `SCRIM_ALPHA_MAX 0.90`（上限，再高背景几乎不可见、读作实体面板）之间。纯白背景 → 0.90（合成面 ~0.113，白字对比 ~6.4:1，WCAG AA）；黑/深色桌面 → 0.55（外观与之前完全一致）；中间亮度连续过渡。
- 权衡与限制：上限 0.90 时背景只透出 10%，亮背景下的毛玻璃效果较弱，但保住可读性优先；暗背景（最常见）观感不变。采样仅在每次呼出（显示）时执行一次，不做拖动/可见期间的持续采样——拖动到亮背景上方不会中途变暗（可后续加拖拽结束重采样）。
- 追加（同一议题的选中行对比度）：蒙层抬升后整条栏变亮，固定白 0.10 的选中 wash 在亮背景下看不清。两处修正——(1) 选中行补上设计记录里本就该有、但一直没渲染的 accent 左描边（`list_active_border` accent 0.6，2px；每行都预留透明左边框以免选中行内容位移），这是与背景亮度无关的强选中标记；(2) wash 跟随蒙层自适应：`adaptive_selection_wash` 从 `SELECTION_WASH 0.10`（蒙层地板）线性升到 `SELECTION_WASH_MAX 0.20`（蒙层上限），纯白背景下选中行与邻行差约 +40 灰阶（原 +20）。wash 通过 `ResultList::render` 的 `selected_wash` 参数每帧推入列表（与 `max_height` 同法），不污染全局主题（设置窗口等其他 gpui-component 列表仍用固定 0.10）。
- 修订：accent 左描边在实际使用中被视为多余装饰，已移除（仅保留自适应 wash 作为选中标记）。设置窗口快捷键 `Ctrl+,` 不再是全局快捷键：设置页面里仍可改绑定，但只持久化、不再注册进全局热键管理器；启动器可见且聚焦时由 `handle_key` 匹配（`keystroke_to_hotkey` 转 `HotKey` 后与 `settings_hotkey` 比对）调用 `open_settings_window_from_launcher` 打开设置。`HotkeyField::Settings` 在 `apply_hotkey` 中走"仅持久化"分支。

### 2026-08-23（内置计算器）

- 启动器输入框支持直接求值：当输入归一化（`×÷−（）` 等 IME 字符映射到 ASCII）后含至少一个二元运算符（`+ - * / % ^`）、整串可解析且结果为有限值时，在结果列表顶部插入一条计算行，无需插件。裸数字（如 `42`）与纯符号（如 `-5`）不触发，避免劫持普通搜索。
- 求值器放在 `steward-core-engine`（无 UI 依赖、可单测）新增的 `calc` 模块，自实现递归下降 parser（expr/term/factor/power/atom，`^` 右结合），不引入第三方求值 crate——符合"不重复造轮子但也不重复现有依赖职责"的依赖政策，且保持依赖面最小。`format_value` 将整数无小数输出、浮点截 10 位去尾零（`0.1+0.2` → `0.3`）。
- 结果列表抽象为 `ResultItem` 枚举（`App(AppEntry)` | `Action { title, subtitle }`）：计算行标题为结果值、副标题为原始算式，无图标；`set_results`/`on_confirm` 同步改为携带 `ResultItem`，确认回调签名从 `Fn(usize)` 改为 `Fn(usize, &mut App)`（利用 `Context<T>: DerefMut<Target=App>` 直接传 `cx`），使 App 侧能在回调内访问剪贴板。
- 确认行为：Enter → 现有 `after_confirm` 清空输入并隐藏窗口；点击计算行 → 复制结果到剪贴板（`App::write_to_clipboard(ClipboardItem::new_string(...))`），保持启动器打开（与 App 行点击行为一致，等失焦再隐藏）。回调内不直接 `handle.update` 隐藏窗口，避免与窗口更新栈的重入问题。计算行需要 `max_w` 副标题截断以复用现有右列布局。

### 2026-08-23（Tinycast 配色）

- 主题从 Catppuccin Mocha（蓝灰底 + 蓝色 accent）改为 tinycast 风格（https://github.com/abue-ammar/tinycast）：启动器与设置面板改为中性深色表面 + 白色 alpha 墨阶 + 紫罗兰品牌色（violet `0x863bff`）。
- 新建 `steward-ui-components::palette` 作为唯一配色来源：`BACKGROUND 0x202024` / `BACKGROUND_ALT 0x2c2c31` / `BORDER 0x4d4d50` / `FOREGROUND 0xffffff` / `MUTED_FOREGROUND 0x79797c` / `SELECTION 0xffffff`（使用时施加 0.10）/ `HOVER 0xffffff`（0.05）/ `ACCENT 0x863bff`。因启动器窗口必须不透明（明暗系统模式下外观必须一致，自己拥有表面色），tinycast 的黑 40% 蒙层折成实体表面色，白色 alpha 档按 tinycast 定义预混在表面之上。
- 选中行/悬停行改为白色 alpha 底色（`list_active` 白 0.10、`list_hover` 白 0.05），与 tinycast 一致；`list_active_border` 保留 accent 0.6，选中行左侧保留 accent 描边以区分悬停；输入框文本选区保持 accent 0.35（否则设置页文本选区不可见）；primary 前景改为纯白（原近黑 `0x11111b`）。
- `ACCENT_PRESETS` 由 4 色扩为 5 色，新增紫罗兰 `0x863bff` 为默认色；i18n 新增 `settings-theme-violet`（7 语言）。托盘图标用色（`#89b4fa on #232332`，`scripts/generate-icons.py`）本次不改。

### 2026-08-23（全局快捷键组 + 设置快捷键）

- 热键设置收拢为独立"快捷键"选项组（`SettingGroup::title`），原"全局唤醒快捷键"改名"全局快捷键"（`settings-summon-hotkey` → `settings-global-hotkey`），并新增"设置快捷键"（`settings-settings-hotkey`），默认 `Ctrl+,`（`HotKey::new(CONTROL, Code::Comma)`，持久化串 `control+Comma`），全局按下即打开/聚焦设置窗口。
- 引入 `HotkeyField` 枚举（`Summon` / `Settings`），承载热键的存储键（`summon_hotkey` / `settings_hotkey`）、当前生效值（`LauncherState.summon_hotkey` / `settings_hotkey`）与默认值；`apply_summon_hotkey` 泛化为 `apply_hotkey(state, field, hotkey)`（注销旧键 → 注册新键 → 按字段持久化 → 失败回滚旧键），录制拦截逻辑按 `recording: Option<HotkeyField>` 复用同一字段渲染。
- `setup_global_hotkey` 同时注册两个热键；设置热键先试持久化值，失败（被占用或与唤醒热键撞键）回退默认，两者皆失败则置 `None`（该键项显示默认值）。
- 事件路由改为按 `HotKey::id()`（由修饰键位 + 主键确定的整数）比对 `event.id` 区分唤醒/设置，不再无条件切换启动器；`toggle_settings_window` 从托盘 `MENU_SETTINGS` 分支抽为公共函数，菜单与热键共用。
- i18n 调整（7 语言）：`settings-summon-hotkey-recording` → `settings-hotkey-recording`（两字段共用），新增 `settings-hotkeys` / `settings-settings-hotkey`。

### 2026-08-23（全局激活快捷键设置项）

- 唤醒热键从硬编码 `Ctrl+Alt+Space` 改为可配置：设置面板"通用"页新增"全局唤醒快捷键"项，点击按钮进入录制态，下一次在该窗口内按下的组合键即为新热键（`Esc` 取消，仅修饰键按下忽略）；要求至少含 Ctrl/Alt/Shift/Win 之一，避免全局热键劫持单键输入。
- 持久化：`HotKey::into_string()` 的字符串（如 `control+alt+Space`，可经 `FromStr` 往返）存入 SQLite `settings` 表 `summon_hotkey` 键；启动时读回注册，解析失败/注册冲突（已被其他应用占用）回退默认 `Ctrl+Alt+Space`。
- `GlobalHotKeyManager` 从 `Box::leak` 改为存入 `LauncherState`（`hotkey_manager` + 当前 `summon_hotkey`），由设置窗口在事件循环线程上"注销旧键 → 注册新键 → 持久化"（`apply_summon_hotkey`），新键注册失败时恢复旧键、不落盘。管理器仍须在事件循环线程创建（Windows 隐藏窗接收 `WM_HOTKEY`）。
- 键捕获用 GPUI `App::intercept_keystrokes`（在所有动作/事件机制之前触发，`stop_propagation` 可阻止组合键落入设置控件），在 `open_settings_window` 建窗闭包内注册、按 `window_handle()` 限定为设置窗口，`Subscription` 存入 `SettingsApp._hotkey_subscription` 随窗口关闭自动注销。
- 键映射：GPUI `Keystroke.key`（小写逻辑键，如 `space`/`a`/`f9`）→ `HotKey` 解析 token（`Space`/`A`/`F9`），修饰键字段 control/alt/shift/platform → `ctrl`/`alt`/`shift`/`super`；modifier-only 与不可映射键返回 `None`。`format_hotkey` 输出人类可读标签（`Ctrl + Alt + Space`，Win 键前置）。
- i18n 新增 `settings-summon-hotkey` / `settings-summon-hotkey-recording`（7 语言）。

### 2026-08-20（设置面板组件化 + 托盘菜单收拢）

- 设置窗口改用 `gpui_component::setting` 的 `Settings` 组件：左侧可搜索/缩放的侧栏 + 右侧页面，页面由 `SettingPage → SettingGroup → SettingItem → SettingField` 层级组成。当前两页——"通用"（开机自动启动 Switch，读注册表真值、写后以实际生效状态重渲染）与"关于"（版本号）；`Settings` 内置搜索、重置按钮（`default_value(false)` 使开启自启后可一键复位）。窗口改为 720×440、可缩放。
- 组件栈初始化：`steward_ui_components::init_components` 从仅初始化主题改为调用 `gpui_component::init` 完整初始化（global state / root / popover / menu / list 等），`Settings` 的搜索输入、下拉、tooltip 依赖这些全局；设置窗口根视图包一层 `gpui_component::Root`（弹层/通知/tooltip 的宿主，`Settings` 的 dropdown 与 reset tooltip 需要）。app crate 直接依赖 `gpui-component.workspace`。
- 托盘菜单收拢：移除"开机自动启动"勾选项及 `MENU_AUTOSTART` 路由、`AutostartItem` 句柄跨窗口同步机制，菜单只剩"设置/分隔线/退出"；自启开关统一收进设置面板（`set_autostart`/`autostart_enabled` 注册表逻辑保留在 app crate）。
- 托盘菜单深色：原生菜单无法套 GPUI 主题，改在启动早期调用 `uxtheme` 未文档化 API（序数 135 `SetPreferredAppMode(ForceDark)` + 136 `FlushMenuThemes`，与 win32-darkmode/tao 同法）强制进程级深色，使托盘右键菜单与设置窗口标题栏跟随深色；`windows-sys` 增加 `Win32_System_LibraryLoader` feature。
- i18n 新增 `settings-general` / `settings-general-description` / `settings-startup` / `settings-autostart-description` / `settings-about` / `settings-about-description` / `settings-version`（7 语言）。

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

- 应用扫描（Windows 优先）：`core-engine` 的 `WinAppsScanner` 遍历当前用户与全体用户的开始菜单 `Programs` 目录，递归收集 `.lnk`，经 ShellLink COM（`IShellLinkW`/`IPersistFile`）解析目标；`GetPath` 不带 `SLGP_RAWPATH`（flags 0）展开 `%windir%` 等环境变量；目标为空的 shell 命名空间快捷方式（如控制面板）回退保留 `.lnk` 本身，由 ShellExecute 解析启动。另用 `IShellItem` 枚举 `shell:AppsFolder` 补充 UWP 应用（计算器、设置、画图、终端等），路径归一化为 `shell:AppsFolder\<AUMID>`，按名称去重（保留 `.lnk`/exe 条目）；其他平台返回空 `NoopScanner`（M4 补齐）。
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
- 应用图标：`app_icons`（Windows）用 `SHGetFileInfoW(SHGFI_ICON | SHGFI_LARGEICON)` 取 exe 的 shell 大图标，`DrawIconEx` 画进 32 位 DIB 保留透明通道，转 RGBA 后用 `image` 编码 PNG 包装为 `gpui::Image`；按路径缓存（入口先展开 `%VAR%`），搜索结果的全部行在后台线程异步补全图标后经 `set_icons` 刷新，滚动到任意行都有图标；`shell:` 的 UWP 别名当前取不到 shell 图标（留待后续）。行左侧以 24 逻辑 px 渲染；行右侧显示本地化类型标签（`application`，如"应用"）。
- 中文输入：启动器查询框实现 GPUI `EntityInputHandler`（查询为单行文档，IME 组字走 `replace_and_mark_text_in_range`、提交走 `replace_text_in_range`），渲染时组字区加下划线；`LauncherInputElement` 在 paint 阶段注册输入处理器。Windows 上 GPUI 的 IME 上下文关联依赖 WM_PAINT 且会因输入处理器被临时取走而误禁用，故每帧调用 `ImmAssociateContextEx(IACE_DEFAULT)` 重新关联，保证拼音等输入法可组字；组字期间编辑/导航键（含 Enter/Esc）交给输入法，不作启动/隐藏处理。IME 提交在光标处插入、组字只替换组字区（带单测覆盖"你好 + 说话 → 你好说话"），不会整体替换查询。
- 布局与视觉：结果列表包在 `h(result_height) + mx(margin)` 固定高度容器内，输入行不再被压缩；`init_theme` 切换 `ThemeMode::Dark`；行内应用名与路径均省略号截断；保持不透明矩形窗口（圆角/透明化留待后续）。呼出时自动执行一次空查询（按使用频率排序），打开即显示常用应用。
- 结果列表滚动：结果行容器（`overflow_y_scroll` + `track_scroll`）随选中行滚动——`select_relative` 按行高与视口高度直接计算偏移并 `set_offset`，超过 8 行后用下/上键继续导航会自动滚动，选中行始终可见。

### 2026-08-20（MSI 安装包）

- 用 `cargo wix`（WiX Toolset v3.14）打 MSI：`crates/app/wix/main.wxs` 定义产品"Steward"（perMachine、MajorUpgrade、Program Files、Add/Remove 程序图标复用 `assets/icon.ico`、开始菜单快捷方式），`wix/License.rtf` 由 `cargo wix init` 从 MIT 许可生成。
- 打包入口 `scripts/package-msi.ps1`（`cargo wix -p steward-app -L -sval`；`-sval` 跳过 ICE 校验，避免在无 Windows Installer 服务的环境失败）。License.rtf 的路径写为 `../../crates/app/wix/License.rtf`，兼容 candle（包目录）与 light（`target/wix`）的不同工作目录。
- `release.yml` 的手动发布入口启用 Windows job：装 WiX 与 cargo-wix 后执行打包脚本并上传 MSI 产物。

### 2026-08-20（托盘右键菜单完善）

- 菜单结构：禁用态品牌项 `Steward` 置顶，分隔线后依次为"显示 / 隐藏 Steward"、"开机自动启动"（勾选项）、"刷新应用列表"，再一条分隔线后为"退出 Steward"；文案全部走 i18n（`app-toggle`/`app-autostart`/`app-refresh`/`app-quit`），品牌名 `Steward` 不翻译。
- 样式统一口径：系统托盘菜单是原生控件，muda 在 Windows 上只保存 `IconMenuItem` 的图标字段、不绘制位图，GPUI 深色主题无法套进原生菜单；因此"样式统一"落在文案、结构与状态上（品牌头、分组分隔线、勾选态），不尝试改原生配色/图标。
- 开机自启：Windows 读写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`（`RegGetValueW`/`RegSetKeyValueW`/`RegDeleteKeyValueW`，值名 `Steward`，值为 `current_exe()` 路径；`windows-sys` 增加 `Win32_System_Registry` feature）。启动时以注册表真值初始化勾选；点击后 muda 已自动翻转原生勾选，事件处理以注册表写入后的实际状态为准再同步勾选（`set_checked`），写失败时勾选回弹。非 Windows 平台该项禁用、事件为空操作（M4 补齐）。
- 刷新应用列表：重新执行 `platform_scanner().scan()` + `mark_seen`，重建 `Engine` 索引，并在已打开的窗口上以 `AnyWindowHandle::downcast::<StewardApp>` 拿到根视图后重跑当前查询（结果、图标、下拉高度一起刷新）；窗口未打开时无需处理，下次呼出本来就重新扫描。
- 事件路由：`setup_tray` 返回自动启动勾选项句柄（`Option<AutostartItem>`）注入 `setup_global_hotkey` 的 10ms 轮询任务，`MENU_AUTOSTART`/`MENU_REFRESH` 与既有 `MENU_TOGGLE`/`MENU_QUIT` 同路处理。

### 2026-08-20（下拉高度修复 + 深色托盘图标 + 托盘菜单精简与设置窗口）

- 下拉高度：根因是两个叠加的尺寸缺口——`launcher_height` 漏算上下两条 4px 拖拽边距，且 Windows 原生边框未计入尺寸，客户端区域总比设计矮约 14.5 逻辑 px，flex 列会压缩下拉容器、把最后一行裁掉（结果多时第 8 行只剩约 70%，即"匹配多反而矮"）。修复：`launcher_height = 60 + 2×4 + 结果高度`；Windows 平台 `platform::resize` 用 `GetWindowRect - GetClientRect` 实测边框差并加回，保证客户端区域精确等于请求的逻辑尺寸（实测 8 行时 client = 760×452）。`window.resize`（GPUI 自带）在该锁定版本内部 scale/边框处理不可靠，Windows 搜索路径改用平台层 resize，`WM_SIZE` 照常驱动 DirectX viewport 同步。
- 深色托盘图标：新增 `scripts/generate-icons.py`，把 `assets/steward.png` 的字形重绘为启动器强调蓝 `#89b4fa`，放在 `#232332` 圆角底上，输出 `assets/steward-dark.png`（macOS 用）与多尺寸 `assets/icon-dark.ico`；`app.rc` 增加资源 `2 ICON`，Windows 托盘改从资源 2 加载（资源 1/32512 仍是 exe/任务栏图标）。
- 托盘菜单精简：去掉顶部品牌项、显示/隐藏、刷新，菜单仅保留"设置"、"开机自动启动"（勾选）与"退出"（i18n `app-quit` 去掉了 Steward 字样）；左键单击托盘与全局热键的显隐功能不变。删除 `MENU_TOGGLE`/`MENU_REFRESH` 及 `refresh_apps`，i18n 移除 `app-toggle`/`app-refresh`，新增 `app-settings`（7 语言）。
- 设置窗口：托盘"设置"打开一个小 GPUI 窗口（340×180，普通窗口带标题栏，深色 `#232332` 背景），当前仅含"开机自动启动"开关；与托盘勾选项共享 `AutostartItem` 句柄，任一侧切换都同步另一侧。窗口句柄存 `LauncherState.settings_window`，关闭时按 `window_id` 精确清理，再次点击菜单可重新打开/聚焦。
- 修订（2026-08-23）：Windows 托盘图标改用资源 1（`assets/icon.ico`），与 exe/任务栏图标一致；`app.rc` 移除资源 `2 ICON`，`icon-dark.ico` 不再嵌入（`scripts/generate-icons.py` 仍生成该资产备用，macOS 托盘沿用 `steward-dark.png`）。

### 2026-08-20（键盘选中/滚动修复）

- 症状：键盘下键前 8 行内高亮正常下移，越过第 8 行后出现"页面在滚、选中行位置错乱/不再下移"。
- 根因：锁定版 GPUI 的 Windows 绘制路径里，`overflow_y_scroll` 容器的**裁剪是失效的**——子元素全部按原样绘制，不被容器裁剪。结果列表（161 行 × 48px）在滚动后从容器底部溢出，填满整个窗口（用 PrintWindow 逐帧验证：截图中可见 12~16 行、高亮出现在窗口中部/顶部等错误位置）。之前的 `select_relative` 用 `ScrollHandle::set_offset` + `track_scroll` 滚动容器，状态数学完全正确，但绘制层不裁剪导致视觉错乱。
- 修复：彻底去掉滚动容器/`ScrollHandle`，改为**手工可见窗口**：`ResultListState::visible_range()` 按当前选中项计算 8 行切片（`top = selected.saturating_sub(7)`），渲染时只输出这 8 行，容器不再有溢出，也不需要裁剪。高亮永远落在选中行上；超过 8 行后选中行停在底部、内容按行切换，符合启动器常规交互。同时修掉"首次按下键选中第 2 行"的问题：`selected=None` 视为 -1，第一次 Down 落在第 0 行。
- 验证：用 `PostMessage(WM_KEYDOWN/UP)` 注入方向键 + PrintWindow 逐帧截图测量高亮位置：第 1 次按下高亮在第 1 行（130–221）、第 7 次在第 7 行（706–797）、第 8/12/20 次均在第 8 行底部（802–893），第 8 行以下无任何溢出内容。

### 2026-08-20（可见态 resize 回归修复）

- 症状回归：搜索路径在 Windows 上改回同步 `platform::resize`（`SetWindowPos`）后，"窗口可见时结果数变化"会再次触发渲染错乱——输入约 3 个汉字或 5 个字母（结果数开始收窄、窗口需实时变高/变矮）时，输入文字消失或错位、结果区不渲染、下拉底部出现白色条；输入"控制面板"能出结果时同样在应用下方出现白色条。
- 根因：同步 `SetWindowPos` 在输入派发（`WM_CHAR`/`WM_IME_*`）调用栈内直接改窗口高度，会让锁定版 GPUI 的 Windows DirectX 渲染器 viewport 失步（08-19 决策已记录过该问题及异步 `window.resize` 修复，08-20 为精确客户端尺寸换回同步 resize 造成回归）。
- 修复：`search()` 在 Windows 分支不再调用同步 `platform::resize`，统一改用 GPUI 自身的异步 `window.resize`（前台 executor 执行 `SetWindowPos`，WM_SIZE 在事件循环干净点到达，渲染器正确同步）；后端用 `border_offset`（`GetWindowRect − GetClientRect`，与手测 13×7 边框同源）补偿原生边框，客户端尺寸仍精确等于 `760 × launcher_height(count)`。`LauncherState` 记录 `last_applied_height`，目标高度未变化时跳过 resize，避免 IME 组字期间每个拼音按键重复触发。同步 `platform::resize` 仅保留在呼出/隐藏路径（隐藏态 resize 已验证安全）；`after_confirm` 改为先隐藏再复位高度。
- 验证：`cargo test -p steward-app`（IME 单测）、`cargo clippy -D warnings`；注入脚本（Add-Type + `PostMessage` + PrintWindow）验证 5 字母/3 汉字/`控制面板` 场景下客户端尺寸 = 760 × `launcher_height(count)`、底部无白色像素、输入文字对齐可见。

### 2026-08-20（启动后重新呼出显示常用应用 + UWP 图标）

- 症状：选中应用回车启动后，再次呼出启动器结果列表为空（需按退格才会重新出现常用应用）；且列表里部分应用（主要是 `shell:AppsFolder` 的 UWP 应用）不显示图标。
- 根因一：`after_confirm` 清空查询与结果后直接隐藏，而"常用应用"只在窗口创建时播种一次（`open_launcher_window` 里的一次空查询），启动一次后每次呼出都只剩空条。
- 修复一：`after_confirm` 清空查询后重新执行一次空查询 `search()`，把常用应用重新播种进结果列表（窗口此时隐藏，尺寸同步由 `show_launcher` 在下次呼出时应用）。
- 根因二：图标提取只走 `SHGetFileInfoW`，对 `shell:` 解析名（UWP 别名）无法解析，恒返回 `None`。
- 修复二：`app_icons` 增加 `shell:` 分支——`SHCreateItemFromParsingName` + `IShellItemImageFactory::GetImage`（32px、`SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK`）取 32 位 DIB 位图，`GetObjectW` 取尺寸、`GetDIBits` 读 BGRA 转 RGBA 后 PNG 编码；函数自带 COM 初始化（`CoInitializeEx(STA)` + 配对 `CoUninitialize`），不依赖 GPUI 启动时的 OLE 初始化。app crate 新增 target 依赖 `windows = "0.61"`（与 core-engine 同版本）。
- 验证：单测覆盖 `C:\Windows\System32\notepad.exe` 与全部 `shell:AppsFolder` 别名（本机 104 个别名全部提取成功且含可见像素）；实机注入验证"选择→Enter 启动→热键重新呼出"后窗口恢复 773×459（8 行常用应用）。

### 2026-08-20（启动架构回退：恢复启动即加载 GPUI，保留缓存/构建优化）

- **回退惰性加载**：曾实现 Windows"常驻态去 GPUI"（原生托盘/热键 + 消息泵，首呼才 `application().run`），实测首呼 0.5–4.5 s（一次性 DirectX/DirectWrite/着色器初始化），对启动器不可接受；已回退为 `application().run` 启动即加载、隐藏建窗，呼出即 `ShowWindow`（首呼 14 ms）。双进程与"空闲预热"都只能转移这笔成本，无法消除，暂不采用。
- **保留：`QuitMode::Explicit`**：GPUI Windows 后端默认"最后一个窗口关闭即退出"，与托盘外壳模型冲突（Esc/失焦/Alt+F4 关窗会杀进程）；显式 `cx.set_quit_mode(QuitMode::Explicit)`，只有托盘"退出"才结束进程。
- **保留：共享状态上移**：`Engine`（索引 + pinyin haystack）、图标缓存、后台扫描结果通道在 `LauncherState`，窗口重建（Alt+F4 后重呼）不再重扫、不重取图标。
- **保留：扫描缓存优先**：SQLite `settings` 表新增 `last_scan` 时间戳（24h TTL）；冷启动直接读缓存建索引，缓存缺失/过期时后台线程全量扫描，结果经通道由前台轮询任务落库（`mark_seen` 改为按 path 增量 upsert + 删除消失项，替代全表重写）。
- **保留：窗口隐藏策略（实测后定）**：关闭窗口回收极少——DirectX 设备与 DirectWrite 字体是平台级资源，GPUI 会话内常驻——而重建窗口使二次呼出增加 ~150 ms；默认 `CLOSE_ON_HIDE=false`（隐藏窗口，二次呼出 21 ms）。
- **保留：查询路径**：`nucleo::Matcher` 从每次查询新建改为 `Engine` 内复用。
- **保留：release 构建**：`[profile.release] lto="thin"`、`codegen-units=1`、`panic="abort"`、`strip=true`，二进制 26.7 MB → 17.8 MB。

### 2026-08-27（Roadmap 对齐与重排）

- 规划文档 `docs/roadmap.md`（2026-08-27 由仓库外迁入）按仓库现状重排为 M0–M10：M0/M1 采用仓库定义（均 DONE）；M2 合并仓库 M2 与 roadmap 原 Plugin Kernel/QuickJS Runtime，落在现有 crate（`plugin-host` / `plugin-registry` / `ipc-protocol` / `plugin-runtime`）与 `packages/extension-api`；M3 合并仓库 M3 与 roadmap 原 GPUI UI Framework；M4 沿用仓库 Windows 支持；M5 沿用仓库插件生态基础设施 v1。排序原则：插件系统（M2–M4）优先，生态基础设施（M5）其次，AI/MCP/兼容层/市场（M6–M10）最后。
- 命名以仓库为准：插件 API 包名 `@steward/extension-api`（非 `@steward/api`）；manifest 字段遵循 `docs/plugin-manifest-spec.md`（`id/name/version/commands/permissions/isolation`，非 `capabilities/runtime/entry`）；Node 兼容在 M3 走 QuickJS 内 polyfill，Node Sidecar 仅作为 M9 兼容层（非核心）；WASM 运行时归入 M10，届时在 `[workspace.dependencies]` 引入。
- 后续 roadmap 与本文件（及 README 里程碑）变更需保持一致；本文件仍是架构唯一 ground truth，重大决策继续在此记录。
- 修订（同日）：AI Capability（M6）与 MCP 兼容层（M7）定位为产品核心能力，纳入正式规划而非愿景；AI Search（M8）同为 AI 能力层正式项；M9（Raycast/Vicinae 兼容）与 M10（WASM/市场）为后期扩展。

### 2026-08-27（M2 插件系统 v1 落地）

- 官方示例插件由 calculator 改为 calendar：内置计算器保留在 `core-engine::calc`（同步、零进程），插件端跑通端到端链路（TS -> esbuild IIFE -> QuickJS -> 启动器列表）；`packages/plugins/calculator` 删除。
- 传输层 M2 采用 stdio NDJSON（JSON-RPC 2.0，一帧一行）：Named Pipe 分支推迟到 M4，协议帧定义收敛在 `ipc-protocol`，换传输只需改 `plugin-host` / `plugin-runtime` 两侧的 IO 层。
- M2 最小 host function：`steward.clipboard.read/write`（arboard）与 `steward.showToast`；`network` / `fs.*` 被 manifest 识别但扫描时拒绝（"not supported in M2"）。toast 在 M2 仅打 stderr（事件已上抛），真实 toast 组件随 M3 UI 框架落地。
- 插件视图（View）M2 仅支持 `{ type: "list", items: [...] }` 且同步返回；异步命令显式报错（M3 支持）。`item.invoke` 走插件导出的 `select(itemId)`，选中后启动器保持打开。
- 查询代际（query generation）：每次搜索递增 gen，迟到的插件响应按 gen 丢弃，避免旧查询污染新结果。
- 崩溃恢复：共享池 / 专用进程崩溃后按指数退避（500ms 起、封顶 30s）重启并重载插件；隔离实例超时 / 超堆后 kill，下次调用按需重建。
- 开发期环境变量：`STEWARD_PLUGINS_DIR` 覆盖插件根目录（如指向仓库 `packages/plugins`），`STEWARD_PLUGIN_RUNTIME_BIN` 覆盖运行时二进制路径。
- 修订（同日）：应产品要求，插件视图契约在 M2 内扩展 `calendar` 类型（原定 M3 的 Grid 提前落地最小形态）：启动器把下拉区切换为月历网格（月份头 + 星期行 + 6 周网格，今天高亮），方向键移动选中日、回车/点击经 `item.invoke` 把日期交给插件；查询变化自动回到列表模式。`ui-components::calendar` 提供网格渲染与周历计算，app 负责视图解析、窗口加高与键盘导航。
- 修订（2026-08-27）：日历视图美化——左侧新增 ISO 周数列（`W` 前缀，按该行周四归属周，跨年月/跨年周正确），行高 44→52，网格包入圆角卡片（`BACKGROUND_ALT` 淡背景 + 1px 白 0.08 细边框），今天/选中日改为圆角药丸高亮；`CALENDAR_GRID_HEIGHT` 重定义为含卡片边框与内边距的外高（324→382），启动器窗口高度随常量自动更新，插件与视图契约不变。
- 修订（2026-08-27）：`command` 触发改为与应用搜索一致的模糊匹配——精确命令名（或命令名 + 参数）优先，其余按 nucleo 子序列得分排序；`cal` / `cldr` / `CAL` 都能命中 `calendar`，`calendarx` 这类仅是前缀更长的词仍不命中。needle 在匹配前统一小写（nucleo 要求调用方预归一化，否则大写查询失效），`prefix` / `regex` / `dynamic` 触发行为不变。
- 修订（2026-08-27）：多语言适配——manifest 命令新增可选 `keywords`（本地化别名，如日历插件声明 `"日历"`）；宿主路由把命令名、标题与每个关键词一起经 `core-engine::search_haystacks` 展开成拼音变体（全拼 / 空格全拼 / 首字母）做模糊匹配，因此 `日历`、`rili`、`rl` 都能命中 calendar，与 App 搜索同一套匹配词表。`keywords` 向后兼容（缺省为空数组），每命令 haystack 上限 32 条防膨胀。
- 修订（2026-08-27）：日历视图可固定——日历头部新增固定/取消固定按钮（`ToggleCalendarPin` 动作，固定与取消固定共用图钉图标，固定态以强调色/填充标识）。固定后启动器窗口失焦不再自动隐藏（窗口为 `PopUp` 置顶类型，保持可见），方便边看日历边做别的事；Esc / 热键隐藏或查询变化导致日历视图关闭时自动复位为未固定。前台窗口监听的兜底隐藏路径同样遵循固定状态。
- 修订（2026-08-27）：日历视图每格新增农历信息——公历日下方第二行显示农历标签，优先级为农历传统节日 > 公历节日 > 二十四节气 > 农历月初月份名 > 农历日名；宿主在 `ui-components` 引入 `tyme4rs`（workspace 依赖）负责公历→农历转换，插件与视图契约不变；节日/节气标签用强调色（`ACCENT`），其余用弱化色（`MUTED_FOREGROUND`）。
- 验证：`cargo fmt --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace`（121 项）全绿；`pnpm lint` / `pnpm typecheck` / `pnpm build` 全绿。

### 2026-08-28（插件视图弹出独立窗口）

- 能力定位：把「视图弹出为独立窗口」做成插件级可扩展能力，而不是耦合到 calendar 插件。插件在 manifest 的 `commands[].detachable: true` 声明某命令的视图可弹出；宿主提供通用「弹出 / 移回」机制与通用独立窗口宿主，calendar 是第一个启用者（`packages/plugins/calendar/plugin.json` 设置 `detachable: true`）。
- 契约与类型：`PluginCommand` 新增 `#[serde(default)] detachable: bool`（默认 false，序列化省略，`deny_unknown_fields` 下向后兼容）；`RouteHit` 新增 `detachable: bool`，由路由构建时从 `command.detachable` 填入。`extension-api` 的 `View`/`PluginModule` 与 `plugin-runtime` 不变。
- 通用窗口宿主：`plugin_panel_window.rs` 的 `PluginPanelWindow` 按 `view["type"]` 分发渲染——`calendar` 复用 `CalendarView`（点击日期 / 方向键 / Enter 走 `item.invoke`），`list` 复用 `ResultList`/`ResultItem::Plugin`（行确认走 `item.invoke`）。窗口为无边框、始终置顶、无任务栏、不可缩放的 `WindowKind::PopUp`，尺寸按视图类型推导（日历用 `CALENDAR_GRID_HEIGHT`，列表按行数）。
- 通用注册表：`LauncherState.panel_view_windows: HashMap<(plugin_id, command), AnyWindowHandle>` 取代原先的 `calendar_pinned`/单窗口思路；同命令只保留一个独立窗口，重复弹出仅聚焦已有窗口。`on_window_closed` 按 `window_id` 比对并清理，触发移回。
- 与启动器解耦：删除 `calendar_pinned` 对启动器失焦 / 前台切换隐显的豁免（`observe_window_activation`、foreground-watch 均不再因日历保持启动器），并移除 `toggle_launcher` 隐藏分支对该位的清零；独立窗口不受全局唤醒热键影响，主面板可继续搜索使用。
- 弹出 / 移回：日历内嵌网格头部的图钉按钮语义改为「弹出」（仅 manifest `detachable` 时显示，`CalendarView` 新增 `detachable` 门控）；detachable `list` 视图在启动器结果头部显示通用「弹出」按钮（`external-link` 图标，仅在恰有一个可弹出 list 面板时出现，避免歧义）。关闭 / Esc / 点击已弹出态按钮触发 `dock_panel_back`，把视图交还启动器主面板。
- 假设：弹出属于 UI 呈现能力，不作为 `permissions` 授予；弹出 / 移回不改动启动器查询；若用户在弹出态改了查询，移回时按当前查询显示，需重新触发才恢复该视图。

### 2026-08-28（M2 插件懒加载：冷启动与搜索只随实际激活数变化）

- 问题：此前 `plugin-host::set_plugins` 在启动时对每个插件逐个发 `plugin.load`（O(N) 次 bundle 求值），且共享池容量（默认 8）满后按 LRU 驱逐 isolate，但宿主 `conn.isolates` 仍保留被驱逐插件的旧 `isolate_id`——后续 `invoke` 会向运行时发送失效 isolate，恒返回 `PLUGIN_NOT_FOUND`，插件实际不可用。既违背「冷启动不随安装量线性」（启动做了 N 次 bundle 求值），也违背决策记录里的「下次调用按需重建」（驱逐后不再重载）。
- 方案：改为**延迟按需加载**。`set_plugins` 只做 `rebuild_routes` + 按插件集 spawn 共享池 / 各 dedicated 进程，不再发 `plugin.load`；isolate 在首次 `invoke`/`item.invoke` 时才 `ensure_loaded`（发 `plugin.load`），驱逐 / 超时 / 超堆被 kill 后再次命中会重载。
- 宿主新增 `loading_plugins`（在途 load）与 `queued`（等待加载完成后重放的 command/item 调用）。`handle_response` 对 `PLUGIN_NOT_FOUND` 走 `handle_stale_isolate`：清掉失效 isolate → 若未在加载则 `ensure_loaded` 一次并重入队（以 `loading_plugins` 防循环）；`plugin.load` 成功后 `flush_queued` 把积压调用转成真实请求；加载失败则把积压调用以错误事件上抛。连接崩溃（`handle_eof`）改用 `forget_conn_state` 清理该连接的 loading/queued/pending，重启后空连接由下一次调用懒加载。
- 验收：新增宿主级规模化回归测试（`plugin-host/tests/scaling.rs`）：N=100 下 `cmd0/cmd50/cmd75/cmd99`（远超共享池容量）均能正确懒加载出 list view，证明任意索引插件可用、不存在失效 isolate；并宽松断言 `set_plugins(N=100)` 与单次冷查询耗时不随安装量显著增长。`cargo test` / `cargo clippy -D warnings` 全绿。
- 测试稳定化：`isolate_pool` 单测在并行线程同时创建多个 QuickJS `Runtime` 时偶发失败（参数传入异常），生产服务循环单线程，故仅在本模块测试内加 `RUNTIME_TEST_LOCK` 串行化，属测试稳定化而非产物改动。

### 2026-08-28（M3 首轮：插件核心 API + 类型化 View + clipboard-history）

- 范围：按 roadmap 第一优先级落地「插件 API 覆盖面 + 声明式 UI 的最小类型化形态」，第二波再补 Node polyfill 与主题/动画打磨。
- UI 模型定为**类型化可序列化 View 描述符**（延续 `list`/`calendar`）：新增 `detail`（`title/subtitle/content[]`，块为 `text/code/separator`）与 `form`（`fields[]/submitLabel`），并为 `list`/`calendar`/`detail` 增加可选 `actionPanel`（`actions[]`）。宿主用原生 GPUI 渲染；`detail`/`form` 落在独立插件窗口，启动器内页仍以 `list`/`calendar` 为主。
- 插件回调通道：保留 `command`/`select(itemId)`；新增 `run(actionId, itemId?)`（`action.invoke`）与 `submit(values)`（`form.submit`）。`select` 可返回新视图（`item.invoke` 返回 `{ view? }`），宿主经 `HostEvent::ItemView` 弹出/替换独立窗口，实现列表→详情下钻。
- 运行时 host bridge 扩展：`clipboard.history()`（读宿主注入的本次 `command.invoke` 快照，受新权限 `clipboard.history` 门控）与 `per-plugin storage.*`（`<data_dir>/plugin-storage/<plugin_id>.json` 免权限 KV）。剪贴板历史由宿主 `ClipboardWatcher` 线程（arboard ~300ms）采集落 SQLite `clipboard_history`，插件只读注入快照。
- 权限：manifest 新增 `clipboard.history`；M3 放行集 = `clipboard.read`/`write`/`history`；`network`/`fs.*` 仍识别但扫描拒绝，文案改 `not supported in M3`。
- 官方插件 `clipboard-history` 落地：列表（置顶 pin）→ 选择出 `Detail` → ActionBar `copy`/`pin`（copy 用 `Clipboard.write`，pin 用 `LocalStorage`）；`permissions` 申明三件套。
- 决策：`LocalStorage` 走运行时文件型 KV、按插件 id 分文件、不新增权限；`detail`/`form` 的窗口内交互（表单文本输入、`set_view` 更新已有面板）与 `Grid`/`SearchBar`/Node polyfill/async `command()` 留待后续迭代；`ActionPanel` 为视图级共享动作条（作用于当前选中项）。
- 验证：`cargo fmt --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test --workspace` 全绿；`tsc --noEmit`（extension-api / calendar / clipboard-history）与两个插件 esbuild IIFE 构建通过。
