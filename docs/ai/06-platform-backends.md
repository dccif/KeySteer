# Windows 与 macOS 原生后端

## Native safety boundary（2026-08）

- Windows DIB/GPU/window 尺寸先通过 `NativeDimensions`；i32 narrowing、BGRA 长度和 `isize::MAX` 约束均在 FFI 前完成。
- 低级 Hook callback 使用 `try_send`，队列忙时 fail-open；timeout warning 以原子单槽合并。
- Vision result 由 Rust RAII owner 释放，读取 slice 前验证 count<=2000 和非空指针。
- COM apartment 显式 `!Send/!Sync`，确保 `CoUninitialize` 回到初始化线程。
- 两个平台入口不放行 undocumented unsafe；每个最小块记录 `SAFETY` 契约。机械门禁当前为
  预算以 `tests/safety_budget.rs` 为准；Window Mover 增加四个 Win32 调用和五个 AX 操作，并同时禁止 `transmute`/`transmute_copy`；`domain` 与其余 portable 层使用编译期
  `forbid(unsafe_code)`/测试门禁保持零 unsafe。

## 共同契约

跨屏窗口移动复用 `common/window_placement.rs`：以窗口与屏幕的最大交集选择源屏，按显示器
编号循环 previous/next。相同屏幕尺寸精确平移（不受两屏任务栏差异影响），不同尺寸按工作区剩余可移动空间的比例映射，
保留窗口大小；超出目标工作区的大窗口至少把左上角放入工作区。
动作开始时只读取一次物理鼠标位置，用于命中和跟随计算。`following_pointer` 保持鼠标在
普通窗口内的偏移；最大化窗口尺寸变化时保留比例，并将结果限制在目标显示器内。
后端提交成功后返回该位置，由 Engine 统一 warp 和同步状态；无目标或请求失败不移动鼠标。

Windows `window_mover.rs` 复用 UIA 的纯 HWND 命中/过滤，不提交扫描。桌面、任务栏、自身及
透明覆盖层不作为目标，也不穿透系统桌面/任务栏移动背后的应用。普通窗口使用异步
`SetWindowPos` 并保留焦点/Z-order；最大化窗口通过异步 `SetWindowPlacement` 同时移动还原
位置、保留最大化状态，显式转换 workspace/screen 坐标。成功表示操作已提交，应用仍可能
限制大小、DPI 行为或拒绝移动；禁止同步等待外部 UI 线程。

macOS `accessibility::move_window_to_screen` 通过 AX 命中鼠标下元素及 `AXWindow`，设置
`AXPosition`；CF 引用由 `OwnedCf` 释放，使用有限 messaging timeout。原生全屏 Space 需要先
退出全屏；不激活应用，也不发送系统快捷键模拟移动。

