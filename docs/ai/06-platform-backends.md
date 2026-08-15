# Windows 与 macOS 原生后端

## Native safety boundary（2026-08）

- Windows DIB/GPU/window 尺寸先通过 `NativeDimensions`；i32 narrowing、BGRA 长度和 `isize::MAX` 约束均在 FFI 前完成。
- 低级 Hook callback 使用 `try_send`，队列忙时 fail-open；timeout warning 以原子单槽合并。
- Vision result 由 Rust RAII owner 释放，读取 slice 前验证 count<=2000 和非空指针。
- COM apartment 显式 `!Send/!Sync`，确保 `CoUninitialize` 回到初始化线程。
- 两个平台入口不放行 undocumented unsafe；每个最小块记录 `SAFETY` 契约。机械门禁当前为
  不高于 250 个 unsafe expression/18 个文件；`domain` 与其余 portable 层使用编译期
  `forbid(unsafe_code)`/测试门禁保持零 unsafe。

## 共同契约

两端都实现 `api::Backend`：poll、按键 disposition、屏幕/光标/前台应用、输入注入、
frame clock、overlay、UI scan、appearance、系统浏览器、状态栏开关和 autostart。目标后端由
`src/platform/mod.rs` 在编译期选择。两端都跟踪本进程已注入的鼠标按钮状态；对已经按住
的同一按钮再次请求 `Press` 必须幂等返回，不得向系统追加第二个 mouse-down。
两端还实现动态 overlay 位置快路径；`None` 表示该层保持原位，跨屏、缩放或任何内容变化
仍由完整 `present` 同步状态。

## Windows

`src/platform/windows/native/` contains the zero-allocation ownership boundary for shared Win32 resources. Owned HWND/HANDLE values and GDI selection guards perform the same single destroy/close/restore operation as the direct code they replace; hot-path forwarding helpers are always inlined. UIA enumeration continues filtering in the native callback instead of allocating an intermediate HWND list.

Web simulator URLs are passed directly to `ShellExecuteW` with the `open` verb.
Do not route fragment-bearing URLs through `explorer.exe`: its command-line parser
may open File Explorer instead of preserving the complete URL for the default browser.

组合入口：`src/platform/windows/mod.rs`。

### 线程和事件

- 创建 Backend 的线程也是 Win32 message loop、tray、overlay window 的 owner。
- overlay worker 在 tray 和屏幕枚举之前启动；消息队列就绪后，GPU device tree 在渲染线程
  与剩余启动工作并行预热，保持第一次进入模式的低延迟。
- 完整 frame 和 cursor/indicator 坐标各使用一个 latest-value 槽；GPU 位置更新只写 visual
  offset 并 commit，不进入 Direct2D；DIB 回退从缓存场景恢复完整绘制。
- `hook.rs` 是专用低级键盘 Hook 线程，使用每事件 disposition handshake。点击、滚轮和
  键盘注入写入一个预分配、固定上限为 32 项的 FIFO，再由 Hook 线程退出当前物理键回调
  后的 `WM_APP` 消息串行执行 `SendInput`。Engine 只入队而不等待 Hook 执行，避免快速
  key-down/key-up 形成“Engine 等注入、Hook 等下一个 disposition”的锁环；相邻请求共用
  一次唤醒，不使用 sleep、轮询、额外线程或无限队列。连续光标移动仍直接使用
  `SetCursorPos`，不经过此队列。
- 键盘事件批次由 Engine 转移所有权到 Hook FIFO。Chord 最多 8 个原生键码直接内联在队列
  请求中，再生成最多 16 个栈内 `INPUT`；超长序列才分配。虚拟键正向查找由同一份定义表
  生成编译期 `match`，不在线性表中逐项搜索，也不维护第二份运行时 map。
