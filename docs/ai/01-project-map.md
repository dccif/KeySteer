# 项目目录与代码地图

## 顶层目录

```text
keysteer/
├─ src/                 Rust 库、程序入口、核心运行时和原生后端
├─ tests/               不导入实现的架构、日志和源码预算护栏
├─ assets/              品牌图、运行时图标和平台图标
├─ packaging/           Windows portable 与 macOS .app 打包脚本
├─ tools/               与主程序解耦的开发/性能测量工具
├─ docs/                VitePress 用户文档、配置编辑器和本 AI 手册
├─ scripts/             文档站静态资源同步脚本
├─ examples/            配置示例说明
├─ keysteer.default.toml 随项目发布的完整默认配置与注释
├─ Cargo.toml           单 crate、按 target 选择依赖、release 优化
├─ build.rs             Windows 资源与 macOS Objective-C bridge 编译
└─ package.json         文档站脚本与前端依赖
```

## `src/` 分层

| 路径 | 职责 | 常见入口 |
| --- | --- | --- |
| `src/api/` | 跨平台公共协议：按键、动作、命令、事件、几何、场景、插件、后端 trait | `api/mod.rs`, `command.rs`, `backend.rs` |
| `src/app/` | 唯一应用聚合根：启动、CLI、配置编译、Mode catalog 和 runtime | `bootstrap.rs`, `configuration.rs`, `mode_catalog.rs`, `runtime/` |
| `src/config/` | TOML 文档、Mode section DTO、别名规范化、校验、主题与 comment-preserving store | `mod.rs`, `modes.rs`, `aliases.rs`, `validation.rs`, `store.rs` |
| `src/app/runtime/` | 应用私有 Engine、原子运行计划、命令执行及有状态协作者 | `mod.rs`, `plan.rs`, `mode_registry.rs`, `input_state.rs`, `scheduler.rs`, `overlay_coordinator.rs` |
| `src/modes/` | 五个内置 Mode 状态机；UI Hint 私有状态和算法位于 `hint/` | `normal.rs`, `grid.rs`, `recursive_grid.rs`, `hint/mod.rs`, `hint/session.rs` |
| `src/plugins/` | 使用公共 API 实现的内置插件示例 | `builtin/screen_selector.rs` |
| `src/platform/common/` | 两端共享的 updater、app info、mailbox、batcher 和 spatial index | `mod.rs` |
| `src/support/` | 日志、worker、错误聚合和性能探针 | `mod.rs` |
| `src/platform/windows/` | Win32/COM/UIA/GDI/DWM 后端 | `mod.rs` 组合所有子模块 |
| `src/platform/macos/` | AppKit/CGEventTap/AX/Vision/Core Graphics 后端 | `mod.rs` 组合所有子模块 |
| `src/platform/unsupported.rs` | 非 Windows/macOS 的可编译占位后端 | 用于检查公共层可移植性 |

## 程序入口

- `src/main.rs`：Windows 使用 GUI subsystem；只调用 `app::prepare_console_for_cli()` 和
  `app::run_cli()`。
- `src/lib.rs`：单 crate 模块入口；KeySteer 的兼容目标是二进制行为而非 Rust 库 ABI/API。
- `src/tests/`：crate 内部的跨模块集成测试与默认忽略、需单线程运行的分配预算。
- `src/app/bootstrap.rs`：加载配置、创建 backend/engine、注册内置模式和插件、进入事件循环。
- `src/platform/mod.rs`：唯一的目标平台选择点。
- `src/platform/common/partial_batcher.rs` / `scan_mailbox.rs`：两端共用的纯计数流式批次与
  generation-aware latest-only 扫描邮箱。
- `src/platform/macos/latest_point_mailbox.rs`：macOS EventTap 使用的无锁 latest-point
  seqlock；只合并高频位置，不承载按键或控制事件。

## `src/api/` 文件定位

| 文件 | 内容 |
| --- | --- |
| `backend.rs` | `BackendEvent`、`KeyDisposition`、`Backend`、`Appearance` |
| `binding.rs` | TOML 右侧动作的 `Binding` 枚举、解析、canonical 输出、序列 |
| `command.rs` | `Command`、`ModeEvent`、`Mode`、扫描请求/结果、`HostContext` |
| `input.rs` | `Key`、`KeyChord`、`InputEvent`、`ModeId`、平台中立别名 |
| `geometry.rs` | `Point`、`Rect`、`Screen`、`UiTarget` |
| `overlay.rs` | RGBA `Color`、标签/形状、光标标记、模式徽章、`OverlayScene` |
| `plugin.rs` | 插件 `Manifest` 和 `Plugin: Mode` |
| `lifecycle.rs` | targeting 生命周期动作 |
| `style.rs` | Mode/overlay 共用的运行期 UI style 与 indicator 设置 |
| `autostart.rs` | 跨平台开机启动 trait |
| `theme.rs` | 已解析的运行时 `Palette` |

## 平台目录速查

Windows：

- `hook.rs` 键盘 Hook 与同步 consume/forward 握手。
- `input.rs` 鼠标、滚轮、键盘注入。
- `overlay.rs` click-through layered window 与软件栅格化。
- `accessibility.rs` UIA 流式扫描、popup HWND、遮挡过滤；`ui_scan.rs` 统一 UIA/视觉流式发布与空间去重；`vision/` 按 worker façade、generation providers、discovery、mailbox、system OCR、capture 和 fallback 分层；`wechat_ocr.rs` 负责微信组件发现与隔离 helper。
- `frame_clock.rs` DWM 合成帧时钟。
- `status_item.rs` 托盘菜单、更新提示与浏览器打开；`autostart.rs` 登录启动；`system_events.rs` 前台/显示事件。

macOS：

- `hook.rs` CGEventTap 与同步 consume/forward 握手。
- `input.rs` Core Graphics 输入注入。
- `overlay.rs` AppKit/Core Graphics 分层覆盖层。
- `accessibility.rs` AX 树遍历；`vision.rs`/`vision_bridge.m` 视觉检测。
- `ui_scan.rs` 单一持久 worker、AX/Vision/Hybrid 调度。
- `display_link.rs` macOS 14 AppKit `CADisplayLink`。
- `workspace.rs` 前台应用、appearance、主 AppKit run loop 的有界事件派发与等待/唤醒。
- `status_item.rs` 顶部 `NSStatusItem`、点击弹出的控制菜单、非模态更新提示与浏览器打开；
  `autostart.rs`/`autostart_bridge.m` 登录启动。

## 文档站

- `docs/.vitepress/components/ConfigStudio.tsx`：键盘绑定编辑器、屏幕预览、TOML 导入/下载。
- `docs/.vitepress/config-studio/ModeStyleControls.tsx`：三种 targeting mode 的轻量样式编辑。
- `docs/.vitepress/simulator/`：浏览器端配置交接、绑定继承和鼠标状态模型及测试。
- `docs/.vitepress/theme/custom.css`：独立模拟器与文档主题样式。
- 文档站只模拟和可视化配置，不加载 Rust/WASM 校验器；真实校验仍由程序和 Rust 测试完成。

## 独立工具

- `tools/benchmark-windows-dist.ps1`：Windows 整进程启动与资源采样入口；启用
  `perf-probe` 时读取插桩后的 `backend_started` marker 供因果诊断，否则只将 `--check` 计为配置检查耗时；两者均不会冒充未插桩 ready 延迟。
  采样期间先保存在内存中，结束后才写 JSON，避免磁盘 I/O 污染测量区间。
