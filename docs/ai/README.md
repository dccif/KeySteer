Restore 内恢复／删除状态合并、移除独立 Delete 配置见 [配置](03-configuration.md)、[模式状态机](04-modes-and-lifecycle.md) 和 [验证](08-build-docs-and-tests.md)。

Window 入口激活鼠标下窗口、可配置关闭动作见 [公共 API](02-runtime-and-api.md)、[模式](04-modes-and-lifecycle.md)、[原生后端](06-platform-backends.md) 和 [验证](08-build-docs-and-tests.md)。

# KeySteer 项目手册

Window 区域编号回收、联动尺寸调整、常驻按键提示即时刷新、全部编号统一避让、工作区预设保存和恢复、窗口候选范围、状态切换中的稳定编号与重入编号回收见 [公共 API](02-runtime-and-api.md)、[配置](03-configuration.md)、[原生后端](06-platform-backends.md)、[渲染](07-rendering-and-performance.md) 和 [验证](08-build-docs-and-tests.md)。

本目录是给维护者和 AI 使用的代码地图，记录当前实现中的模块边界、数据流和不变量。它不是普通用户教程；用户请先读[快速上手](/guide/getting-started)，开发者请先读[架构](/development/architecture)和[开发流程与测试](/development/workflow)。

## 事实来源

遇到冲突时按以下顺序判断：

1. 当前源码和测试。
2. `keysteer.default.toml` 中实际发布的默认行为。
3. 本目录中的专题说明。
4. 面向用户的参考文档。

版本、最低 Rust 版本和依赖以 `Cargo.toml`、`rust-toolchain.toml` 和 `package.json` 为准，不在本页重复维护。

## 最短阅读路径

Window 组内优先切换、同程序编号和整组最小化见 [公共 API](02-runtime-and-api.md)、[原生后端](06-platform-backends.md) 和 [验证](08-build-docs-and-tests.md)。

标签栏的布局占位、完整成员卡片、方向绑定、滚动、模式外拖拽、窗口堆叠和透明度显隐改动见 [公共 API](02-runtime-and-api.md)、[配置](03-configuration.md)、[原生后端](06-platform-backends.md)、[渲染约束](07-rendering-and-performance.md) 与 [原生验收](08-build-docs-and-tests.md)。

| 任务 | 先读 |
| --- | --- |
| 找模块和入口 | [项目地图](01-project-map.md) |
| 修改启动、事件路由、输入消费确认或动作执行 | [核心运行时与公共 API](02-runtime-and-api.md) |
| 修改 TOML、按键提示配置、字符/物理按键、临时模式激活键消费与入口隔离、配置优先级、Reload、继承或持久化 | [配置、按键和持久化](03-configuration.md) |
| 修改鼠标侧键触发与模拟点击、组合键前缀仲裁、跨屏窗口移动（含 macOS 原生全屏异步过渡）或鼠标跟随 | [配置与按键](03-configuration.md)、[核心运行时与公共 API](02-runtime-and-api.md)、[模式与插件](04-modes-and-lifecycle.md)、[原生后端](06-platform-backends.md) |
| 修改 Mode、插件或 Finish | [内置模式、插件与 Finish](04-modes-and-lifecycle.md) |
| 修改 Window 三态循环、库存、编号聚焦、约束布局、模式交接、跨平台活动成员标签组、模板保存/恢复/删除、底部备注输入、撤销/重做或初始状态恢复 | [模式状态机](04-modes-and-lifecycle.md)、[窗口请求 API](02-runtime-and-api.md)、[配置与工作区预设库](03-configuration.md)、[原生后端](06-platform-backends.md)、[验证](08-build-docs-and-tests.md) |
| 修改 UIA、AX、OCR、Vision、扫描范围、跨屏重扫或扫描超时 | [UI Hint 扫描链路](05-ui-scanning.md) |
| 修改原生平台能力、后台输入恢复、登录项或状态栏生命周期 | [Windows 与 macOS 后端](06-platform-backends.md) |
| 修改 Quick 配置比例尺、Window 顶部输入与分割线、固定快捷键缓存、语义分组提示、独立双栏、底部模式栏或退出键布局 | [覆盖层与按键提示](07-rendering-and-performance.md)、[运行时与 API](02-runtime-and-api.md)、[验证](08-build-docs-and-tests.md) |
| 修改统一 presentation、视图端口、覆盖层、实时按键提示、Window 底部恢复列表与锚定面板、标签栏原生事件定位、缓存、帧时钟或性能 | [覆盖层、帧同步与性能](07-rendering-and-performance.md)、[原生后端](06-platform-backends.md) |
| 修改跨平台字符需求/ASCII 过滤、Windows 布局候选过滤或输入延迟 | [核心运行时与公共 API](02-runtime-and-api.md)、[原生后端](06-platform-backends.md)、[构建与性能验收](08-build-docs-and-tests.md) |
| 修改构建、打包、文档或测试 | [构建、打包、文档站与测试](08-build-docs-and-tests.md) |
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
- `Mode` 是平台无关状态机，只返回内含 `Command` 的 `CommandBatch`，经 `HostContext` 的 Presenter 端口提交视图。
- `presentation` 统一构造场景，只依赖 `api`；原生后端执行实际绘制。
- `Backend` 是原生边界，负责 Hook、输入注入、屏幕、覆盖层、UI 扫描和状态栏。
- Windows/macOS 由 `cfg(target_os)` 在编译期选择。

## 维护方式

- 文件路径均相对 `keysteer/`。
- 完整默认值以 `keysteer.default.toml` 和 `Config::default()` 为准。
- 文档应记录稳定的模块职责、数据流和不可破坏的约束；实现细节以源码为准。
- 新增模块更新 `01-project-map.md`；改变数据流更新 `02`；改变配置语义更新 `03` 和用户参考；改变原生后端或构建方式更新 `06`/`08`。


Window/Quick/Editor 的应用音量组合键与合并 Tab 提示见 [API](02-runtime-and-api.md)、[配置](03-configuration.md)、[原生后端](06-platform-backends.md) 和 [验证](08-build-docs-and-tests.md)。


Window 音频控制区分应用与系统：V+J/K、V+H/L 和 Shift+V+J/K、Shift+V+H/L，见 [API](02-runtime-and-api.md)、[配置](03-configuration.md) 与 [原生后端](06-platform-backends.md)。


独立跨平台音频请求（按键 → Command::AudioRequest → Engine → Backend）及 macOS 系统音频/应用 Tap 见 [API](02-runtime-and-api.md)、[原生后端](06-platform-backends.md)、[项目地图](01-project-map.md) 和 [构建验收](08-build-docs-and-tests.md)。
