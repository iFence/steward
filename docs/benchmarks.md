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
