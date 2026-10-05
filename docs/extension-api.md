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

## M3.5：Virtual UI Tree（`{ "type": "ui" }`）

除固定视图（`list` / `calendar` / `detail` / `form` / `grid` / `search`）外，插件可以返回一棵可序列化的
元素树，由宿主校验后用与启动器相同的 gpui 组件渲染。插件依旧**不运行任何 UI 代码、也不离开插件进程**，
进程隔离与低内存约束不变。

```ts
import { button, col, input, row, text, ui } from "@steward/extension-api";
import type { View } from "@steward/extension-api";

let clicks = 0;

function render(): View {
  return ui(
    col()
      .gap(8)
      .p(12)
      .child(text("Hello").text_color("primary").text_lg())
      .child(
        row()
          .gap(8)
          .items_center()
          .child(button("Click").onClick(() => {
            clicks += 1;
            return render();
          }))
          .child(input("q", { placeholder: "type here" })),
      ),
  );
}

export function command(): View {
  return render();
}
```

### 元素与样式

- 容器：`div` / `row` / `col` / `grid(columns)` / `scroll(axis)`。
- 显示叶子：`text` / `heading(level)` / `tag` / `badge` / `icon`（内联 SVG）/ `image`（内联 `data:` URI）/
  `separator` / `progress(0..1)` / `spinner` / `skeleton` / `description_list([{label,value}])` / `spacer`。
- 交互叶子：`button` / `link` / `input`（宿主持有文本）/ `checkbox` / `switch` / `select`。控件值由**插件
  自持**：树里携带当前 `checked` / `value`，点击经 `change` 事件（值为新值）回报，插件翻转状态并返回新树。
- 样式方法与 gpui 同名，由宿主规范表（`style_table.json`）生成：布局/尺寸/flex/间距/边框圆角/颜色/文字等。
  颜色取主题 token（如 `"primary"`、`"muted_foreground"`）或 `#rrggbb`；未知方法在调用点即报错。
- `.id(name)` 提供稳定 id（`input` 必需）；`.child(...)` 追加子节点；`.style(name, value)` 是向前兼容的逃生口。

### 事件与输入

- 事件首批为 `click` / `change` / `submit`。处理函数可返回新的 `ui` 视图替换当前树；返回 `undefined`
  表示不变。宿主用 `view.invoke` 把事件交给插件。
- 输入框文本由**宿主持有**：`props.value` 只是初值，之后由宿主维护并派发 `change`；因此输入延迟与插件的
  往返无关。提交（回车）派发 `submit`。输入实体按元素 id 在重绘间保留。

### 边界与限制

- 宿主把树当作不可信数据校验：深度 ≤ 32、节点 ≤ 2000、每节点子节点 ≤ 256、样式项 ≤ 64、文本 ≤ 8 KiB、
  整树 ≤ 1 MiB，长度 0–4096、`opacity` 0–1、`columns` 1–16；越界或未知字段会被拒绝。
- v1 媒体仅限内联 SVG 与 `data:` URI；`link` 的点击走回调（打开 URL 仍由权限化的 `openUrl` 负责）。
- 仍需宿主托管状态的控件（`slider` / `combobox` / `stepper` / `tabs` / `accordion`）、数据展示
  （`table` / `tree` / `pagination`）与富内容（`markdown` / `code` / `chart`）留待后续批次；代码编辑器 /
  LSP、WebView 明确不做。
