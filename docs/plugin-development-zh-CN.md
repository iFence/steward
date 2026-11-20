# Steward 插件开发与本地测试

本文面向 Steward 官方插件和兼容 NSH 的第三方插件，覆盖项目结构、宿主
API、本地预览、无头测试、临时签名、完整 Steward 集成测试和发布打包。

相关规范：

- [NSH 插件 API](nsh-plugin-api.md)
- [Steward Plugin Manifest v2](plugin-manifest-spec.md)
- [Extension API](extension-api.md)
- [NSH 迁移计划与状态](nsh-migration.md)

## 1. 架构约束

Steward 的插件是 TSX 应用，由固定版本的 NSH CLI 编译和签名：

```text
Steward
  |
  +-- 启动器只读取 plugin.json
  |
  +-- 打开插件时读取并验证 plugin.nshpkg
        |
        +-- 每个打开窗口使用独立 QuickJS VM
```

- 启动器搜索和路由发现只读取 `plugin.json`，不会启动插件 VM。
- 插件代码、能力、UI 和资源都在 `plugin.nshpkg` 中。
- 每次打开插件窗口都会启动一个独立的 NSH/QuickJS 运行时。
- 插件能力默认拒绝；只有 `nsh.json` 声明且宿主允许的能力可用。
- 每个打开插件使用 64 MiB 堆、1 MiB 栈、2 秒加载预算、250 ms 单次调用
  预算和最多 20,000 个 UI 节点。

## 2. 前置工具

推荐版本：

- Rust `1.97.1`
- Node.js 20 或更高版本
- pnpm 11
- NSH CLI `0.6.1`

安装 NSH CLI：

```powershell
cargo install --locked --version 0.6.1 nsh-cli
```

安装仓库依赖并验证基础环境：

```powershell
cd D:\rustprojects\steward
corepack pnpm install
nsh --version
```

### GPUI 开发宿主

通过 `cargo install` 安装的 `nsh` 不包含可选的开发宿主，
`nsh run --target gpui` 可能提示：

```text
nsh-gpui-host for gpui not found
```

使用官方发行版时，可以尝试：

```powershell
nsh component add gpui
```

源码版 `nsh` 不支持 `component add`，需要从对应 NSH 版本构建宿主。
以下命令会把 NSH `v0.6.1` 的 GPUI 宿主构建到 Cargo bin 目录：

```powershell
$nshSource = "D:\rustprojects\nsh-v0.6.1"

git clone --depth 1 --branch v0.6.1 `
  https://github.com/longbridge/nsh.git `
  $nshSource