坐标契约参考 [Microsoft WINDOWPLACEMENT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-windowplacement)
和 [Apple AX hit testing](https://developer.apple.com/documentation/applicationservices/1462077-axuielementcopyelementatposition)。

两端都实现 `api::Backend`：poll、按键 disposition、屏幕/光标/前台应用、输入注入、
frame clock、overlay、UI scan、appearance、系统浏览器、状态栏开关和 autostart。目标后端由
`src/platform/mod.rs` 在编译期选择。两端都跟踪本进程已注入的鼠标按钮状态；对已经按住
的同一按钮再次请求 `Press` 必须幂等返回，不得向系统追加第二个 mouse-down。
两端还实现动态 overlay 位置快路径；`None` 表示该层保持原位，跨屏、缩放或任何内容变化
仍由完整 `present` 同步状态。

可失败的后台 owner 必须在显式 stop/join 成功后才从 Backend 中移除。`WorkerJoin` 超时会
保留 handle 并转交显式 quarantine；backend poll 先检查非空原子标志，只有确有待回收线程
才进入 quarantine 锁。显式 shutdown 已返回的失败只由最终处理边界记录一次；Backend/owner
Drop 仍重试未完成阶段，但不得重复上报同一错误。所有阶段用 `ErrorBundle` 聚合，前一项失败
不能跳过后续输入、扫描、系统回调或原生窗口的释放。

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
- `vision.rs` 在 backend ready 后的首次事件轮询派发一次低优先级 OCR discovery，缓存系统语言/尺寸与微信绝对路径/文件标识，探测线程结束前不保留引擎或 helper。每次扫描从快照生成 `None`/`SystemOnly`/`WechatOnly`/`Dual` 内部计划；系统单路不会构造任何微信 bitmap、WIC、PNG、helper、job、pipe 或 reader。WinRT OCR/WIC 使用 generation-owned activation factory，不能依赖跨临时 COM apartment 的投影静态缓存。视觉 coordinator 按请求懒启动；每次扫描只拥有计划需要且可取消、可 join 的 provider，当前与 latest pending generation 完成后 coordinator 退出。`ui_scan.rs` 统一流式发布和空间去重；`wechat_ocr.rs` 只在 generation-scoped 隐藏 helper 中加载可选桥接 DLL。
- capture 使用 generation 栈上的 `PreparedCapture`，CPU overlay 使用线程绑定 `GdiDibSurface`；两者共用带长度验证的 DIB/DC owner，selected-object guard 通过 Rust 生命周期绑定所属 surface。微信 helper 创建后立即加入 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` job；取消、超时或上下文变化跳过 graceful 等待并终止子进程，reader/pipe/PNG 都在 terminal 前回收。PNG 只写入带独占 owner lock 的进程私有临时目录，启动下一次编码时只清扫确认没有活跃 owner 的同名目录。
- `frame_clock.rs` 有 DWM 等待 worker，one-slot channel 合并多余帧。帧时钟只负责移动动画；
  物理指针 wake 独立跟随 overlay 生命周期，由 `present` 开启、`dismiss` 关闭。停止 H/J/K/L
  的帧时钟不得关闭仍可见模式的指针跟踪。
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
| `update_installer/` | Authenticode 同签名者校验、候选暂存、临时 helper、原子替换、ready/rollback |
| `autostart.rs` | 当前用户登录启动注册表项 |
| `system_events.rs` | foreground/display/appearance 变化 |
| `console_control.rs` | 控制台关闭和进程退出事件 |

Windows 自动安装要求当前 EXE 与候选 EXE 的 Authenticode 摘要有效且叶证书 SHA-256 指纹
完全相同；仅自签根不受系统信任时可以接受 `CERT_E_UNTRUSTEDROOT`，不接受其他 trust failure，
因此用户不需要安装自签证书。临时 helper 是当前已签名 EXE 的字节副本，以隐藏内部参数启动；内部 helper
和微信 OCR helper 都不得附加控制台。父进程握手完成前不能发送 Quit，第一次 backend poll
之前不能发 ready。替换、备份和候选位于同一卷，`ReplaceFileW` 失败或 ready 超时不得留下
半更新状态；用户配置和 `keysteer.default.toml` 不参与替换。

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

- `MacOsBackend::new` 先在主线程完成 accessory 应用初始化并创建、强持有顶部 `NSStatusItem`，
  然后才进入权限检查、Hook 启动和屏幕枚举等可能较慢的工作。直接点开与登录启动共用这条
  backend 初始化路径，不需要应用层的 macOS 专用入口。
- `hook.rs` 在专用 CFRunLoop thread 安装 CGEventTap，并做 disposition handshake。Backend 完成前
  的异步事件先进入 fallback channel，成功后通过共享 `OnceLock` 路由到有界 Hook 队列。
- EventTap 的修饰键和 tap-disabled 状态使用单写者原子字段，不在同步 callback 中锁
  `Mutex<TapState>`。`TapDisabledByUserInput` 在 callback 内只用原子状态、disposition cancel 和
  run-loop stop/wake 立即 fail-open；不做 AppKit、AX 查询或日志 I/O。底层 Mach port 同时注册
  文档化的 `CFMachPort` invalidation callback，TCC 直接销毁 port 而没有投递 disabled event 时也
  会立即进入同一条 capture-loss 路径。Hook 的 CFRunLoop 由 source 或显式 stop 驱动，不再周期
  唤醒或查询权限；普通键鼠事件不读取 lifecycle 状态。invalidation callback 的私有 `info` 不被
  解释，port 地址只用于从冷路径 registry 取得 Arc owner；显式 shutdown 先 disarm、清除 callback
  和 registry，再让 `CGEventTap` invalidate，避免悬垂指针与错误撤权事件。若 active run loop 在
  没有 terminal/stop 的情况下意外返回，同样立即按输入捕获丢失终止，不能原地空转。
- macOS 仍使用公共 `Engine::run`。`MacOsBackend::poll` 每轮有界派发 AppKit 事件，再按 Engine
  提供的准确 timeout 等待主 run loop、display link 或原生生产者；应用层不持有 AppKit delegate、
  observer 或 timer。`workspace` 先比较 PID，只有前台进程变化时才分配 bundle ID，appearance
  与静态 `NSString` 直接比较。
- `ui_scan.rs` 有一个持久扫描 worker；Hybrid 内部只在本次 job scope 并发 AX。
- worker/menu event 通过 hook queue 或 channel 发送，并显式唤醒主 run loop。
- frame clock 绑定 overlay cursor view，跨屏后 AppKit 自动跟踪目标显示器 cadence。
- 高频 Pointer 位置使用原子 seqlock latest-point mailbox；队列忙时只覆盖旧位置，按键和
  capture-loss 仍走各自可靠路径，因此 EventTap callback 不再竞争 `Mutex<Point>`。
- EventTap 还把当前物理 Shift/Ctrl/Option/Command 压缩为一个 `AtomicU8`；只有 Backend 已持有
  鼠标按钮并合成 `mouseDragged` 时才读取它并设置 CoreGraphics flags。普通 MouseMoved、点击和
  双击不读取该状态；capture loss、Hook 停止及 shutdown 都会清零，避免旧修饰键泄漏到后续拖动。
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
| `workspace.rs` | 前台应用、appearance、有界 AppKit 事件派发和 run-loop wait/wake |
| `status_item.rs` | 顶部 NSStatusItem、点击弹出的控制菜单和非模态提示窗口 |
| `permissions.rs` | Accessibility trust 检测、提示和设置入口 |
| `autostart.rs`/bridge | ServiceManagement `SMAppService` 登录项 |

Core Graphics 要求合成事件显式携带 click state。Backend 按 `NSEvent.doubleClickInterval`
跟踪按钮、位置和连续点击次数，并从 CGEventTap 合并实体鼠标的 click state；因此两个快速
`left_click`、显式 `double_click`、多击，以及实体鼠标与键盘点击的混合序列保持同一语义。
按下/抬起分离的长按或拖动不会被误报为 click。游标旁 indicator 使用整数逻辑尺寸，避免
Retina 下快速重绘时文字基线出现单帧纵向抖动。

### 权限和应用身份

- 键盘捕获需要 Accessibility；缺失时 Backend 仍能显示顶部状态图标，但
  `keyboard_available=false` 并给出说明。
- 运行中撤销 Accessibility 时，`TapDisabledByUserInput` 被视为用户意图：Hook 立即
  fail-open 并停止，不自动重新启用；port 被销毁时 invalidation callback 执行同一终态转换。两者
  通过单一原子 lifecycle 竞争终态，立即取消 disposition waiter 并停止 Hook run loop，因此不会
  重复发布 capture loss，也不会重新启用已经被 TCC 关闭的 tap。正常输入不读取该 lifecycle。
  capture-loss 使用独立原子单槽，不经过可能已满的 Hook 队列。Backend 在旧物理输入前
  交付它并丢弃旧 KeyDown/Pointer，Engine 随后取消 scan、释放合成输入、撤下可能存在的普通指示器
  并回到 Idle。撤权路径不在 TCC 正在改动时调用 `AXIsProcessTrusted`。`TapDisabledByTimeout` 全进程
  只自动恢复一次，再次超时同样停止。
- macOS Backend 的幂等 shutdown 分两阶段：第一阶段不等待，先把 Event Tap disposition mailbox
  永久切到 fail-open 终态，再移除 status item、停止 display link、取消 scan/update、释放 held input
  并关闭 overlay；第二阶段才在同一 deadline 内 join Hook、扫描和更新 worker。终态同时唤醒已有
  waiter，并让停止窗口内的后到回调直接透传，因此主线程等待不可立即取消的 Vision 请求时，实体输入
  不会再产生新的 100 ms disposition 等待。显式 shutdown 已返回失败后，Backend `Drop` 不再开启第二轮
  等待；对应 owner 把未结束 worker 转入既有 quarantine，进程退出负责最终终止。
- Vision 屏幕内容检测需要 Screen Recording。
- Vision 自动扫描只做 Screen Recording preflight；权限申请不得从扫描 worker 发起，以免
  在用户修改 TCC 设置时与 System Settings 竞争。
- 权限绑定应用 bundle identity；正式用户必须运行打包的 `KeySteer.app`，不能让 Terminal
  代替应用申请权限。
- `SMAppService` 需要 bundle 上下文，裸二进制不等同正式 `.app` 登录项。
- macOS 顶部状态图标直接使用 `NSStatusBar::systemStatusBar()` 创建一个固定方形
  `NSStatusItem`。直接点开与 `SMAppService` 开机自启都在 `MacOsBackend::new` 的最前段完成
  AppKit launch 并创建状态项，随后立即给 button 设置 `ImageOnly` 和原有 KeySteer 程序 PNG、
  挂接点击时弹出的控制 `NSMenu`，并强持有 item/menu/target/icon 到 shutdown。冷登录期间若
  AppKit 尚未提供 button，仅复用 backend poll 做有界补挂；不创建线程或系统 timer，也不删除、
  重建状态项。这里不创建 `NSApplication` main menu，不做 autosave、可见性轮询或 Dock 切换；
  shutdown 只移除当前唯一 status item。

## 新增平台的最小边界

新增 `src/platform/<os>/` 和 `platform/mod.rs` 的一个 cfg arm，实现 Backend。不要修改
Mode 以适配平台。配置中的 platform-specific 字段仍应在所有目标可反序列化，这样同一
TOML 可跨平台复用，但只有对应 Backend 消费它。
# Input and readiness latency invariants

- macOS synchronous CGEventTap input has a dedicated bounded lane. Top-status,
  update, and other asynchronous events use an independent unbounded lane; poll
  priority is hook, scan/pending, then async. The backend dispatches only a
  bounded AppKit batch per poll turn, then returns to Engine ordering or waits
  on the main run loop until the requested deadline.
- macOS display reconfiguration callbacks use no userdata. Registration and
  explicit removal return errors, eliminating ownership ambiguity and the old
  callback-userdata lifetime risk.
- Windows foreground/window callbacks set atomic failure flags. Logging occurs
  later at the engine or renderer boundary, never inside the OS callback.
- Readiness markers distinguish `hook_ready`, `uia_ready`, `ocr_ready`, and
  `renderer_ready`; `backend_started` is not interactive readiness.
