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

环境：Windows（开发机）、debug 构建、2026-08-19；时间口径同 M0，另加应用索引扫描与首次查询延迟（进程峰值 RSS）。

| 指标 | 数值 | 备注 |
|---|---|---|
| 冷启动 | ≈ 1.8 s（debug） | release 目标 50ms；逻辑不变，耗时集中在 wgpu/GPU 初始化 |
| 应用扫描 + 建索引 | 手动记录 | 遍历开始菜单 `.lnk` + ShellLink COM 解析；扫描结果已 `mark_seen` 缓存，冷启动回退读缓存 |
| RSS | ≈ 152 MB（debug，含 wgpu） | 常驻峰值 |
| 呼出延迟 | 手动记录 | 与 M0 同路：热键 → 前台 10ms 轮询 → show |
| 首次查询响应 | 手动记录 | 空查询读固定 8 行 / 模糊匹配全量索引；`nucleo` 预处理 haystack 每次查询复用 |

> 说明：M1 新增的扫描/索引/查询链路均为同步、无额外进程，逻辑耗时相对 GPU 初始化可忽略；等 Windows release 数据补齐后再做 uTools/Raycast 对比，目标数据放 `docs/benchmarks.md` 同表。

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
