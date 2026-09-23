# 项目目录与代码地图

`modes/normal_targeting.rs` 仅在启用 `normal.targeting` 时组合 Normal 手势与 `modes/targeting.rs::TargetingController`。后者同时供 Grid／Recursive Grid 使用，统一选格、回退、重置、终点和跨屏路径重放；视图仍由各独立模式提交。

`platform/common/window_session/transaction.rs` 统一 worker 的事务调度；`layout_confirmation.rs` 管理布局增量校验、提交、回滚和恢复；`history_confirmation.rs` 管理撤销／重做／初始状态恢复的独立确认及历史提交。原生句柄仍由平台适配器拥有。`api::WindowTextCache` 是模式拥有、presentation 使用的原始卡片文本缓存。

`app/runtime/quick_switch.rs` 负责操作模式中的长按／数字选择、固定排名快照及按键消费配对；`presentation/quick_switch.rs` 只使用 API 数据绘制面板。`app/preset_store/usage.rs` 负责有界 mailbox、事件触发 checkpoint 和后台 writer 生命周期。原生后端仅提供窗口几何和系统退出通知。

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
| `src/config/` | TOML 文档、Mode section DTO、别名规范化、校验、主题与 comment-preserving store | `mod.rs`, `settings.rs`, `aliases.rs`, `validation.rs`, `store.rs` |
| `src/app/runtime/` | 应用私有 Engine、原子运行计划、命令执行及有状态协作者 | `mod.rs`, `plan.rs`, `registry.rs`, `input_state.rs`, `scheduler.rs`, `overlay_coordinator.rs`, `key_help.rs` |
| `src/modes/` | 十个内置 Mode 状态机；UI Hint 私有状态和算法位于 `hint/` | `normal.rs`, `window.rs`, `window/mode.rs`, `grid.rs`, `recursive_grid.rs`, `hint/mod.rs`, `hint/session.rs` |
| `src/presentation/` | 统一布局、样式与场景构建；仅依赖 api | `mod.rs`, `hint/`, `grid.rs`, `recursive_grid.rs`, `window.rs`, `key_help.rs`, `dynamic.rs` |
| `src/plugins/` | 使用公共 API 实现的内置插件 | `builtin/screen_selector.rs`, `builtin/window_mover.rs` |
| `src/platform/common/` | 两端共享的字符需求/ASCII 过滤、原生候选快照、updater、app info、mailbox、batcher 和 spatial index | `mod.rs`, `character_candidates.rs` |
| `src/support/` | 日志、worker、错误聚合和性能探针 | `mod.rs` |
| `src/platform/windows/` | Win32/COM/UIA/GDI/DWM 后端 | `mod.rs` 组合所有子模块 |
| `src/platform/macos/` | AppKit/CGEventTap/AX/Vision/Core Graphics 后端 | `mod.rs` 组合所有子模块；`window_move.rs` 管理原生全屏跨屏过渡，AX 引用由 `accessibility.rs` 持有 |
| `src/platform/unsupported.rs` | 非 Windows/macOS 的可编译占位后端 | 用于检查公共层可移植性 |

## 程序入口

- `src/api/window_presets.rs`：无原生身份的 Layout/Tabs 类型化模板、共享预设库请求和异步文本输入契约。
- `src/app/preset_store.rs` / `preset_store/codec.rs`：有界二进制布局文件、版本检查、原子替换；`runtime/window_presets.rs` 持有备注请求和会话取消。
- `src/modes/window/mode.rs`：五个独立 Window Mode 及共享会话交接；`config/window.rs` 分别定义配置 DTO。
- `src/api/window_tabs.rs`：不透明组身份、明确的窗口／组目标、组操作和原生标签栏消息。
- `src/modes/window/tabs.rs`：连续编号分组、异步确认前的输入队列、组编号前缀和手动模板恢复。
- `src/platform/common/window_tab_model.rs` / `window_tabs.rs`：纯成员转换、独立于 Mode 会话的组管理、原生事务及分组历史。
- `src/platform/windows/window_tabs.rs` / `window_tabs/strip.rs`：WinEvent 监听与独立浮动标签栏；应用可见性租约在 `window_manager.rs`，应用保持顶层窗口。
- `src/platform/macos/accessibility/window_tabs.rs` / `macos/window_tabs.rs`：worker 的 AX observer 与主线程 AppKit 标签栏，mailbox 只传 API 数据。
- `docs/.vitepress/simulator/window-tabs.ts`：与原生模式同步的浏览器组合模型，退出预览模式仍保留组合。
- `src/modes/window/presets.rs` / `view.rs`：恢复／删除编号布局列表与状态数据，由 `presentation/key_help.rs` 在底部面板统一展示；不读文件或调用原生 UI。
- `src/presentation/key_help/window.rs`：Window 帮助的动作语义分组、方向族和返回目标展示；有效返回绑定由 Host 提供，原生绘制与缓存边界不变。
- `src/platform/windows/text_prompt.rs`：已有托盘线程拥有的 modeless EDIT 对话框；macOS 备注面板由 `status_item.rs` 在主线程持有。

