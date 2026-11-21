# Windows 11 验收矩阵

本文档记录 Steward 在 Windows 11 23H2+ x64 上的 P0 发布验收。请在干净的
虚拟机中执行自动用例，并使用纯键盘操作完成人工用例。发布证据应附上命令输出、
性能 JSON、MSI 日志和虚拟机镜像标识。

## 环境

| 字段 | 要求 |
|---|---|
| 操作系统 | Windows 11 23H2 或更高版本，x64 |
| Steward 构建 | Release 可执行文件和 MSI 来自同一提交 |
| 插件 | 使用生产密钥签名的 Calendar、Clipboard History、UI Showcase |
| 显示环境 | 至少一台 100% 缩放的显示器；多 DPI 用例在第二台虚拟机执行 |
| 账户 | 使用本地非管理员用户执行启动测试 |
| 网络 | 默认关闭；仅专门测试更新功能的用例允许联网 |

## P0 自动验收用例

| 编号 | 用例 | 命令 | 通过条件 |
|---|---|---|---|
| P0-A01 | 生产信任根格式 | `pwsh scripts/verify-trust-roots.ps1` | 至少包含一个 32 字节 Ed25519 根公钥；不存在无效或重复公钥 |
| P0-A02 | Release 插件签名 | `pwsh scripts/verify-release-plugins.ps1` | 每个 zip 恰好包含两个文件，并可通过生产信任根验证 |
| P0-A03 | Rust 质量门禁 | `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` | 所有命令退出码为 0 |
| P0-A04 | TypeScript 与插件门禁 | `pnpm lint`、`pnpm format:check`、`pnpm typecheck`、`pnpm build` | 所有命令退出码为 0 |
| P0-A05 | 常驻性能 | `pwsh scripts/bench-resident.ps1 -Samples 20` | 已记录启动与 RSS；呼出 p95 不高于 50 ms；托盘待机时隐藏 VM 数为 0 |
| P0-A06 | 插件性能 | `pwsh scripts/bench-plugins.ps1 -Plugin calendar -Iterations 20` | 首帧 p95 不高于 500 ms；聚焦 p95 不高于 50 ms；隐藏 VM 数为 0 |
| P0-A07 | MSI 干净安装、修复与卸载 | `pwsh scripts/test-msi-lifecycle.ps1 -Msi <当前版本.msi>` | 安装、启动、修复和卸载全部通过 |
| P0-A08 | MSI 大版本升级 | `pwsh scripts/test-msi-lifecycle.ps1 -Msi <当前版本.msi> -PreviousMsi <上一版本.msi>` | 上一版本可原位升级；开机自启状态保留；卸载后安装文件被移除 |

性能验收必须保留 `target\benchmarks\...\summary.json` 和
`scripts/bench-resident.ps1` 的输出。还需记录机器型号、CPU、内存、Windows
版本、构建配置、插件数量和原始采样值。功能 smoke 结果不能替代 Release
性能证据。

## 当前执行记录

记录时间：2026-11-17。代码提交：
`f91f19436c6cead116b0eb3a1fd22ed5ddb64a7e`。

