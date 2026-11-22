# Steward 插件开发者模式

> 状态：PDX-2 已实现并纳入 CI。开发者模式只改变本地开发目录和替换流程，不会
> 放宽签名、信任根、能力策略或进程隔离。

## 隔离运行配置

开发者模式会在打开数据库、申请单实例锁和初始化插件目录之前切换配置。

| 项目 | 正式应用 | 开发者模式 |
| --- | --- | --- |
| 数据目录 | `%APPDATA%\Steward` | `%APPDATA%\Steward-Developer` |
| 插件目录 | `<data>\plugins` | `<developer-data>\plugins` |
| 单实例锁 | 正式配置 | `developer` 后缀 |
| 信任根 | 内置生产信任根 | 内置根加显式开发信任根 |

可以在正式 Steward 运行期间同时启动开发用应用：

```powershell
cargo run -p steward-app -- --developer-mode
```

自定义隔离目录和开发信任根：

```powershell
cargo run -p steward-app -- `
  --developer-mode `
  --developer-data-dir D:\steward-dev\data `
  --developer-plugins-dir D:\steward-dev\plugins `
  --developer-trust-root D:\steward-dev\signing\root.pub
```

`STEWARD_DATA_DIR` 和 `STEWARD_PLUGINS_DIR` 在开发者模式下会被忽略，避免继承
的环境变量误写正式安装。

## 一条命令完成重建与重载

在一个终端以开发者模式运行 Steward，修改插件后执行：

```powershell
steward-plugin --developer-mode dev D:\plugins\my-plugin `
  --signing-key C:\secure\release.secret `
  --signing-cert C:\secure\release.cert
```

命令会依次：

1. 优先运行 `package.json` 的 `dev:build`，其次运行 `build`，否则执行
   `nsh build App.tsx --release -o dist`；
2. 使用 `nsh publish` 签名；
3. 生成只包含 `plugin.json` 和 `plugin.nshpkg` 的标准插件 ZIP；
4. 使用与 CLI/Settings 安装相同的签名、信任、版本、能力与布局检查；
5. 即使在开发模式中版本号和 build 号未变化也替换当前开发包；
6. 向运行中的开发者模式 Steward 排队一次静态重载。

签名路径也可以使用 `STEWARD_NSH_RELEASE_KEY` 与
`STEWARD_NSH_RELEASE_CERT`。如果自定义构建没有生成 `dist/plugin.nshpkg`，
必须提供签名材料。CLI 会拒绝比源码更旧的包，避免误加载旧构建。

只重新安装现有签名包：

```powershell
steward-plugin --developer-mode dev . --no-build
```

## 静态重载回退

PDX-2 不依赖 NSH GPUI 热重载。重载请求仅包含插件 ID、命令、标题和是否重新
打开窗口。Steward 会刷新路由缓存、关闭旧宿主进程，并在全新 QuickJS VM 中
加载静态包。

Settings > Plugins 的“重新加载”按钮使用相同流程。即使插件窗口已崩溃或关闭，
也可以按请求重新打开；`--no-open` 可只替换而不打开窗口。

## 从 Settings 安装本地包

Settings > Plugins 新增：

- **安装 ZIP...**：选择标准两文件插件 ZIP；
- **安装文件夹...**：选择只包含 `plugin.json` 与 `plugin.nshpkg` 的目录。

Settings 与 CLI 调用同一个 `PluginStore` 检查和安装路径。开发者模式中对同版本
本地包使用开发替换规则；正式配置仍要求更高语义版本和 NSH build。

## 开发者诊断

开发者模式下的 Settings > Plugins 会显示每个插件的：

- 首帧耗时；
- 宿主调用结果和失败次数；
- 被拒绝的能力；
- 进程退出与最近崩溃；
- 最近运行事件；
- 包含插件 ID 且带错误信号的有限日志摘录。

能力跟踪只保存调用名称与结果。请求正文、参数、剪贴板内容、文件内容、签名密钥
和其他敏感值默认不会进入诊断数据。

## 启动失败 Doctor

可以对已安装 ID、ZIP 或两文件目录运行统一诊断：

```powershell
steward-plugin --developer-mode doctor com.example.my-plugin
steward-plugin --developer-mode doctor .\plugin.zip --json
```

检查项覆盖来源与布局、manifest、NSH 包、ID/版本一致性、签名、信任根、宿主
API、能力声明与发布策略。失败时命令返回非零状态，并指出第一个失败阶段。

## 自动验收

`pnpm test:pdx2` 会创建隔离的签名插件，在相同版本下重建两次，验证包替换、
静态重载队列、doctor 成功/失败路径，并确认开发者模式忽略正式
`STEWARD_DATA_DIR`。