- 物理左右 Alt 始终立即透传，不延迟也不回放，因此 AHK、Quicker 和 `Alt+物理鼠标键` 能看到真实状态。若随后一个明确绑定的非修饰键被消费，Hook 将带自身标记的未分配 `0xE8` down/up 排入自己的消息循环，回调返回后再发送，以阻止 Alt 松开时激活菜单；失败只报告非致命 warning。
- `accessibility.rs` 是持久 COM MTA UIA worker，并在 MTA 内复用只读 query plan。
- `vision.rs` 在 backend ready 后的首次事件轮询派发一次低优先级 OCR discovery，缓存系统语言/尺寸与微信绝对路径/文件标识，探测线程结束前不保留引擎或 helper。WinRT OCR/WIC 使用 generation-owned activation factory，不能依赖跨临时 COM apartment 的投影静态缓存。视觉 coordinator 按请求懒启动；每次扫描拥有可取消并可 join 的 system OCR、WeChat OCR 和 fallback provider，当前与 latest pending generation 完成后 coordinator 退出。`ui_scan.rs` 统一流式发布和空间去重；`wechat_ocr.rs` 只在 generation-scoped 隐藏 helper 中加载可选桥接 DLL。
- `frame_clock.rs` 有 DWM 等待 worker，one-slot channel 合并多余帧。
- worker 和 tray 通过 `EventSender` 发 channel，并用自定义 `WM_APP` 唤醒 engine thread。
- Windows Hook、overlay renderer 和 tray 的 readiness 由 `WorkerJoin` 设置 deadline；平台初始化
  失败必须返回错误，不能在 `recv()` 上无限等待。
- overlay、frame clock、Hook、tray、UIA、vision 和 update 都通过公共 `WorkerJoin` 记录 completion、panic
  与 shutdown deadline。frame-clock 的 compositor event 由 worker 自己拥有，Engine 只读取其
  临时 token 发出 interrupt；即使 deadline 失败也不会关闭仍被 worker 使用的 HANDLE。
  `WorkerJoin` 等待超时会保留 `JoinHandle`，调用者可以在同一绝对 deadline 内继续回收，不能
  因第一次短等待超时就静默 detach。
- `MsgWaitForMultipleObjects`/message pump 保持原生窗口与 Engine poll 集成。
- Windows Backend shutdown 先使 discovery/generation 失效，终止微信 helper、关闭 provider IPC 并等待所有视觉线程，再停止 UIA，最后释放 Hook、tray 和 overlay；重复 shutdown/dismiss 必须幂等，不得向已经退出的渲染线程继续发送唤醒消息。

### 子模块

| 文件 | 原生职责 |
| --- | --- |
| `hook.rs` | `WH_KEYBOARD_LL`、同步消费决定及回调后的串行输入注入 |
| `input.rs` | `SendInput`、相对/绝对鼠标、滚轮、键盘和按钮状态 |
| `screens.rs` | per-monitor DPI awareness、显示器与 work area |
| `overlay.rs` | topmost layered click-through HWND、RGBA DIB、文字栅格化 |
| `accessibility.rs` | UI Automation 与 popup/遮挡扫描 |
| `ui_scan.rs` | UIA/视觉共享流式发布、空间去重和组合终态 |
| `vision.rs` | GDI 截图、Windows OCR 与纯 Rust 区域检测 |
| `wechat_ocr.rs` | 微信 OCR 自动发现、PE 校验、WIC PNG 与隐藏 helper IPC |
| `frame_clock.rs` | `DwmFlush` 合成帧 |
| `status_item.rs` | notification-area 图标、菜单、非阻塞更新提示和网页打开请求 |
| `autostart.rs` | 当前用户登录启动注册表项 |
| `system_events.rs` | foreground/display/appearance 变化 |
| `console_control.rs` | 控制台关闭和进程退出事件 |

### 权限边界

普通进程不能可靠注入管理员/UIPI 保护窗口。此时 Backend 抛出可恢复输入错误，Engine
清理状态并回到 Idle；不要在 Windows Backend 里伪造成功。

Windows 点击首先以完整 down/up 序列交给一次 `SendInput`；连续单击的双击时间和空间判定
由系统设置负责，显式 double-click 的两个 down/up 对也优先在同一个原生批次中提交。只有
整个批次返回零、确认没有任何边沿插入时，才逐个提交原有边沿作为第三方 Hook 兼容降级；
部分成功的批次绝不重放。降级中途失败时 Hook 在同一执行上下文立即追加一次 Release；
键盘和弦失败也会释放其中所有 Down 键，退出时仍有统一释放兜底。

正常注入成功路径不格式化诊断字符串，也不查询进程、令牌或前台窗口。只有请求投递或
`SendInput` 执行失败时，错误才附加请求类型、队列阶段、generation、原生线程 ID、程序
版本、原子批次/单边沿位置，以及当前与前台进程的安全上下文。执行失败通过
`InputInjectionFailed` 返回 Engine 并触发统一输入状态复位；成功不产生完成事件。原子批次
失败但逐边沿降级成功属于异常兼容路径，只在进程内记录一次 warning。

