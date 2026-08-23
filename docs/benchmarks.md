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
