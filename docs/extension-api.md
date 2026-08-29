# 插件 API 草案

> 状态：M3 已落地（含 M3 二轮 async / Grid·Search / Node polyfill / 跨进程 fs·net）。
> `packages/extension-api` 提供类型声明 + 运行时 polyfill：
> esbuild 把插件源码与 SDK 打成单文件 IIFE，运行时代码全部路由到宿主注入的
> `globalThis.steward` 桥（剪贴板 / toast / storage / open / fs / net），按 manifest `permissions` 授权。

## 最小 API 集（M2）
| API | 说明 |
|---|---|
| `List` | 在 Steward UI 中渲染可搜索的列表（`items` + `onSelect`） |
| `Clipboard` | 剪贴板读写（`read` / `write`），按 `clipboard.read` / `clipboard.write` 授权 |
| `showToast` | 显示短暂通知（`message` / `kind` / `durationMs`） |
| `selectItem` | 把 `item.invoke` 分发给最近一次 `List` 注册的 `onSelect` |

> M2 视图支持 `{ type: "list", items: [...] }` 与
> `{ type: "calendar", year, month, today, startOfWeek?, selected? }`，且同步
> 返回；插件通过导出 `command(name, input)` 与 `select(itemId)` 暴露能力，
> 宿主读取 `globalThis.__stewardPlugin`。日历视图在启动器中渲染为月历网格，
> 方向键移动选中日、回车/点击把日期交给 `select`。日历左侧的周数列
> （`W` 前缀，ISO 8601）为纯展示，不改变视图契约。
> 农历信息（农历日 / 传统节日 / 公历节日 / 二十四节气）由宿主在渲染时自动叠加，同样不改变视图契约。

## M3 扩展
| API | 说明 |
|---|---|
| `ActionPanel` | 操作面板（操作列表 / 详情视图）；M2 调用会显式报错 |
| `Detail` | 详情视图 |
| `Form` | 表单视图 |
| `LocalStorage` | 插件本地键值存储 |
| `openUrl` / `openPath` | 打开 URL（默认浏览器）/ 打开文件/文件夹/`shell:` 目标（OS `open` verb）；分别需 `open.url` / `open.path` 权限，调用未授权即抛 `permission denied` |
| `fs.readFile` | 读取磁盘文件（`await`，跨进程往返）；需 `fs.read` 权限 + `fs_roots` 白名单；`encoding` 支持 `utf8`（返回 `string`）/ `base64`（返回 `Uint8Array`） |
| `fs.writeFile` | 写入磁盘文件（`await`，跨进程往返）；需 `fs.write` 权限 + `fs_roots` 白名单；`encoding` 支持 `utf8`（`data: string`）/ `base64`（`data: Uint8Array`） |
| `net.request` | 发起 HTTP(S) 请求（`await`，跨进程往返）；需 `network` 权限；返回 `{ status, headers, body }`；`timeoutMs`/`maxBytes` 由宿主限制 |

## 约束

- 插件产物是 esbuild 打包的单文件 JS（IIFE + `--global-name=__stewardPlugin`），不依赖 Node API。
- M3 起提供 20-30 个常用 Node 内置模块 polyfill：`path` / `buffer` / `process` / `events` / `util` /
  `url` / `querystring` / `string_decoder` / `assert` / `os` 为纯 JS 全功能；`fs` 提供
  `readFile` / `writeFile`（宿主往返），其余 `fs` 接口与 `http` / `net` 等为 stub。不支持 native binding。
- 默认零权限：需要的能力在 manifest `permissions` 中声明。
- manifest 可选的 `icon` 字段是内联 SVG 文档；宿主会把它缓存并在启动器结果行中
  与应用图标一样渲染（未声明时插件行不显示图标）。

## M3 二轮：async 命令、Node polyfill、Grid/Search、主题一致性

### 全部 handler 可 `async`

`command` / `select` / `run` / `submit`（以及新加的 `search`）都允许返回 `Promise`。宿主在执行
deadline 内驱动 QuickJS 微任务队列直到 settled（`command`/`select`/`search` 取返回值，`run`/`submit`
忽略返回值），因此插件可以写 `async function command() { await Clipboard.read(); ... }`。M3 支持真正的
跨进程 await：`fs.readFile` / `fs.writeFile`（`host.fs.read` / `host.fs.write`）与 `net.request`
（`host.net.request`）会 park isolate，宿主完成后恢复 Promise；微任务 + 同步宿主函数（`Clipboard` /
`LocalStorage`）立即 resolved。若 promise 永不 settle 则按 timeout 处理并回收 isolate；同一 isolate
一次仅一个 parked 调用，busy 时新请求返回 `plugin is busy`，isolate 被 kill/驱逐后在途回复直接丢弃。

### `grid` 与 `search` 视图

- `grid`：`{ type: "grid", columns, items: GridItem[], selectedId?, actionPanel? }`，`GridItem` 为
  `{ id, title, subtitle?, icon?, badge? }`。宿主用 N 列卡片渲染，方向键移动选中、Enter 确认走
  `select(itemId)`。
- `search`：`{ type: "search", placeholder?, actionPanel? }`。宿主渲染一个搜索列（panel 内自带
  `SearchBar`），输入变化发 `search.query`，插件导出的 `search(query)` 返回 `View`（通常 `list`/`grid`）
  替换结果区；`gen` 丢弃旧结果。结果确认走 `select`。

### Node 内置模块 polyfill（runtime 注入）

插件 bundle 将 Node 内置模块标为 external（见 `scripts/build-plugin.mjs`），运行时在求值 bundle 前注入
`require`/`module`/`exports`/`process`/`Buffer`/`global` 与模块注册表。纯 JS 模块 `path` / `buffer` /
`process` / `events` / `util` / `url` / `querystring` / `string_decoder` / `assert` / `os` 全功能；
`require("fs").readFile` / `writeFile` 走宿主往返（需 `fs.read`/`fs.write` 权限 + `fs_roots`）；其余
`fs` 接口（`readFileSync`/`writeFileSync`/`readdir`/`stat` 等）抛 "not supported in this phase"。
`http` / `https` / `net` / `dns` / `child_process` / `crypto` / `zlib` / `stream` 为 stub，调用即抛
"not supported in M3"；`network` 权限已支持（`net.request`）。`timers`、原生 binding、`worker_threads`
明确不支持。plugin 内可直接 `require("path")` 或使用 `global.Buffer`。
