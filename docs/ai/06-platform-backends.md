# Windows 与 macOS 原生后端

## 共同契约

两端都实现 `api::Backend`：poll、按键 disposition、屏幕/光标/前台应用、输入注入、
frame clock、overlay、UI scan、appearance、状态栏开关和 autostart。目标后端由
`src/platform/mod.rs` 在编译期选择。

## Windows

`src/platform/windows/native/` contains the zero-allocation ownership boundary for shared Win32 resources. Owned HWND/HANDLE values and GDI selection guards perform the same single destroy/close/restore operation as the direct code they replace; hot-path forwarding helpers are always inlined. UIA enumeration continues filtering in the native callback instead of allocating an intermediate HWND list.

组合入口：`src/platform/windows/mod.rs`。

### 线程和事件

- 创建 Backend 的线程也是 Win32 message loop、tray、overlay window 的 owner。
- `hook.rs` 是专用低级键盘 Hook 线程，使用每事件 disposition handshake。点击、滚轮和
  键盘注入写入一个固定容量请求槽，再由 Hook 线程退出当前物理键回调后的下一条
  `WM_APP` 消息执行 `SendInput`。这样回调返回与注入之间没有跨线程竞争窗口，也没有
  sleep、轮询或无限队列；连续光标移动仍直接使用 `SetCursorPos`，不经过请求槽。
- 物理左右 Alt 始终立即透传，不延迟也不回放，因此 AHK、Quicker 和 `Alt+物理鼠标键` 能看到真实状态。若随后一个明确绑定的非修饰键被消费，Hook 将带自身标记的未分配 `0xE8` down/up 排入自己的消息循环，回调返回后再发送，以阻止 Alt 松开时激活菜单；失败只报告非致命 warning。
- `accessibility.rs` 是持久 COM MTA UIA worker。
- `frame_clock.rs` 有 DWM 等待 worker，one-slot channel 合并多余帧。
- worker 和 tray 通过 `EventSender` 发 channel，并用自定义 `WM_APP` 唤醒 engine thread。
- `MsgWaitForMultipleObjects`/message pump 保持原生窗口与 Engine poll 集成。

### 子模块

| 文件 | 原生职责 |
| --- | --- |
| `hook.rs` | `WH_KEYBOARD_LL`、同步消费决定及回调后的串行输入注入 |
| `input.rs` | `SendInput`、相对/绝对鼠标、滚轮、键盘和按钮状态 |
| `screens.rs` | per-monitor DPI awareness、显示器与 work area |
| `overlay.rs` | topmost layered click-through HWND、RGBA DIB、文字栅格化 |
| `accessibility.rs` | UI Automation 与 popup/遮挡扫描 |
| `frame_clock.rs` | `DwmFlush` 合成帧 |
| `status_item.rs` | notification-area 图标、菜单、非阻塞更新提示和 Release 页面打开 |
| `autostart.rs` | 当前用户登录启动注册表项 |
| `system_events.rs` | foreground/display/appearance 变化 |
| `console_control.rs` | 控制台关闭和进程退出事件 |

### 权限边界

普通进程不能可靠注入管理员/UIPI 保护窗口。此时 Backend 抛出可恢复输入错误，Engine
清理状态并回到 Idle；不要在 Windows Backend 里伪造成功。

Windows 点击首先以完整 down/up 序列交给一次 `SendInput`；连续单击的双击时间和空间判定
由系统设置负责，显式 double-click 的两个 down/up 对也优先在同一个原生批次中提交。只有
整个批次返回零、确认没有任何边沿插入时，才逐个提交原有边沿作为第三方 Hook 兼容降级；
部分成功的批次绝不重放。降级中途失败时 Backend 保守记录按钮并追加一次 Release，退出时
仍有统一释放兜底；若防御性 Release 也失败，会在下一次鼠标动作前再次释放。

正常注入成功路径不格式化诊断字符串，也不查询进程、令牌或前台窗口。只有请求投递、等待
或 `SendInput` 失败时，错误才附加请求类型、Hook 消息阶段、generation、原生线程 ID、程序
版本、原子批次/单边沿位置，以及当前与前台进程的安全上下文。原子批次失败但逐边沿降级
成功属于异常兼容路径，只在进程内记录一次 warning。

## macOS

`src/platform/macos/native.rs` owns Create/Copy-rule Core Foundation references in a pointer-sized, non-Clone wrapper. It neither retains nor allocates and replaces each former manual `CFRelease` one-for-one, including early-return cleanup.

组合入口：`src/platform/macos/mod.rs`。Backend 必须在主线程创建，因为 AppKit status
item、window 和 display link 都有线程亲和性。

### 线程和事件

- `hook.rs` 在专用 CFRunLoop thread 安装 CGEventTap，并做 disposition handshake。
- AppKit main run loop 负责窗口、菜单栏和 workspace 事件。
- `ui_scan.rs` 有一个持久扫描 worker；Hybrid 内部只在本次 job scope 并发 AX。
- worker/menu event 通过 hook queue 或 channel 发送，并显式唤醒主 run loop。
- frame clock 绑定 overlay cursor view，跨屏后 AppKit 自动跟踪目标显示器 cadence。

### 子模块

| 文件 | 原生职责 |
| --- | --- |
| `hook.rs` | CGEventTap、权限失败诊断、按键消费 |
| `input.rs` | Core Graphics 鼠标、滚轮、键盘事件 |
| `screens.rs` | NSScreen/CG display 坐标与变化监听 |
| `overlay.rs` | nonactivating click-through AppKit window 与 Core Graphics 绘制 |
| `display_link.rs` | macOS 14 `NSView.displayLinkWithTarget:selector:` |
| `accessibility.rs` | AXUIElement 流式树遍历 |
| `vision.rs` | Rust FFI 封装和视觉候选后处理 |
| `vision_bridge.m` | ScreenCaptureKit + Vision Objective-C bridge |
| `workspace.rs` | 前台应用、appearance、run-loop wait/wake |
| `status_item.rs` | NSStatusItem 菜单、非模态 NSAlert 和 NSWorkspace 页面打开 |
| `permissions.rs` | Accessibility trust 检测、提示和设置入口 |
| `autostart.rs`/bridge | ServiceManagement `SMAppService` 登录项 |

Core Graphics 要求合成事件显式携带 click state。Backend 按 `NSEvent.doubleClickInterval`
跟踪按钮、位置和连续点击次数，并从 CGEventTap 合并实体鼠标的 click state；因此两个快速
`left_click`、显式 `double_click`、多击，以及实体鼠标与键盘点击的混合序列保持同一语义。
按下/抬起分离的长按或拖动不会被误报为 click。游标旁 indicator 使用整数逻辑尺寸，避免
Retina 下快速重绘时文字基线出现单帧纵向抖动。

### 权限和应用身份

- 键盘捕获需要 Accessibility；缺失时 Backend 仍能启动菜单栏，但
  `keyboard_available=false` 并给出说明。
- Vision 屏幕内容检测需要 Screen Recording。
- 权限绑定应用 bundle identity；正式用户必须运行打包的 `KeySteer.app`，不能让 Terminal
  代替应用申请权限。
- `SMAppService` 需要 bundle 上下文，裸二进制不等同正式 `.app` 登录项。

## 新增平台的最小边界

新增 `src/platform/<os>/` 和 `platform/mod.rs` 的一个 cfg arm，实现 Backend。不要修改
Mode 以适配平台。配置中的 platform-specific 字段仍应在所有目标可反序列化，这样同一
TOML 可跨平台复用，但只有对应 Backend 消费它。
