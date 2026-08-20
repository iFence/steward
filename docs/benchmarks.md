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

