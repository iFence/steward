# 插件 API 草案

> 状态：M2 已落地。`packages/extension-api` 提供类型声明 + 运行时 polyfill：
> esbuild 把插件源码与 SDK 打成单文件 IIFE，运行时代码全部路由到宿主注入的
> `globalThis.steward` 桥（剪贴板 / toast），按 manifest `permissions` 授权。

## 最小 API 集（M2）
| API | 说明 |
|---|---|
| `List` | 在 Steward UI 中渲染可搜索的列表（`items` + `onSelect`） |
| `Clipboard` | 剪贴板读写（`read` / `write`），按 `clipboard.read` / `clipboard.write` 授权 |
| `showToast` | 显示短暂通知（`message` / `kind` / `durationMs`） |
| `selectItem` | 把 `item.invoke` 分发给最近一次 `List` 注册的 `onSelect` |

> M2 视图仅支持 `{ type: "list", items: [...] }` 且同步返回；插件通过导出
> `command(name, input)` 与 `select(itemId)` 暴露能力，宿主读取
> `globalThis.__stewardPlugin`。

## M3 扩展
| API | 说明 |
|---|---|
| `ActionPanel` | 操作面板（操作列表 / 详情视图）；M2 调用会显式报错 |
| `Detail` | 详情视图 |
| `Form` | 表单视图 |
| `LocalStorage` | 插件本地键值存储 |

## 约束

- 插件产物是 esbuild 打包的单文件 JS（IIFE + `--global-name=__stewardPlugin`），不依赖 Node API。
- M3 起提供 20-30 个常用 Node 内置模块 polyfill（fs、path、buffer、http 等），不支持 native binding。
- 默认零权限：需要的能力在 manifest `permissions` 中声明。
