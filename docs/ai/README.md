# KeySteer 项目手册

本目录是给维护者和 AI 使用的代码地图，记录当前实现中的模块边界、数据流和不变量。它不是普通用户教程；用户请先读[快速上手](/guide/getting-started)，开发者请先读[架构](/development/architecture)和[开发流程与测试](/development/workflow)。

## 事实来源

遇到冲突时按以下顺序判断：

1. 当前源码和测试。
2. `keysteer.default.toml` 中实际发布的默认行为。
3. 本目录中的专题说明。
4. 面向用户的参考文档。

版本、最低 Rust 版本和依赖以 `Cargo.toml`、`rust-toolchain.toml` 和 `package.json` 为准，不在本页重复维护。

## 最短阅读路径

| 任务 | 先读 |
| --- | --- |
| 找模块和入口 | [项目地图](01-project-map.md) |
| 修改启动、事件路由、输入消费确认或动作执行 | [核心运行时与公共 API](02-runtime-and-api.md) |
| 修改 TOML、按键提示配置、字符/物理按键、临时模式激活键消费、配置优先级、Reload、继承或持久化 | [配置、按键和持久化](03-configuration.md) |
| 修改鼠标侧键绑定、组合键前缀仲裁、跨屏窗口移动（含 macOS 原生全屏异步过渡）或鼠标跟随 | [配置与按键](03-configuration.md)、[核心运行时与公共 API](02-runtime-and-api.md)、[模式与插件](04-modes-and-lifecycle.md)、[原生后端](06-platform-backends.md) |
| 修改 Mode、插件或 Finish | [内置模式、插件与 Finish](04-modes-and-lifecycle.md) |
| 修改 UIA、AX、OCR、Vision 或扫描超时 | [UI Hint 扫描链路](05-ui-scanning.md) |
| 修改原生平台能力、后台输入恢复、登录项或状态栏生命周期 | [Windows 与 macOS 后端](06-platform-backends.md) |
| 修改覆盖层、实时按键提示及其缓存、帧时钟或性能 | [覆盖层、帧同步与性能](07-rendering-and-performance.md) |
| 修改跨平台字符需求/ASCII 过滤、Windows 布局候选过滤或输入延迟 | [核心运行时与公共 API](02-runtime-and-api.md)、[原生后端](06-platform-backends.md)、[构建与性能验收](08-build-docs-and-tests.md) |
| 修改构建、打包、文档或测试 | [构建、打包、文档站与测试](08-build-docs-and-tests.md) |
| 对比 0.9.20 与 0.9.13 性能 | [本机性能报告](performance-0.9.20.md) |
| 准备实施跨层改动 | [改动导航与不变量](09-change-guide.md) |
| 检查模块依赖和配置编译边界 | [架构边界与运行计划](10-architecture-boundaries.md) |

## 一句话架构

```text
TOML -> ConfigFile -> app::configuration -> RuntimePlan -> Engine
                                                     -> ModeEvent -> Mode/Plugin
                                                     <- CommandBatch<Command>
                                                           |
                                                        Backend -> OS
```

- `api` 是跨层共享的唯一词汇。
- `Engine` 负责编译按键、维护运行状态、切换模式和执行命令。
- `Mode` 是平台无关状态机，只返回内含 `Command` 的 `CommandBatch`。
- `Backend` 是原生边界，负责 Hook、输入注入、屏幕、覆盖层、UI 扫描和状态栏。
- Windows/macOS 由 `cfg(target_os)` 在编译期选择。

## 维护方式

- 文件路径均相对 `keysteer/`。
- 完整默认值以 `keysteer.default.toml` 和 `Config::default()` 为准。
- 文档应记录稳定的模块职责、数据流和不可破坏的约束；实现细节以源码为准。
- 新增模块更新 `01-project-map.md`；改变数据流更新 `02`；改变配置语义更新 `03` 和用户参考；改变原生后端或构建方式更新 `06`/`08`。