Push-Location "$nshSource\crates\gpui"
try {
  cargo +1.97.1 build --release -p nsh-gpui-host
  Copy-Item `
    -LiteralPath "$nshSource\target\release\nsh-gpui-host.exe" `
    -Destination "$env:USERPROFILE\.cargo\bin\nsh-gpui-host.exe" `
    -Force
} finally {
  Pop-Location
}
```

> 如果 Windows 上 NSH `v0.6.1` 的 GPUI 热更新开发服务器仍报告
> `RefCell already borrowed`。这不影响 `nsh test` 和静态 bundle 预览。
> 遇到该问题时使用下文“GPUI bundle 预览”的方式。

## 3. 创建插件

官方插件位于 `packages/plugins/`。创建新插件时，优先复制现有插件作为
起点，保证 NSH、Steward API 和工作区配置一致：

```powershell
$plugins = "D:\rustprojects\steward\packages\plugins"
$source = "$plugins\calendar"
$target = "$plugins\my-plugin"

New-Item -ItemType Directory -Force -Path $target | Out-Null

@(
  "App.tsx"
  "App.test.tsx"
  "nsh.json"
  "plugin.json"
  "package.json"
  "tsconfig.json"
) | ForEach-Object {
  Copy-Item -LiteralPath "$source\$_" -Destination "$target\$_"
}

cd "D:\rustprojects\steward"
corepack pnpm install
```

然后修改包名、插件 ID、路由命令和版本：

```text
packages/plugins/my-plugin/
  App.tsx          # 插件入口
  App.test.tsx     # 无头测试
  nsh.json         # NSH 包元数据、版本和能力
  plugin.json      # Steward 路由元数据
  package.json     # pnpm 包和脚本
  tsconfig.json
```

`package.json` 的 `name` 必须是新的 `@steward/*` 包名。不要复制
`dist/`、`.nsh/` 或 `node_modules/`；这些目录都是生成或本机安装内容。

一个最小 `App.tsx`：

```tsx
import { launchParams, onCommand } from "@steward/extension-api";
import { mount, signal, Text, View } from "nsh";

function App() {
  const params = launchParams();
  const input = signal(params.input);

  onCommand(({ input: next }) => {
    input.set(next);
  });

  return (
    <View
      style={{
        flex: 1,
        alignItems: "center",
        justifyContent: "center",
        backgroundColor: params.dark ? "#202024" : "#ffffff",
      }}
    >
      <Text style={{ color: params.dark ? "#ffffff" : "#202024" }}>
        Hello from {params.pluginId}
      </Text>
      <Text style={{ color: params.accent }}>{input()}</Text>
    </View>
  );
}

mount(App);
```

## 4. Manifest 与版本

Steward 插件同时需要 `plugin.json` 和 `nsh.json`。两者的插件 ID
和版本必须一致。

### `plugin.json`

`plugin.json` 只负责启动器路由：

```json
{
  "schemaVersion": 2,
  "id": "com.example.my-plugin",
  "name": "My Plugin",
  "version": "0.1.0",
  "package": "plugin.nshpkg",
  "commands": [
    {
      "name": "my-plugin",
      "title": "My Plugin",
      "keywords": ["example"],
      "trigger": { "type": "command" }
    }
  ]
}
```

支持的命令触发类型：

- `command`：命令名匹配。
- `prefix`：关键字前缀匹配。
- `regex`：正则表达式匹配。

完整字段见 [Plugin Manifest v2](plugin-manifest-spec.md)。

### `nsh.json`

`nsh.json` 定义签名包里的应用 ID、版本、构建号和能力：

```json
{
  "name": "My Plugin",
  "id": "com.example.my-plugin",
  "version": "0.1.0",
  "build": 1,
  "capabilities": {
    "host": ["steward.*"]
  }
}
```

发布新构建时必须增加 `build`。`nsh publish` 会把该构建号写入签名包；
Steward 会检查包内版本与 `plugin.json` 完全一致。

能力采用 deny-by-default。只声明插件实际需要的能力：

- `host: ["steward.*"]`：使用 `@steward/extension-api`。
- 剪贴板、文件、URL 等能力按 NSH 要求显式声明。
- 不声明网络、进程执行、WebView 或图片选择器，除非产品需求明确需要并且
  宿主策略已经放行。

## 5. Steward Host API

插件通过 `@steward/extension-api` 使用稳定类型：

| API                      | 用途                                              |
| ------------------------ | ------------------------------------------------- |
| `launchParams()`         | 获取插件 ID、命令、输入、语言、主题色和明暗模式。 |
| `onCommand(cb)`          | 插件窗口已打开时接收新的命令输入。                |
| `onTheme(cb)`            | 接收强调色和明暗主题变化。                        |
| `clipboardHistory()`     | 读取 Steward 剪贴板历史快照。                     |
| `copyText(text)`         | 通过 Steward 写入剪贴板。                         |
| `storageGet(key)`        | 读取插件私有字符串。                              |
| `storageSet(key, value)` | 写入插件私有字符串。                              |
| `closeWindow()`          | 请求 Steward 关闭当前插件窗口。                   |

示例：

```tsx
import { copyText, launchParams, onTheme, storageGet, storageSet } from "@steward/extension-api";
import { signal } from "nsh";

const params = launchParams();
const accent = signal(params.accent);
const dark = signal(params.dark);

onTheme((theme) => {
  accent.set(theme.accent);
  dark.set(theme.dark);
});

async function saveAndCopy(value: string) {
  await storageSet("last-value", value);
  await copyText(value);
}

void storageGet("last-value").then((value) => {
  if (value) {
    console.log("restored", value);
  }
});
```

宿主接口清单和当前能力策略见 [NSH Plugin API](nsh-plugin-api.md)。

## 6. 开发测试分层

建议按以下顺序测试，反馈速度由快到慢：

1. TypeScript/TSX 静态检查。
2. NSH 无头交互测试。
3. GPUI 或原生窗口布局预览。
4. 本地签名包与完整 Steward 集成。
5. Release 工作流中的正式签名验证。

### 6.1 静态检查

在插件目录中：

```powershell
nsh check App.tsx
```

在仓库根目录运行全部 TypeScript 检查：

```powershell
corepack pnpm typecheck
corepack pnpm lint
corepack pnpm format:check
```

### 6.2 NSH 无头测试

在插件目录中：

```powershell
nsh test .
```

无头测试会运行 TSX 交互并模拟 Steward 宿主。示例：

```tsx
import { expect, open, test } from "nsh/test";

test("copies the selected value", async () => {
  const app = await open("./App.tsx", {
    host: {
      handlers: {
        "steward.clipboard.write": {},
      },
      params: {
        pluginId: "com.example.my-plugin",
        command: "my-plugin",
        input: "my-plugin demo",
        locale: "zh-CN",
        accent: "#863bff",
        dark: true,
      },
    },
  });

  await app.press("id:copy-value");

  expect(app.host.calls()).toEqual([
    {
      name: "steward.clipboard.write",
      data: { text: "demo" },
    },
  ]);
});
```

可以用宿主处理器模拟：

- `steward.clipboard.write`
- `steward.clipboard.history`
- `steward.storage.get`
- `steward.storage.set`
- `steward.window.close`

测试同时可以发送主题事件：

```tsx
await app.host.send("steward.theme", {
  accent: "#26a69a",
  dark: false,
});
```

### 6.3 GPUI bundle 预览

构建最新 bundle：

```powershell
cd D:\rustprojects\steward
corepack pnpm --filter @steward/plugin-my-plugin build
```

如果 `nsh run --target gpui` 可用，可以直接运行：

```powershell
cd packages\plugins\my-plugin
nsh run App.tsx --target gpui --window-size 900x760
```

如果遇到 NSH 热更新开发服务器的 GPUI `RefCell already borrowed` 问题，
直接加载构建后的静态 bundle：

```powershell
& "$env:USERPROFILE\.cargo\bin\nsh-gpui-host.exe" `
  --bundle "D:\rustprojects\steward\packages\plugins\my-plugin\dist\bundle.nshc" `
  --window-size 900x760 `
  --app-id com.example.my-plugin
```

静态 bundle 模式没有热更新。每次修改后重新执行 `pnpm build`，再重启
宿主。

### 6.4 构建

在插件目录中：

```powershell
nsh build App.tsx --release -o dist
```

或在仓库根目录构建全部官方插件：

```powershell
corepack pnpm build
```

输出位于：

```text
packages/plugins/my-plugin/dist/
  bundle.js
  bundle.nshc
  bundle.js.map
```

## 7. 本地签名与完整 Steward 集成

Steward 只启动通过签名验证的 `.nshpkg`。日常开发不应使用生产签名密钥，
可以创建一套仅用于本机的开发签名根。

### 7.1 创建开发签名根

密钥放在仓库外：

```powershell
$keys = "$env:LOCALAPPDATA\Steward\dev-signing"
New-Item -ItemType Directory -Force -Path $keys | Out-Null

nsh keygen "$keys\root"
nsh keygen "$keys\release"

nsh certify `
  --root "$keys\root.secret" `
  --key "$keys\release.pub" `
  --apps "*" `
  --days 365 `
  -o "$keys\release.cert"
```

永远不要提交 `*.secret`、开发证书或生产签名材料。

### 7.2 签名并暂存插件

以下命令使用开发密钥签名最新源码，并放入隔离的插件目录：

```powershell
$repo = "D:\rustprojects\steward"
$pluginName = "my-plugin"
$source = "$repo\packages\plugins\$pluginName"
$devRoot = "$repo\target\dev-plugin"
$plugins = "$devRoot\plugins"
$data = "$devRoot\data"

$manifest = Get-Content -Raw -Encoding utf8 "$source\plugin.json" |
  ConvertFrom-Json
$target = "$plugins\$($manifest.id)"

New-Item -ItemType Directory -Force -Path $target | Out-Null

nsh publish "$source\App.tsx" `
  --key "$keys\release.secret" `
  --cert "$keys\release.cert" `
  -o "$target\plugin.nshpkg"

nsh verify "$target\plugin.nshpkg" --root "$keys\root.pub"

Copy-Item `
  -LiteralPath "$source\plugin.json" `
  -Destination "$target\plugin.json" `
  -Force
```

`nsh verify` 必须输出 `valid`。如果只显示 “signature valid” 但没有检查
证书，说明没有正确传入 `--root`。

### 7.3 用隔离数据目录运行 Steward

先关闭所有已经运行的 Steward，因为应用是单实例。然后：

```powershell
$env:STEWARD_DATA_DIR = $data
$env:STEWARD_PLUGINS_DIR = $plugins
$env:STEWARD_NSH_TRUST_ROOT = (
  Get-Content -Raw "$keys\root.pub"
).Trim()
$env:STEWARD_SHOW_ON_START = "1"

cargo run -p steward-app
```

这套环境变量有以下作用：

| 环境变量                 | 作用                                           |
| ------------------------ | ---------------------------------------------- |
| `STEWARD_DATA_DIR`       | 使用独立数据库、日志、插件缓存和插件私有数据。 |
| `STEWARD_PLUGINS_DIR`    | 使用隔离的插件目录，不覆盖正式安装。           |
| `STEWARD_NSH_TRUST_ROOT` | 在当前进程中追加开发签名根。                   |
| `STEWARD_SHOW_ON_START`  | 启动后立即显示启动器。                         |

测试完成后可以清除变量：

```powershell
Remove-Item Env:STEWARD_DATA_DIR
Remove-Item Env:STEWARD_PLUGINS_DIR
Remove-Item Env:STEWARD_NSH_TRUST_ROOT
Remove-Item Env:STEWARD_SHOW_ON_START
```

Steward 会在启动时扫描插件目录，并在运行期间定时重新扫描。替换开发包后
通常只需关闭并重新打开插件窗口；如果修改了路由元数据，等待下一次扫描或
重启应用。

## 8. 自动打开插件并检查首帧

Steward 支持开发 smoke 模式：

```powershell
$env:STEWARD_SMOKE_PLUGIN = "com.example.my-plugin"
$env:STEWARD_SMOKE_COMMAND = "my-plugin"
$env:STEWARD_SMOKE_TITLE = "My Plugin"
$env:STEWARD_METRICS_PATH = "$repo\target\my-plugin.jsonl"

cargo run -p steward-app
```

指标文件中出现以下事件表示插件成功完成首帧：

```json
{
  "plugin_id": "com.example.my-plugin",
  "event": "plugin_first_frame"
}
```

还可以检查：

- `plugin_window_vm_count`：打开的插件 VM 数。
- `hidden_plugin_vm_count`：隐藏插件 VM 数，必须为 0。
- `plugin_focus`：再次打开已有插件窗口的聚焦耗时。

## 9. 打包发布

发布前同步更新：

```text
nsh.json       version + build
plugin.json    version
package.json   version
```

然后运行：

```powershell
cd D:\rustprojects\steward

$env:STEWARD_NSH_RELEASE_KEY = "C:\secure\release.secret"
$env:STEWARD_NSH_RELEASE_CERT = "C:\secure\release.cert"

.\scripts\package-plugins.ps1 `
  -Plugin my-plugin `
  -ForceSign

.\scripts\verify-release-plugins.ps1
```

插件 zip 只能包含：

```text
my-plugin/
  plugin.json
  plugin.nshpkg
```

Steward 安装目录结构：

```text
%APPDATA%\Steward\plugins\my-plugin\
  plugin.json
  plugin.nshpkg
```

不要只复制 `plugin.nshpkg`，也不要扁平化到 `plugins` 根目录。

新增 pnpm workspace 包后，还需要提交更新后的 `pnpm-lock.yaml`。

## 10. 常见问题

### 插件窗口打开后立即关闭

先查看 Steward 日志：

```text
%APPDATA%\Steward\logs\steward.log.YYYY-MM-DD
```

搜索：

- `plugin launch failed`
- `first_frame_failed`
- `plugin application error`

常见原因：

- 签名包仍由旧源码构建。
- 使用了当前 NSH 运行时已移除的模块。
- 插件 ID 或版本与 `plugin.json` 不一致。
- 包未通过正式或开发信任根验证。

### 启动器搜索不到插件

检查：

```powershell
$plugin = "$env:APPDATA\Steward\plugins\my-plugin"
Get-ChildItem -Force -Recurse $plugin
```

确认：

- 插件位于 `plugins` 的直接子目录中。
- `plugin.json` 存在且是 schema v2。
- `package` 字段等于 `plugin.nshpkg`。
- `commands` 至少包含一个命令。
- 插件的 `manifest.id` 和包内 ID 一致。

Steward 运行时会定时扫描插件目录，安装后通常无需重启。

### 包签名验证失败

开发环境需要同时满足：

- 使用开发 release key 签名。
- 对应 root public key 通过 `STEWARD_NSH_TRUST_ROOT` 注入。
- `nsh.json` 与 `plugin.json` 的 ID、版本一致。

### `nsh-gpui-host` 不存在

参考本文“GPUI 开发宿主”小节从匹配的 NSH 版本构建。

### GPUI 报告 `RefCell already borrowed`

这是 NSH `v0.6.1` GPUI 热更新开发服务器在当前 GPUI Fast 版本下的问题。
改用“GPUI bundle 预览”或 `nsh test`。

### `nsh test` 提示 `.nsh` 目录拒绝访问

先删除或重命名旧的生成文件：

```powershell
Get-ChildItem -Force -Recurse .nsh\test
```

必要时删除被阻塞的 `.nsh/test/*.run.js`。这些文件都是生成缓存，不属于
插件源码。

## 11. 提交前检查清单

- [ ] `plugin.json`、`nsh.json` 和 `package.json` 版本一致。
- [ ] `nsh.json` 的 `build` 已增加。
- [ ] `nsh check App.tsx` 通过。
- [ ] `nsh test .` 通过。
- [ ] `pnpm lint`、`pnpm typecheck`、`pnpm format:check` 通过。
- [ ] GPUI bundle 或原生窗口人工检查通过。
- [ ] 本地签名包在隔离 Steward 数据目录中完成首帧。
- [ ] 日志中没有 `first_frame_failed` 或运行时错误。
- [ ] 没有提交任何签名私钥、证书或临时数据文件。