成功运行：[Build and Release #44029877843](https://github.com/iFence/steward/actions/runs/44029877843)。

| 用例 | 状态 | 证据或未执行原因 |
|---|---|---|
| P0-A01 | 通过 | 本地脚本和生产 workflow 均校验通过；生产根 SHA-256 为 `569e4243fc43965650ba3e3c365c62f89425c09bfc476124ee0a1079d2e99b74` |
| P0-A02 | 通过 | workflow 使用生产信任根签名并验证三个插件；下载 artifact 后本地再次验证通过 |
| P0-A03 | 通过 | `cargo fmt`、workspace Clippy、workspace 测试全部通过 |
| P0-A04 | 通过 | `pnpm lint`、`format:check`、`typecheck`、`build` 全部通过 |
| P0-A05 | 通过（开发机） | 20 次采样呼出 p95 为 46 ms；正式干净 VM 证据仍需归档 |
| P0-A06 | 通过（开发机 smoke） | Calendar 首帧 p95 为 349.1 ms，聚焦 p95 为 5.4 ms，隐藏 VM 为 0 |
| P0-A07 | 未执行 | 当前会话不是管理员，且不是干净验收 VM |
| P0-A08 | 未执行 | 需要干净验收 VM 和上一版本 MSI |
| P0-M01 至 P0-M12 | 未完成 | 需要交互式 Windows 11 桌面环境；自动 smoke 已覆盖部分启动和插件路径 |

本次生产 artifact：

- MSI：`steward-app-0.3.1-x86_64.msi`，26,529,792 字节，
  SHA-256 `be7b71799a5c9a44673140a42d4c03d42ff7517f1af39771dba972f4cd53d165`。
- Calendar：`01045e968adbac41e4fb9ec23cd797a9224c1cb8d51860c1e58fba0e265b9d20`。
- Clipboard History：`374d9b233a5fc621cd2f12345982aced4742ebf6841a6407d9061fcb8d21c23a`。
- UI Showcase：`3e3155671c71c8322e305f24d10df0ccd7316876dc3f6d4f297059918522546a`。

## P0 人工验收用例

| 编号 | 用例 | 操作步骤 | 通过条件 |
|---|---|---|---|
| P0-M01 | 首次启动 | 在干净用户配置中登录，并从开始菜单启动 Steward | 托盘图标出现；不创建插件窗口、QuickJS VM 或插件子进程；启动器可通过配置的热键打开 |
| P0-M02 | 键盘启动器 | 全程只使用键盘搜索记事本、Windows 设置页和文件 | 选中项与可见列表始终同步；按 Enter 打开当前选中项 |
| P0-M03 | 计算器与 URL | 输入 `8*8`，然后输入一个有效 URL | 计算器复制 `64`；URL 在默认浏览器中打开 |
| P0-M04 | 热键冲突 | 在另一个应用中占用 Steward 的呼出热键，再把 Steward 改为同一组合键 | 界面明确报告冲突，Steward 保留原先可用的绑定 |
| P0-M05 | 插件生命周期 | 打开三个官方插件，重复执行各自命令，关闭窗口后重新打开 | 每个插件只有一个 NSH 窗口和插件宿主进程；重复命令会聚焦窗口并发送命令；关闭后释放进程与 VM；外壳进程内 VM 数保持为 0 |
| P0-M06 | 插件故障隔离 | 在测试数据目录中用无效或未签名包替换一个插件包，并终止一个已打开的插件宿主进程 | 启动失败并报告信任或包错误；插件进程退出不会关闭启动器、托盘或其他插件，并显示结构化失败提示 |
| P0-M07 | 运行中升级 | Steward 运行时安装当前 MSI，覆盖上一版本 | 安装器关闭运行中的应用并完成升级；用户设置和开机自启行为保留 |
| P0-M08 | 修复 | 删除已安装可执行文件后运行 MSI 修复 | 可执行文件和开始菜单快捷方式恢复；用户数据保持不变 |
| P0-M09 | 卸载 | 从“设置 > 应用”卸载 Steward | 可执行文件、快捷方式和开机自启项被移除；遵循既有的无关用户数据保留策略 |
| P0-M10 | 多 DPI 与 IME | 在 125%/150% 缩放、混合 DPI 显示器和中文输入法下重复 P0-M02 | 窗口位置、焦点、IME 组合输入和键盘导航均可正常使用 |
| P0-M11 | 托盘与单实例 | 通过托盘切换、按全局热键，并再次启动一个实例 | 只有一个进程持有托盘和热键；第二次启动会唤起已有启动器 |
| P0-M12 | 离线启动 | 关闭网络后启动 Steward | 启动器和本地搜索可用；更新失败不会阻塞启动或插件 |

## 证据模板

```text
提交：
标签/版本：
CI 运行：
虚拟机镜像：
Windows 版本：
CPU/内存：
Release 可执行文件 SHA-256：
MSI SHA-256：
插件归档 SHA-256：
信任根 SHA-256：
冷启动：
常驻 RSS：
呼出 p95：
插件首帧 p95：
插件聚焦 p95：
隐藏 VM 数：
MSI 生命周期日志：
人工用例偏差：
```

只有当全部自动用例通过，且人工用例偏差已在
[nsh-migration.md](nsh-migration.md) 中登记并接受后，才能退出 P0。

## Phase 3 Process Isolation Acceptance

Run these checks with the same production-signed MSI, executable, plugin
archives, and trust root as P0.

| ID | Check | Command | Pass criteria |
|---|---|---|---|
| P3-A01 | Job Object assignment | `pwsh scripts/bench-plugins.ps1 -Plugin calendar -Iterations 20 -PackagePath <calendar nshpkg> -TrustRootFile <root>` | Containment backend is `windows_job_object`; memory limit is 256 MiB; CPU cap is 50%; active-process limit is 1; handle limit is 1,024 |
| P3-A02 | Plugin performance and RSS | Same as P3-A01 | First-frame p95 is at most 500 ms, focus p95 is at most 50 ms, and summary contains peak host and aggregate RSS |
| P3-A03 | All official plugins | Repeat P3-A01 and P3-A02 for Calendar, Clipboard History, and UI Showcase; then run `pwsh scripts/bench-plugin-set.ps1 -TrustRootFile <root>` | Every plugin opens once, the three-window run reports exactly three host processes and aggregate RSS, and no host process survives shell closure |
| P3-A04 | Crash containment and backoff | `pwsh scripts/test-plugin-isolation.ps1 -Exe <release exe> -PackagePath <calendar nshpkg> -TrustRootFile <root>` | The shell survives the forced host crash, host count returns to zero, and the immediate repeated launch is rejected by backoff |
| P3-A05 | Memory-limit enforcement | `pwsh scripts/test-plugin-isolation.ps1 -Exe <release exe> -PackagePath <calendar nshpkg> -TrustRootFile <root> -MemoryLimitMiB 32` | Host terminates with `resource_limit`; the shell remains alive and host count returns to zero |
| P3-A06 | Clean-VM lifecycle | Run the P0 MSI lifecycle plus the signed install/upgrade/crash matrix | Install, launch, upgrade, repair, first-frame, crash, and uninstall pass on a clean Windows 11 23H2+ x64 VM |

Store each `summary.json`, raw metrics JSONL, MSI log, executable SHA-256, plugin
archive SHA-256, trust-root SHA-256, VM image identifier, and machine details
under the release evidence directory. Do not mark Phase 3 complete from a
development-machine run alone.
