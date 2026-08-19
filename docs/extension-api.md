# 插件 API 草案

> 状态：M2 草案。运行时（宿主函数注入）M2 落地，当前 `packages/extension-api` 仅提供类型契约。

## 最小 API 集（M2）

| API | 说明 |
|---|---|
| `List` | 在 Steward UI 中渲染可搜索的列表（`items` + `onSelect`） |
| `ActionPanel` | 渲染操作面板（操作列表/详情视图） |
| `showToast` | 显示短暂通知（`message` / `kind` / `durationMs`） |

## 扩展示例（M3）

| API | 说明 |
|---|---|
| `Detail` | 详情视图 |
| `Form` | 表单视图 |
| `LocalStorage` | 插件本地键值存储 |
| `Clipboard` | 剪贴板读写 |

## 约束

- 插件产物为 esbuild 打包的单文件 JS，不依赖 Node API。
- M3 起提供 20-30 个常用 Node 内置模块 polyfill（fs、path、buffer、http 等），不支持 native binding。
- 默认零权限：需要的能力在 manifest `permissions` 中声明。