## macOS

`src/platform/macos/native.rs` owns Create/Copy-rule Core Foundation references in a pointer-sized, non-Clone wrapper. It neither retains nor allocates and replaces each former manual `CFRelease` one-for-one, including early-return cleanup.

组合入口：`src/platform/macos/mod.rs`。Backend 必须在主线程创建，因为 AppKit status
item、window 和 display link 都有线程亲和性。

### 线程和事件

- `hook.rs` 在专用 CFRunLoop thread 安装 CGEventTap，并做 disposition handshake。tap 的创建与
  status item、frame clock、屏幕和 workspace 初始化并行，Backend 完成前才启用，启动期间
  的异步事件先进入 fallback channel，成功后通过共享 `OnceLock` 路由到有界 Hook 队列。
- AppKit main run loop 负责窗口、菜单栏和 workspace 事件；wake 使用 typed
  `objc2-core-foundation` 接口。workspace 先比较 PID，只有前台进程变化时才分配 bundle ID，
  appearance 与静态 `NSString` 直接比较。
- `ui_scan.rs` 有一个持久扫描 worker；Hybrid 内部只在本次 job scope 并发 AX。
- worker/menu event 通过 hook queue 或 channel 发送，并显式唤醒主 run loop。
- frame clock 绑定 overlay cursor view，跨屏后 AppKit 自动跟踪目标显示器 cadence。
- 高频 Pointer 位置使用原子 seqlock latest-point mailbox；队列忙时只覆盖旧位置，按键和
  capture-loss 仍走各自可靠路径，因此 EventTap callback 不再竞争 `Mutex<Point>`。
- cursor/indicator 位置更新直接在禁用隐式动画的事务中修改已有 CALayer frame，不构造
  `NSString`、颜色、路径或完整 scene；隐藏或首帧未完成时由 Engine 回退完整提交。
- Backend 缓存一个轻量 `CGEventSource`；单键与拥有所有权的组合键批次复用它，批次先验证
  全部键码再注入。输入注入使用 `objc2-core-graphics` 的 typed retained/borrowed API，
  `input.rs` 编译期 `forbid(unsafe_code)`；正向键码使用由反向表同源生成的编译期 `match`，
  修饰键 Hook 复用预热 `Key`。

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
| `status_item.rs` | NSStatusItem 菜单、非模态 NSAlert 和网页打开请求 |
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
- 运行中撤销 Accessibility 时，`TapDisabledByUserInput` 被视为用户意图：Hook 立即
  fail-open 并停止，不自动重新启用。capture-loss 使用独立原子单槽，不经过可能已满的
  Hook 队列；Backend 在旧物理输入前交付它并丢弃旧 KeyDown/Pointer，Engine 随即停止帧
  时钟、释放合成输入并回到 Idle。撤权路径不在 TCC 正在改动时调用
  `AXIsProcessTrusted`。`TapDisabledByTimeout` 全进程只自动恢复一次，再次超时同样停止。
- macOS Backend 的幂等 shutdown 顺序为：先移除 status item/系统回调，再停止 display
  link、失效并停止 UI scan、取消更新、释放 held input、停止 Hook、关闭 overlay。扫描和更新
  event tap、扫描和更新使用公共 `WorkerJoin`；成功 shutdown 必须完成 join，deadline 超时作为错误传播并立即退出
  进程，不能在后台 worker 仍存活时继续驻留。
- Vision 屏幕内容检测需要 Screen Recording。
- Vision 自动扫描只做 Screen Recording preflight；权限申请不得从扫描 worker 发起，以免
  在用户修改 TCC 设置时与 System Settings 竞争。
- 权限绑定应用 bundle identity；正式用户必须运行打包的 `KeySteer.app`，不能让 Terminal
  代替应用申请权限。
- `SMAppService` 需要 bundle 上下文，裸二进制不等同正式 `.app` 登录项。

## 新增平台的最小边界

新增 `src/platform/<os>/` 和 `platform/mod.rs` 的一个 cfg arm，实现 Backend。不要修改
Mode 以适配平台。配置中的 platform-specific 字段仍应在所有目标可反序列化，这样同一
TOML 可跨平台复用，但只有对应 Backend 消费它。
