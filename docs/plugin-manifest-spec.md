# 插件 Manifest 规范（草案）

> 状态：M2 草案，先定字段，随 `plugin-registry`/`plugin-host` 实现迭代。

## 示例

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

## 字段

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `id` | string | 是 | 反向域名格式，全局唯一 |
| `name` | string | 是 | 展示名 |
| `version` | string | 是 | 语义化版本；变化时才重新扫描解析 |
| `commands` | array | 是 | 命令列表 |
| `commands[].name` | string | 是 | 命令名 |
| `commands[].title` | string | 是 | 命令标题 |
| `commands[].detachable` | bool | 否 | 该命令返回的视图可否弹出为独立窗口（默认 `false`）。宿主据此在视图上显示通用「弹出」控件，弹出后窗口不受启动器全局热键/失焦隐显影响；仅影响 UI 呈现，不新增权限。 |
| `commands[].keywords` | string[] | 否 | 本地化搜索关键词（如 `"日历"`），配合命令名/标题一起参与模糊匹配，默认空数组 |
| `commands[].trigger` | object | 是 | 触发条件 |
| `permissions` | string[] | 否 | 能力白名单，默认空数组（零权限） |
| `isolation` | string | 否 | 隔离级别，默认 `shared-pool` |

## trigger.type

- `command`：命令名，按应用搜索同样支持部分输入/模糊命中（精确名或命令名+参数优先，其余按 fuzzy 得分排序）
- `prefix`：关键字前缀（如 `=`）
- `regex`：正则触发（谨慎开放）
- `dynamic`：每次输入都参与，必须带响应超时熔断（如 100ms 未返回则跳过本次渲染）

## isolation

- `shared-pool`（默认）：进程内多实例池，设置 QuickJS 堆内存上限 + 执行超时，超限 kill 该实例
- `dedicated-process`：独立子进程隔离；声明网络/文件系统权限或依赖较重的插件强制此项

## permissions（白名单枚举，M2 扩展）

`clipboard.read` / `clipboard.write` / `network` / `fs.read` / `fs.write` 等。主进程按声明开放对应 host function。