- `src/main.rs`：Windows 使用 GUI subsystem；只调用 `app::prepare_console_for_cli()` 和
  `app::run_cli()`。
- `src/lib.rs`：单 crate 模块入口；KeySteer 的兼容目标是二进制行为而非 Rust 库 ABI/API。
- `src/tests/`：crate 内部的跨模块集成测试与默认忽略、需单线程运行的分配预算。
- `src/app/bootstrap.rs`：加载配置、创建 backend/engine、注册内置模式和插件、进入事件循环。
- `src/platform/mod.rs`：唯一的目标平台选择点。
- `src/app/runtime/prefix_chords.rs`：消费已匹配短组合、等待释放或长组合完成；候选关系由 `input_router.rs` 在路由重建时编译。
- `src/platform/common/window_placement.rs`：跨屏窗口位置的纯几何计算；Windows `window_mover.rs` 与 macOS `accessibility.rs` 负责原生操作。
- `src/modes/window/inventory.rs` 负责拥有所有权的库存结果和关闭回收；`src/platform/common/window_session.rs`：Window 按需 worker、请求取消/合并、稳定 Tab 循环和最多 32 步撤销；`window_geometry.rs`：中心缩放、工作区约束、单窗口布局和等面积平铺。
- `src/platform/windows/window_manager.rs` 与 `src/platform/macos/accessibility/window_manager.rs`：worker 私有窗口身份、枚举、尺寸写入与回读。Windows 与旧 Window Mover 共用原生 placement helpers；macOS 复用 `MovableWindow` 和原生全屏过渡状态机。
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
| `window_layout.rs` | 纯 QuickPlacement 与受最小尺寸约束的 BSP LayoutTree |
| `window.rs` | 不透明 `WindowId`、窗口能力/结果、会话请求、窗口动作与带修订号的编辑事务 |
| `window_tabs.rs` | `TabGroupId`、`WindowTarget`、持久组快照与后台标签栏事件 |
| `presentation.rs` | 借用式 View、Presenter 端口和会话级 Hint 分层缓存 |
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
- `status_item.rs` 托盘菜单、更新提示与浏览器打开；`update_installer/` 负责 Windows 签名校验、
  同卷 staging、临时 helper、原子替换、启动就绪确认和回滚；`autostart.rs` 登录启动；
  `system_events.rs` 前台/显示事件。

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

- `docs/.vitepress/release-history.ts`：为启用 `releaseHistory` 的 Markdown 页面折叠首个版本之后的更新说明，保留原始标题和锚点。

- `docs/.vitepress/config-studio/i18n.ts`、`messages.ts`：实例级中英文语言状态与集中显示文案，供页面、控件及双语搜索复用，不修改配置数据。

