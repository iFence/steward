# 性能基准

> 三个核心指标：冷启动时间、常驻内存（RSS）、呼出延迟。
> 每个里程碑跑一次 `scripts/bench.sh`，把结果回填到下表。

## 指标

| 指标 | 说明 | 单位 |
|---|---|---|
| 冷启动 | 进程启动到窗口可交互（M0 近似：进程存活） | ms |
| 常驻内存 RSS | 进程常驻内存 | MB |
| 呼出延迟 | 热键按下到窗口显示 | ms |

## M0 基线

环境：Windows（开发机）、debug 构建、2026-08-19；计时为 PowerShell 启动进程到 `MainWindowHandle` 非空。

| 指标 | 数值 | 备注 |
|---|---|---|
| 冷启动 | ≈ 1756 ms | debug 构建；release + macOS 目标验收 50ms |
| RSS | ≈ 152 MB | debug 构建（含 wgpu/GPU 初始化），release 预计大幅下降 |
| 呼出延迟 | 待手动记录 | 热键已注册成功；前台任务 10ms 轮询桥接 |

> 验收标准（50ms 冷启动）按 macOS release 构建定义；Windows 开发机上先积累数据。

## M1 基线（应用启动器 MVP）

环境：Windows（开发机）、release 构建、2026-08-23；由 `scripts/bench-resident.ps1` 测量（PostMessage 注入 `WM_HOTKEY` + FindWindow 轮询可见性，按 PID 校验窗口归属）。冷启动为启动 → 托盘窗口就绪；应用扫描/建索引用删库冷启动测得（后台线程，不阻塞启动）。

| 指标 | 数值 | 备注 |
|---|---|---|
| 冷启动 | ≈ 307 ms（release） | 启动 → 托盘就绪；含一次性 GPUI/DirectX/DirectWrite 初始化，启动静默 |
| 应用扫描 + 建索引 | ≈ 1.1 s（首扫，后台） | 删除 `steward.db` 后冷启动实测；遍历开始菜单 `.lnk` + ShellLink COM 解析 + `shell:AppsFolder` UWP 枚举。扫描结果 `mark_seen` 缓存（`SCAN_CACHE_TTL` 24h），冷启动回退读缓存、不阻塞 |
| RSS | 65.2 MB 工作集 / 79.9 MB 私有 | release 常驻；相比 M0 debug 152 MB 明显下降 |
| 呼出延迟 | 8–29 ms（首呼）/ 26–28 ms（二呼） | 窗口启动时已隐藏创建，呼出即 ShowWindow；nucleo 匹配在按键回调内同步完成 |
| 首次查询响应 | ≈ 87 µs | `cargo test -p steward-core-engine bench_query_latency --release` 临时测得：200 条目索引、空查询 + 模糊查询混合，500 次平均 |

> 说明：M1 新增的扫描/索引/查询链路均为同步、无额外进程，逻辑耗时相对 GPU 初始化可忽略；uTools/Raycast 对比待 release 数据在更多机器积累后补齐（目标数据放 `docs/benchmarks.md` 同表）。

## M2 基线（启动即加载 GPUI，呼出瞬时）

环境：Windows 开发机、release 构建、2026-08-20；由 `scripts/bench-resident.ps1` 测量（PostMessage 注入 `WM_HOTKEY` + FindWindow 轮询可见性）。

> 曾实现"常驻态去 GPUI 惰性加载"（托盘-only 私有内存 2.9 MB），实测首呼需 0.5–4.5 s（一次性 DirectX/DirectWrite/着色器初始化），作为启动器不可接受，已回退为启动即加载 GPUI + 隐藏建窗。回退后首呼恢复瞬时，缓存优先/增量扫描与 release 构建优化保留。

| 指标 | 数值 | 备注 |
|---|---|---|
| 启动 → 托盘就绪 | ≈1.4 s（release） | 含一次性 GPUI/DirectX/DirectWrite 初始化，启动静默；旧 debug 基线 ≈1.8 s |
| 常驻 RSS | 94–105 MB 工作集 / 140–147 MB 私有 | 与旧版相当；平台级 DirectX 设备与字体常驻 |
| 首次呼出 → 可见 | 14 ms | 窗口启动时已隐藏创建，呼出即 ShowWindow |
| 二次呼出 → 可见 | 21 ms | 窗口保持隐藏（`CLOSE_ON_HIDE=false`），不做重建 |
| release 二进制体积 | 17.8 MB | 旧 26.7 MB；`lto="thin"` + `codegen-units=1` + `panic="abort"` + `strip` |

## M2 规模化基准（插件系统 v1，待积累）

目标：冷启动与搜索延迟不随安装量线性劣化，只随"实际激活数"变化。

方法（`scripts/gen-test-plugins.sh <count> <dir>`）：

1. 批量生成 N 个测试插件（N=100/500/1000，command / prefix / dynamic 三类
   触发条件轮转），把插件根目录经 `STEWARD_PLUGINS_DIR` 指向被测目录。
2. 冷启动：清空 `%APPDATA%\Steward\plugins.db` 后首启（重建缓存，后台扫描），
   再测带缓存冷启——应只读 SQLite，不做全量文件 I/O。
3. 搜索延迟：分别查询只命中 1 个插件的命令、命中 10 个插件的批量前缀、
   dynamic 参与查询（100ms 熔断）三种场景，对比 100/500/1000 安装量。

判定标准：

| 场景 | 预期 |
|---|---|
| 带缓存冷启动 | 不随 N 变化（读缓存建索引，扫描放后台） |
| 单插件命令查询 | 不随 N 变化（路由过滤后只唤醒匹配插件） |
| dynamic 全量参与 | 稳定 ≤ ~100ms + 往返，且超时插件被熔断跳过 |

> 说明：M2 落地后尚未在本机跑批；数据随 `bench-resident.ps1` 的 release
> 基线在更多机器积累后回填（与 M1 相同的记录方式）。
>
> 插件系统侧的规模化验收由自动化回归测试先行保障（`plugin-host/tests/scaling.rs`）：
> 在超过共享池容量的安装量下，任意索引的插件都能经懒加载正确出视图，且
> `set_plugins` / 单次冷查询耗时不随安装量线性增长。真实 release 整机数据仍待补齐。

## 插件 fs 跨进程 async（M3 二轮后）

插件的 `fs.readFile` / `fs.writeFile` 走**宿主往返**（`host.fs.read` / `host.fs.write`）：
命令 `await` 后 runtime 暂停 isolate（`Pending`），宿主做文件 I/O 后回写响应，runtime
再恢复 Promise。该路径受命令 `deadline_ms`（默认静态 500ms）与 4 MiB 上限双重约束，晚到的
host 回复若 isolate 已被 kill/驱逐则被丢弃。

> 说明：该异步链路的正确性已由 `plugin-runtime` 的 park/resume、并行 `Promise.all`、
> kill/驱逐一致性、parked 超时等单元用例保障；整机延迟/内存影响待 `net.request`
> 落地后随 `scripts/bench.sh` 一起在 release 机器上回填。
