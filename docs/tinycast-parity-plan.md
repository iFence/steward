# Steward 对齐 Tinycast 实施计划

| 项目             | 内容                                       |
| ---------------- | ------------------------------------------ |
| 文档状态         | Active                                     |
| 上游目标         | Tinycast v0.12.11                          |
| 上游基准提交     | `58168bf7e0b721f7c5fbfc7f4e8a068a48cff446` |
| Steward 当前基线 | v0.3.0                                     |
| 正式验收平台     | Windows 11 23H2+ x64                       |
| 最后更新         | 2026-11-09                                 |

## 1. 目标与边界

本计划把 [Tinycast](https://github.com/abue-ammar/tinycast) v0.12.11 的用户可见能力拆成可发布的 Steward
里程碑。目标不是复制 Tinycast 的 Swift 实现，而是在 Steward 的 Rust、GPUI Fast、NSH 和签名插件架构上实现等价的
Windows 产品行为。

Tinycast 使用 AGPL-3.0，Steward 使用 MIT。实施时必须基于公开行为、产品规格和独立实现完成功能，不直接复制
Tinycast 源码、资源或内部架构。

### 1.1 成功标准

- Windows 11 23H2+ x64 用户可以只靠键盘完成应用启动、系统动作、剪贴板、窗口管理和插件命令等高频工作。
- 启动器在大量插件安装时仍只扫描路由元数据，不因安装数量创建隐藏 VM 或插件进程。
- AI、网络、进程和自动化能力默认关闭，必须经过明确的按需授权。
- 第三方插件获得高权限前，插件窗口与主进程隔离。
- 每个里程碑都有可验证的退出标准、性能数据和迁移/回滚路径。
- 除明确列为平台差异的能力外，Tinycast v0.12.11 的主要用户流程在 Windows 上有对应实现。

### 1.2 非目标

- 不承诺 ABI 级兼容 Raycast 扩展。
- 不模拟 Apple Intelligence、Apple Shortcuts、Secure Input 或 macOS Spaces。
- 不复制 Tinycast 的可执行文件、Swift 源码、签名资产或受限制资源。
- 不在本计划内支持 Windows 10、Linux、macOS 或 Windows ARM64 的正式验收。
- 不为了视觉相似而放弃 Windows 原生窗口、托盘、菜单、输入法和无障碍行为。

## 2. 上游能力与当前差距

### 2.1 Tinycast 能力基线

Tinycast 当前已经形成以下能力群：

- 启动器：应用、系统设置、收藏、别名、隐藏项、运行状态、应用退出/重启、搜索排序和分类搜索。
- Palette：嵌套页面、键盘导航、操作菜单、紧凑模式、主题、IME 和跨页面返回。
- 生产力：剪贴板历史、富文本/图片/文件/颜色、计算器、Emoji、词典、Notes、Snippets、Quicklinks 和自定义命令。
- 桌面自动化：窗口尺寸、位置、显示器移动、窗口布局、Window Rooms、窗口切换和菜单搜索。
- 系统集成：文件搜索、日历与会议、摄像头、听写、系统动作、应用卸载器和 Deeplinks。
- AI 与自动化：AI Chat、Quick AI、Quick Actions、BYOK、订阅/本地 CLI 路由、MCP、工具调用和聊天预设。
- 扩展：Raycast 扩展运行时、插件菜单栏命令、脚本导入和设置备份。
- 发布：应用内更新、签名、公证、Homebrew、设置文件和迁移。

### 2.2 Steward 当前能力

当前 v0.3.0 已具备：

- Windows Start Menu `.lnk` 和 `shell:AppsFolder` 应用扫描，包括 UWP 应用。
- 基于 `nucleo` 的模糊搜索、拼音全拼/首字母匹配、使用频率加权和 30 秒后台重扫。
- 基础四则运算、幂、取模、括号和 URL 打开。
- 托盘、开机自启、可录制呼出热键、语言、强调色和 GPUI Kit 设置窗口。
- NSH Rust 启动器；启动器不启动 JavaScript VM。
- 签名 `.nshpkg`、插件路由缓存、命令/前缀/正则触发和每插件一个 NSH 窗口。
- 极简宿主桥：剪贴板历史、关闭窗口、重复命令和主题变更。
- 三个官方插件：Calendar、Clipboard History 和 UI Showcase。
- 剪贴板后台轮询，只保存最近 50 条纯文本。

当前明确缺失或不完整：

- 无文件搜索、窗口管理、系统动作、应用卸载器、窗口切换器或菜单搜索。
- 无收藏、别名、逐项隐藏、逐命令热键、Hyper Key 或热键冲突管理。
- 无图片/文件/HTML/RTF 剪贴板、固定项、类型筛选或恢复粘贴目标。
- 无 Notes、Snippets、Quicklinks、自定义命令、快捷链接或系统日历集成。
- 无 AI、MCP、Quick Actions、听写、摄像头或 Deeplinks。
- 插件没有安装/更新/移除市场、用户能力授权、进程隔离或完整宿主 API。
- 设置页仅有语言、强调色、开机自启和呼出热键。
- 生产信任根、发布基准和干净 Windows 安装仍属于 NSH-4 待完成项。

### 2.3 功能差距矩阵

| 领域           | Tinycast v0.12.11                             | Steward v0.3.0              | 目标阶段  |
| -------------- | --------------------------------------------- | --------------------------- | --------- |
| 应用启动与搜索 | 完整启动器、分类、别名、收藏、应用动作        | 应用、计算器、URL、插件路由 | P1        |
| Palette 交互   | 多页面、操作菜单、紧凑模式、完整键盘流程      | 单层结果列表                | P1-P5     |
| 剪贴板         | 文本、图片、文件、颜色、固定、搜索、编辑、OCR | 50 条纯文本                 | P1-P5     |
| 系统动作       | 设置页、音量、麦克风、外观、锁屏等            | 无                          | P1        |
| 用户内容       | 命令、Quicklinks、Snippets、Notes             | 无                          | P2        |
| 窗口和导航     | 窗口命令、布局、Rooms、窗口/菜单搜索          | 无                          | P2-P5     |
| 文件与日历     | 本地索引、日历、会议和摄像头                  | 独立的农历日历演示插件      | P2-P3     |
| AI/MCP         | Chat、Quick Actions、Provider、MCP、Presets   | 无                          | P3-P5     |
| 扩展           | Raycast 运行时和大量宿主 API                  | 最小 NSH 窗口和宿主桥       | P2-P5     |
| 发布与数据     | 更新、备份、导入、设置文件                    | MSI 和签名包基础            | P0、P3-P5 |

## 3. 目标架构与公共契约

### 3.1 统一启动器模型

新增一个与 UI 无关的启动器条目模型，所有功能都贡献同一类条目：

```rust
struct LauncherItem {
    id: String,
    feature: FeatureId,
    section: LauncherSection,
    title: String,
    subtitle: Option<String>,
    aliases: Vec<String>,
    keywords: Vec<String>,
    icon: LauncherIcon,
    action: ActionId,
    visibility: Visibility,
    hotkey: Option<HotkeyBinding>,
}
```

约束：

- `id` 在来源生命周期内稳定，用于收藏、别名、排序、隐藏和热键引用。
- `ActionId` 是动作的唯一分发入口，搜索行、收藏槽、全局热键、Deeplink 和插件命令不得各自实现一套动作逻辑。
- 搜索继续使用 `nucleo`。只扩展候选生成和排序规则，不增加第二套模糊匹配器。
- 分区顺序和扁平选择索引由同一数据源生成，避免可见行与键盘选中行错位。
- 路由发现只读取元数据；启动器、设置和纯 Rust 页面不得创建插件 VM。

### 3.2 设置、热键与数据存储

- 设置项通过 `FeatureId` 和稳定 key 注册，包含默认值、是否允许备份、是否需要权限和所属设置页。
- 内置命令、系统动作、应用、用户内容和插件命令统一使用 `HotkeyBinding`。
- 热键注册失败不得静默覆盖；必须保留原绑定并显示冲突原因。
- SQLite 增加显式 schema version 和逐版本迁移。用户创作数据损坏时报告错误，绝不自动删除。
- 剪贴板、学习排序和索引快照属于可再生数据；Notes、Quicklinks、Snippets、命令和布局属于用户创作数据。
- API Key、OAuth token 和敏感请求头存入 Windows Credential Manager 或 DPAPI 保护存储，不进入 SQLite 明文、日志或备份。

### 3.3 Windows 平台层

Windows 专用能力集中到一个可测试的平台层，避免散落在 UI 和搜索代码中：

- 应用发现、进程和运行状态。
- 顶层窗口、前台窗口、显示器和工作区。
- 剪贴板格式、序列号、监听和敏感内容过滤。
- 文本选择读取、文本注入和剪贴板安全恢复。
- Windows 设置页、系统动作和通知区。
- Windows Search、日历、摄像头、麦克风和电源/会话事件。

线程约束：

- UI 线程不执行插件验证、文件扫描、日历查询、OCR、网络请求或窗口枚举。
- COM 初始化和线程套间归属必须集中管理。
- 所有显示器坐标使用同一逻辑/物理坐标转换边界，支持混合 DPI 和负坐标显示器。

### 3.4 插件与能力授权

`plugin.json` v2 继续只承担路由元数据。宿主能力由签名包声明、发布策略限制和用户授权三者取交集：

```text
签名包声明能力 ∩ Steward 发布策略 ∩ 用户授权 = 实际能力
```

新增 `CapabilityGrant`：

```rust
struct CapabilityGrant {
    plugin_id: String,
    publisher_fingerprint: String,
    version_range: String,
    capability: Capability,
    scope: CapabilityScope,
    granted_at: i64,
    revoked_at: Option<i64>,
}
```

约束：

- 高权限能力默认拒绝，首次使用时按功能、域名、目录、可执行文件或动作类型授权。
- 授权界面必须展示调用方、能力、作用域、有效期和撤销入口。
- 每次高权限调用写入结构化审计日志，日志不包含密钥或完整敏感内容。
- 第三方插件获得网络、进程、文件系统或自动化能力前，必须运行在独立插件宿主进程中。
- 插件崩溃、超时或取消不得拖垮启动器；插件进程退出后不得遗留隐藏 VM。

扩展宿主 API 按能力分组演进：

| 分组                  | 能力                             |
| --------------------- | -------------------------------- |
| `steward.clipboard.*` | 历史、读取、写入、监听           |
| `steward.storage.*`   | 插件私有键值存储                 |
| `steward.apps.*`      | 应用发现、启动、退出、重启       |
| `steward.windows.*`   | 窗口枚举、激活和布局动作         |
| `steward.files.*`     | 授权目录读取、路径打开、文件选择 |
| `steward.network.*`   | 受限 HTTP 和域名白名单           |
| `steward.process.*`   | 受控可执行文件和参数             |
| `steward.ai.*`        | 经用户授权的模型调用             |
| `steward.ui.*`        | 通知、确认、浮层和主题事件       |

### 3.5 Deeplink 与备份契约

Deeplink 使用以下形式：

```text
steward://run/<action-id>?input=<encoded-input>
steward://plugin/<plugin-id>/<command>?input=<encoded-input>
```

- 外部应用首次调用某类动作时要求确认。
- 不允许 Deeplink 绕过自定义命令确认、插件授权或危险系统动作确认。
- 设置文件与备份使用版本化 schema。
- 默认不备份 API Key、OAuth token、权限授权、危险功能开关和机器特定路径。
- 导入自定义命令、Snippets 和 Quicklinks 时只增加内容，不自动授予能力。

## 4. 版本映射与里程碑

采用正式能力基线而不是逐个 beta 机械追赶：

| 阶段 | 对标基础          | Steward 目标版本 | 核心结果                           |
| ---- | ----------------- | ---------------- | ---------------------------------- |
| P0   | NSH-4             | v0.3.x           | 发布基座和产品模型                 |
| P1   | Tinycast v0.7.5   | v0.4.0           | 每日可用的 Windows 启动器          |
| P2   | Tinycast v0.9.7   | v0.5.0           | 用户内容、桌面自动化和原生插件生态 |
| P3   | Tinycast v0.10.23 | v0.6.0           | AI、MCP、系统和发布能力            |
| P4   | Tinycast v0.11.19 | v0.7.0           | 听写、AI CLI 和高频工作流深化      |
| P5   | Tinycast v0.12.11 | v1.0.0           | 当前功能闭环和 Windows parity      |

### 4.1 P0：Windows 基座与 NSH-4 收口

目标：先让后续功能建立在可发布、可诊断、可隔离的基础上。

交付：

- 完成生产 Ed25519 信任根和 CI 发布密钥部署。
- 完成干净安装、升级、修复和卸载验证。
- 测量冷启动、RSS、呼出延迟、插件首帧、焦点延迟和隐藏 VM 数。
- 引入统一 `LauncherItem`、`ActionId`、热键注册、设置注册和 schema migration。
- 建立 Windows 11 23H2+ x64 自动化与人工验收矩阵。
- 建立能力授权表、撤销流程和审计日志骨架。

退出标准：

- NSH-4 所有发布门禁关闭。
- 生产信任根下的官方插件可安装和启动。
- 现有应用搜索、托盘、热键、设置和插件无回归。
- release 构建不再依赖开发信任根。

### 4.2 P1：对齐 Tinycast v0.7.5

目标：把 Steward 从“能搜应用”升级为每天可用的启动器。

交付：

- 搜索分区：Favorites、Applications、System Settings、Commands、Emoji。
- 应用别名、逐项隐藏、收藏、空查询排序、frecency 和分类搜索。
- 运行状态圆点，以及打开、关闭、强制关闭、重启和定位到资源管理器。
- Windows 设置页搜索、锁屏、睡眠、音量、麦克风、明暗模式等系统动作。
- 每项热键绑定、冲突检测、恢复默认和一键清除。
- 剪贴板 v1：文本、URL、颜色识别、历史、复制、粘贴、固定、删除和类型筛选。
- 计算器历史、结果复制和更稳定的行内计算交互。
- Emoji 与字符选择器：本地数据、关键词、分类和最近使用。
- Hyper Key v1：全局 Caps Lock/修饰键映射和组合键。

退出标准：

- 启动、应用动作、系统动作和剪贴板可只靠键盘完成。
- 热键冲突不会静默覆盖。
- 剪贴板关闭后停止监听并关闭其数据库连接。
- 普通查询和空查询排序都有可复现测试。

### 4.3 P2：对齐 Tinycast v0.9.7

该阶段拆成三个可独立验收的子阶段，可以发布预发布版本，但最终共同达到 v0.5.0 退出标准。

#### P2.1 用户内容层

- 自定义命令：PowerShell、CMD、WSL 和显式解释器。
- 命令目录、最多三个参数、超时、确认、流式输出和停止。
- 参数通过进程参数传递，不拼接进 shell 文本。
- Quicklinks：URL、路径、搜索和 Deeplink，支持参数、剪贴板、选中文本、日期和 UUID 模板。
- Snippets：Markdown 存储、模板变量、参数、启动器插入、热键和自动关键词扩展。
- Notes：本地 Markdown 文件、浮动编辑器、列表、全文搜索和快捷键。
- 所有用户内容支持创建、编辑、复制、删除、导入导出和统一图标来源。

#### P2.2 Windows 自动化层

- 窗口管理：最大化、半屏、四分屏、三分屏、居中、调整尺寸、边距、显示器移动、还原和循环。
- 窗口布局：保存应用、尺寸、位置和显示器组合，并一次性恢复。
- 文件搜索 v1：使用 Windows Search `SystemIndex`，默认只查询用户配置目录。
- Windows Search 未运行时显示明确状态和修复入口，不静默退回全盘遍历。
- 日历 v1：通过 Windows `AppointmentStore` 请求权限并读取下一次会议、当日日程和创建事件。
- 对系统日历不提供的字段，后续通过 Microsoft Graph/Outlook 适配补充。
- 应用卸载器：先退出应用，再删除安装目录和可确认的缓存/配置残留。
- 窗口切换器和菜单搜索：窗口通过 Win32 枚举，菜单通过 UI Automation 读取。

#### P2.3 原生插件生态

- 插件安装、更新、移除、回滚和签名状态界面。
- 引入独立 `steward-plugin-window` 宿主进程，每个插件窗口一个进程。
- 主进程只管理目录、路由、权限、安装和窗口；插件崩溃不能拖垮启动器。
- 实现进程退出、超时、取消、资源限制、结构化日志和诊断包。
- 实现文件、网络、进程、应用、窗口、剪贴板和 AI 能力的授权界面。
- 仅导入 Raycast Quicklinks、脚本命令和基础元数据，不承诺完整 Raycast runtime。

退出标准：

- 用户可在 UI 中完成插件全生命周期。
- 恶意或崩溃插件不能读取未授权资源或影响主进程。
- 没有打开窗口时插件 VM 和插件宿主进程均为零。
- Windows Search、日历和窗口 API 都有明确不可用降级路径。

### 4.4 P3：对齐 Tinycast v0.10.23

目标：加入 AI、MCP 和系统级集成，补齐从启动器到工作流的闭环。

交付：

- AI Provider 层：OpenAI-compatible、Anthropic、Gemini、OpenRouter 和本地兼容服务。
- 流式 Quick AI、AI Chat、会话存储、停止生成、重试、模型选择和系统提示。
- API Key 仅进入 Windows Credential Manager；远程端点要求 HTTPS，localhost 例外。
- Quick Actions：修复语法、改写、翻译和总结。
- MCP 客户端支持 stdio 和 HTTP，工具调用要求授权、超时、取消和审计。
- 摄像头预览、会议提醒、托盘下一会议摘要和会议链接打开。
- 窗口布局高级能力、自定义尺寸、窗口切换器和菜单搜索完善。
- 带签名和哈希校验的应用内更新，支持下载、退出时安装和失败回滚。
- 分类备份、恢复合并、设置文件镜像和危险内容导入警告。
- 计算器升级为类型化表达式：单位、货币、日期、时间跨度、时区和自然语言日期。

退出标准：

- AI 完全默认关闭；未授权时不发送网络请求。
- 取消、超时和 Provider 错误都可诊断。
- 更新包同时通过 Authenticode、SHA-256 和发布清单签名验证。
- MCP 服务作用域和工具调用可逐项关闭。

### 4.5 P4：对齐 Tinycast v0.11.19

目标：深化高频工作流，让自动化和 AI 真正具备日常使用价值。

交付：

- 本地听写 v1：麦克风选择、按住/切换热键、离线模型和文本注入。
- 听写、Snippets 和文本注入共享目标窗口解析及剪贴板安全恢复。
- AI 支持已安装 CLI 路由、模型发现、每对话模型和每 Quick Action 模型。
- 文件搜索加入最近文件、类型筛选、预览、打开所在目录、拖动和文件/目录收藏。
- Quicklinks、Snippets、自定义命令、窗口布局和 Rooms 使用统一的启动器表单。
- 设置文件双向同步，外部错误不得覆盖当前有效配置。
- 导入 Windows 常见脚本目录及 Raycast 脚本元数据。
- 补充高 DPI、多显示器、IME、键盘布局、无障碍名称和键盘焦点测试。

退出标准：

- 高频路径不依赖鼠标。
- 文件搜索在索引服务不可用时行为明确。
- 听写和自动展开在提权窗口、UAC 安全桌面等不可注入场景中安全失败并解释原因。

### 4.6 P5：对齐 Tinycast v0.12.11

目标：完成当前 Tinycast 主要能力的 Windows 闭环，并达到 Steward v1.0.0。

交付：

- Window Rooms：保存和恢复窗口集合，在其 Windows 桌面内隐藏其他窗口并切换布局。
- 紧凑启动器模式、可配置紧凑快捷键和文件/目录收藏。
- Deeplinks、外部输入、首次授权和调试命令。
- AI Presets、Reasoning Effort、Web Search、图片/PDF/文本附件和上下文限制。
- MCP 多服务器、每会话工具开关、OAuth/凭据存储和工具调用展示。
- 剪贴板扩展为文本、HTML、RTF、图片、文件、颜色和固定内容。
- 剪贴板搜索、编辑、类型筛选、拖动和可选本地 OCR。
- 双语词典、更完整的自然语言计算和 Emoji 数据更新。
- 通知中心、托盘状态、更新 Pill、诊断页面、权限状态和崩溃恢复。
- 完成 MSI、WinGet、代码签名、自动更新和回滚发布链。

退出标准：

- 除明确标记为平台差异的能力外，Tinycast v0.12.11 的主要用户流程可在 Windows 11 x64 完成。
- 所有功能都可以关闭并完整释放线程、监听器、数据库连接、VM 和插件进程。
- 干净虚拟机上的安装、升级、修复、卸载和更新回滚全部通过。

## 5. Windows 与 macOS 差异

| Tinycast 能力                        | Windows 决策与原因                                                                                         |
| ------------------------------------ | ---------------------------------------------------------------------------------------------------------- |
| 完整 Raycast 扩展兼容                | 不承诺 ABI 兼容。以签名 NSH 原生插件为主，只保留有限导入和未来可选 sidecar；上游也已停止扩大完整兼容范围。 |
| Apple Intelligence                   | 不模拟。使用 BYOK、OpenAI-compatible 本地服务、本地模型或已安装 CLI。                                      |
| Apple Shortcuts                      | 不强行复刻 Shortcuts 库。使用自定义命令、PowerShell/WSL、COM 自动化和可选 Power Automate 适配。            |
| macOS Spaces / Secure Input          | 不承诺完全等价。Windows 虚拟桌面通过公开 API 提供有限能力，Rooms 基于当前桌面窗口集合实现。                |
| macOS 菜单栏                         | 使用 Windows 通知区和托盘菜单；限制插件常驻图标数量，避免通知区膨胀。                                      |
| Liquid Glass / 精确视觉样式          | 行为对齐，视觉使用 Windows 11 Mica/Acrylic、系统圆角和原生标题栏。                                         |
| 窗口管理                             | 基础尺寸、位置、显示器移动和还原可靠实现；虚拟桌面与特殊应用限制必须明确提示。                             |
| EventKit 日历                        | 使用 Windows AppointmentStore，Microsoft Graph/Outlook 作为增强；会议链接质量取决于提供方。                |
| Dictionary.app                       | 使用本地词典数据，不依赖 macOS 系统词典。                                                                  |
| 自动文本展开/文本注入                | 可以实现，但无法作用于提权窗口、UAC 安全桌面、部分游戏反作弊和受保护应用。                                 |
| 按实体键盘分别映射 Hyper Key         | 低层键盘钩子无法稳定识别具体设备。只承诺全局 Hyper Key，按键盘区分不进入 v1。                              |
| Homebrew、Developer ID、notarization | 使用 MSI、WinGet、Authenticode、SHA-256 和发布清单签名。                                                   |

## 6. 权限、安全与隐私

- 高权限能力默认拒绝，按插件、功能和范围授权。
- 发布策略是硬上限，插件元数据不得自行提升权限。
- 网络授权至少包含目标域名；重定向后的目标也必须重新校验。
- 进程授权至少包含可执行文件、参数边界、工作目录和超时。
- 文件授权使用明确的 root 和规范化路径，拒绝路径穿越及逃逸符号链接。
- 自定义命令参数不作为 shell 文本拼接。
- 剪贴板排除前台应用和 Windows 敏感格式；日志不得记录完整剪贴板内容。
- AI 请求显示目标 Provider；密钥、Authorization 头和敏感请求头统一脱敏。
- 外部 Deeplink、插件命令和危险系统动作均经过对应确认门禁。
- 备份和设置文件不携带密钥、权限授权或可自动开启的敏感监听功能。

## 7. 测试与发布门禁

### 7.1 基础检查

每个里程碑至少运行：

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm lint
pnpm format:check
pnpm typecheck
pnpm build
```

官方插件还需运行各自的 `nsh test`、打包、签名和验证流程。

### 7.2 功能测试

- 搜索：精确、别名、拼音、错字、优先级、frecency、空查询、隐藏项和插件路由。
- 存储：所有历史 schema 升级、损坏数据库、未完成迁移和回滚路径。
- Windows：多显示器、不同 DPI、休眠唤醒、用户切换、资源管理器重启和 IME。
- 剪贴板：文本、HTML、RTF、图片、文件、颜色、超大内容、重复内容和敏感内容。
- 窗口：负坐标、不同尺寸屏、最小尺寸限制、全屏、无响应窗口和布局恢复。
- 插件：未签名、版本不匹配、权限越界、路径穿越、崩溃、超时、取消、进程泄漏和回滚。
- AI/MCP：流式输出、限流、错误正文、取消、超时、工具循环、重定向和密钥泄漏。
- 发布：干净 Windows 11 x64 虚拟机完成安装、升级、修复、卸载和自动更新回滚。

### 7.3 性能门禁

| 指标               |                    门禁 |
| ------------------ | ----------------------: |
| 缓存冷启动         | 不高于上一版本 + 500 ms |
| 启动器呼出 p95     |            不高于 50 ms |
| 空闲 RSS 增量      |           不高于 32 MiB |
| 官方插件首帧 p95   |           不高于 500 ms |
| 已打开插件焦点 p95 |            不高于 50 ms |
| 隐藏插件 VM        |                       0 |
| 隐藏插件宿主进程   |                       0 |
| 文件搜索查询 p95   |        目标不高于 50 ms |

性能测试必须记录机器、Windows 版本、构建配置、插件数量和原始输出。功能 smoke 结果不能替代 release 性能验收。

## 8. 文档与维护

- `docs/nsh-migration.md` 继续作为 NSH 迁移和发布证据的 canonical ledger。
- 本文件作为 Tinycast 功能对标的 canonical roadmap。
- 每个里程碑完成时更新架构决策、基准数据、Roadmap、英文和中文 Changelog。
- 上游版本升级时先更新本文件的基线和差距矩阵，再决定是否调整里程碑。
- 任何被判定为 Windows 不可实现或不值得强做的能力，必须在第 5 节记录原因和替代方案。

## 9. 风险与实施前提

- Windows Search 可能被用户关闭或在部分设备上不可用；功能必须显式降级。
- UI Automation 对 Electron、自绘控件、提权窗口和部分游戏支持不一致；菜单搜索只能承诺可用范围内的行为。
- Windows 虚拟桌面缺少与 macOS Spaces 完全等价的公开能力；不通过不稳定的私有 COM 接口承担核心功能。
- 日历字段取决于 AppointmentStore、Outlook/Exchange 或 Microsoft Graph，不能假设每个会议都有统一链接。
- 文本注入无法覆盖 UAC 安全桌面、提权目标和主动阻止输入的应用。
- 第三方插件在获得敏感能力前必须完成进程隔离，否则不开放市场高权限能力。
- 新增依赖必须遵守工作区依赖策略；优先复用 `nucleo`、`rusqlite`、`serde`、NSH 和已有 Windows 依赖。
- 计划不以日历时间承诺，而以上述版本门禁和可验证的退出标准推进。