- `docs/.vitepress/components/ConfigStudio.tsx`：键盘绑定编辑器、屏幕预览、TOML 导入/下载。
- `docs/.vitepress/config-studio/navigation.ts`：展示分类、模式名称、页签、字段归属与搜索；不参与 TOML 序列化。
- `docs/.vitepress/config-studio/fields.ts`：现有可视化字段及搜索标签；`SettingsNavigation.tsx` 提供桌面分类和窄屏两级选择。
- `docs/.vitepress/config-studio/PaneDivider.tsx`：可拖动和键盘调整的工作区分隔条，仅改变展示宽度。
- `docs/.vitepress/config-studio/FittedKeyboard.tsx`：观察容器和完整键盘尺寸，等比缩放并保留正确点击／拖放坐标，避免键帽裁切和嵌套滚动。
- `docs/.vitepress/simulator/grid-region.ts`：Grid／Recursive Grid 选键路径到百分比区域的公共计算，供网格预览和模拟指针复用。
- `docs/.vitepress/simulator/normal-targeting.ts` 与 `config-studio/NormalTargetingControls.tsx`：Normal 盲操预览的无视图定位状态，以及可选配置的网页编辑；`simulator/bindings.ts::displayedBindings` 只在临时触发键按住时显示借用模式的键帽标签。
- `docs/.vitepress/config-studio/ModeStyleControls.tsx`、`CommonConfigControls.tsx`：按当前页面和页签筛选字段，共用窗口卡片样式只在共用外观编辑。
- `docs/.vitepress/simulator/`：浏览器端配置交接、绑定继承和鼠标状态模型及测试。
- `docs/.vitepress/theme/custom.css`：独立模拟器与文档主题样式。
- 文档站只模拟和可视化配置，不加载 Rust/WASM 校验器；真实校验仍由程序和 Rust 测试完成。

## 独立工具

- `tools/benchmark-windows-dist.ps1`：Windows 整进程启动与资源采样入口；启用
  `perf-probe` 时读取插桩后的 `backend_started` marker 供因果诊断，否则只将 `--check` 计为配置检查耗时；两者均不会冒充未插桩 ready 延迟。
  采样期间先保存在内存中，结束后才写 JSON，避免磁盘 I/O 污染测量区间。

- `src/presentation/label_placement.rs`：按显式组身份统一摆放窗口卡片、区域和组编号，避让实际面板并保持引线。


`src/platform/windows/window_audio.rs` 在独立音频 worker 上枚举 Core Audio 会话，只调整目标 PID 与同可执行文件子进程的应用音量。


`src/platform/windows/audio_policy.rs` 隔离 Windows 音频策略的版本化 COM/WinRT ABI；`window_audio.rs` 负责端点枚举、应用输出策略、系统默认输出与系统音量。


`src/api/audio.rs` 是独立跨平台音频请求/结果词汇。`src/platform/macos/window_audio.rs` 拥有后台线程的音频控制器，`audio_bridge.m` 封装 Core Audio 系统控制与进程 Tap 的本地增益/输出路由；音频实现不进入模式模块。


`src/platform/common/audio_worker.rs`：懒启动的独立音频 worker，64 项有界队列、会话取消标记和有界退出。窗口 worker 只解析活动成员与进程创建身份，音频线程重新验证身份后执行原生调用。

- docs/.vitepress/config-studio/CardPositionEditor.tsx：百分比定位图、Pointer Capture 拖动、预设和四边滑块。
- docs/.vitepress/config-studio/CardStylePreview.tsx：卡片字体、颜色、透明度和几何的实时独立预览。

## AX 所属窗口查找

`src/platform/common/accessibility_window.rs`：不依赖原生 API 的有界关系遍历和普通窗口角色规则。macOS 提供保留的 AX 对象、超时和几何校验；可跨平台执行的测试覆盖直接关系缺失／无效、父链回退、循环和遍历预算。


窗口可见元数据匹配：src/platform/common/window_visibility.rs 提供可跨平台测试的矩形／标题消歧，macOS AX 与 Quartz 枚举使用；范围与编号仍由 window_tabs 的公共协调层负责。


## 响应路径的职责拆分

`platform/common/window_session/` 的 `access` 定义原生能力端口，`worker` 管理有界队列和唤醒，`confirmation` 管理普通几何的提交／读回，`history` 管理紧凑撤销快照，`overlap` 管理相交缓存；父模块保留会话状态与事务协调。Windows 的 `native/gdi.rs` 和 `native/uia_cache.rs` 分别封装字体／选入 DC 的生命周期与 UIA 缓存属性读取。`app/preset_store/usage.rs` 复用单个惰性 workspace-io worker，串行处理布局库操作和使用次数落盘。

- `app/runtime/configuration_work.rs`：按序运行配置读取、编译与持久化；Engine 接受候选后才启动下一项。
- `platform/common/window_session/layout_confirmation.rs`：布局提交、独立确认、失败回滚与取消编辑恢复；复用 confirmation 的原生状态／几何状态机。

- `src/modes/text_input.rs`：临时文本输入透传模式，退出使用标准可配置绑定。
