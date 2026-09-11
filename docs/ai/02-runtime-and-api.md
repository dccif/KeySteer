# 核心运行时与公共 API

`Mode::window_action_supported` 描述模式稳定支持的 Window 动作，用于模式进入时构建固定快捷键表；`window_action_available` 描述当前能否执行，仍保留编辑事务、恢复输入和确认状态的检查。两者共享 WindowKind 的动作集合，帮助表不会因瞬时未就绪而丢失保存或确认键。Host 的帮助解析使用独立候选状态，实际输入解析仍使用真实按键状态。

## 合成指针跨屏通知

MovePointer / WarpPointer 成功更新权威 cursor 后，若 active_bounds 改变，立即向当前模式派发
PointerMoved，使 UI Hint 重扫新屏幕；不依赖可能被原生 Hook 忽略的合成事件。同屏移动不增加该派发。


WindowRequest 的 scope 统一约束库存、数字选择与循环候选；模式编译为当前屏幕或全部屏幕及 include_minimized。同一会话内，暂时最小化或退出候选范围不删除编号；明确关闭才释放，进入新会话时重建编号，跨 Window 家族切换采用目的模式自身配置；范围变化立即刷新库存。BeginEdit 在全部屏幕范围捕获各屏候选；ApplyLayout.additional_screens 在各屏工作区分别映射归一化矩形，合成一个原生事务，失败整体回滚、退出后一次撤销覆盖全部参与窗口。Quick 仍只操作所选窗口。

Windows 原生标签拖拽通过 `TabNativeEvent::Drop(TabDrop)` 传递来源窗口／组、目标组和插入位置；共享协调器验证身份、提交一次可撤销事务并在失败时恢复。事件由持续存在的窗口 worker 消费，退出 Window 模式后仍可合并、转移和排序。

分组对 Window 会话返回“标签栏＋活动应用”的外框，原生 `TabBar.bounds` 仍指应用内容框。`Grouped` 在布局写入、快照／还原及最小高度之间转换，标签栏高度由 `WindowAccess::tab_bar_height` 提供；原生 `tab_fit_frame` 在工作区边缘预留空间。分组历史保存真实应用快照，普通布局历史保存逻辑外框，不能混用而重复扣减栏高。

## 启动链路

Window 的数字、Tab 与 Shift+Tab 继续发送 `Select(WindowId)`、`Cycle`、`CyclePrevious`，不增加全局或容器专用键盘捕获。反向动作名为 `window_select_previous`，与 `window_select` 共用切换逻辑：当前目标在标签组内时，按组成员顺序从活动成员开始循环；组外继续使用稳定窗口环。数字直选仍可跳出组。`WindowAccess::tab_selected` 校验原生焦点通知是否仍然有效。共享协调器先准备并显示新成员，尝试转移系统焦点，最后隐藏旧成员；焦点失败仍提交成员可见性，不能撤销已完成的成员切换。Windows `WindowAccess` 提供原生消息等待与 mailbox 唤醒端口；Engine/Mode 不接触原生消息或线程身份。

跨平台标签组只通过 `WindowOperation::Tabs`、`TabState` 和 `TabNativeEvent` 跨层。关闭独立标签栏报告 `Dissolve(TabGroupId)`。`WindowAccess::tab_set_hidden` 封装单窗口的可见性；共享协调器负责切换时对齐新成员和恢复隐藏租约。Mode/Engine 不接触 HWND、AX 或原生窗口所有权。

```text
main.rs
  -> app::prepare_console_for_cli
  -> app::run_cli
     -> logging::init + panic hook
     -> cli::parse_args
     -> bootstrap::run
        -> ConfigFile::discover/load 或 ConfigFile::default
        -> app::configuration::compile -> RuntimePlan
        -> platform::backend
        -> Engine::from_plan
        -> Engine::run
```

Windows 二进制默认无控制台；只有携带 CLI 参数时才尝试附加父控制台。无配置文件不是
错误：程序使用 `Config::default()` 静默进入 Windows 托盘或 macOS 顶部状态区域并进入 `idle`。

诊断统一经过 `app::logging`：`report_error!`/`report_error` 与 panic 不受 debug 配置控制，始终写入 stderr 和日志文件并
立即 flush；debug/info/warning（包括 `report_warning`）继续受 `debug.enabled` 控制。
首选日志目录不可写时退到系统临时目录的 `KeySteer/keysteer.log`，并把这次降级本身
记录为 ERROR。包括 CLI 在内的应用代码不得直接写 stderr；初始化和日志 I/O 失败也只能
通过 `logging.rs` 内部的 emergency console 路径输出。
Error 写入或同步 flush 失败时，logger 会立即丢弃当前文件 sink；下一条记录重新打开日志
文件，不能让一个失效句柄永久吞掉后续错误。错误格式化、emergency 输出和聚合收敛都属于
`cold`/`inline(never)` 路径，正常输入和绘制路径不承担后台日志线程、channel 或额外分配。

清理和 shutdown 使用安全 Rust `ErrorBundle` 保存带阶段名的全部失败：主流程继续向上返回的
错误只在最终边界记录一次；已经被吞掉并继续运行的真实错误立即调用 `report_error!`。关闭
不能因第一项失败而跳过后续按键、鼠标、扫描或原生 owner 的释放。日志轮换前必须成功 flush；
emergency stderr 使用不 panic 的 `Write` 路径，且不创建后台日志线程。

后台线程统一由 `WorkerJoin` 持有 completion 与 `JoinHandle`。join deadline 超时后 handle
仍由调用方或显式 quarantine owner 持有，禁止静默 detach；backend poll 仅在 quarantine
非空原子标志置位时才加锁回收，空闲热路径不执行无意义的 Mutex 操作。可失败的 worker、
系统回调 owner 在 stop 成功前继续留在 backend 中；显式 shutdown 已经返回过的失败不会由
后续 Drop 重复记录，Drop 只重试尚未完成的阶段。

## API 边界

托盘 `OpenConfigSimulator` 一次性读取当前 TOML source 和 `PresetStore::export_file`；有布局文件时通过 v2 zlib/base64url fragment 传递 `{source, presets, preset_error}`（presets 为同一二进制文件的 base64url），无文件保留 v1 source-only 兼容。浏览器同步清除 fragment 后有界解压并校验两份数据；文件读取错误保留按键配置导入并显示布局错误。v2 解压上限 2 MiB、URL fragment 仍限 24 KiB，文件/源码继续各自限制。只在显式打开、R 列表及保存时读文件，没有文件监听。

Window 保存/列表经 `Command::WindowLayouts` → Engine `PresetController` → `PresetStore`，结果用带 session 的 `ModeEvent::WindowLayouts` 返回。保存时 Backend 异步展示通用 `TextPrompt`，`BackendEvent::TextPromptResult` 以独立请求 id 匹配；备注期间输入保持原生 down/up 配对并 forward，保留窗口／网格覆盖层，暂时撤下帮助列表；TextPrompt 携带工作区底部矩形，原生输入控件无标题栏并置顶。取消 Window 会话先取消所属原生输入框，迟到的结果不能写入文件。保存、取消和错误后模式重新请求目标激活并绘制。磁盘和文本 UI 只在显式保存/列表操作触发，不进入帧热路径。

跨屏窗口移动通过 `Command::MoveWindowToScreen(WindowScreenTarget)` 到
`Backend::move_window_to_screen`。窗口命中与位置查询按动作执行时的物理鼠标完成，不使用
前台应用代替鼠标下窗口。后端返回 `Option<Point>`：提交移动后返回对应的鼠标目标位置，
Engine 复用 `WarpPointer` 同步物理鼠标、权威 cursor、拖动状态和覆盖层；无目标返回 `None`，
移动请求失败不触发 warp。未实现此可选能力的 Backend 返回明确的不支持错误。
macOS 原生全屏移动启动后也返回 `None`；后端保留窗口，按 poll 推进退出全屏、跨屏、恢复全屏，
最终发送 `BackendEvent::WindowMoveCompleted(Result<Point, String>)`。Engine 只在成功时复用
`WarpPointer`，失败只记录错误；后台窗口移动错误不作为输入注入失败清空键盘状态。
组合键前缀在 `registry` 重编译时构建；`prefix_chords` 只持有尚未执行的动作及共享候选，
不拥有系统资源、timer 或按键重放。它复用正常绑定执行路径并先完成原生 disposition 握手。

`src/api/` 是唯一允许跨层传递的词汇：

- 原生层向上只产生 `BackendEvent`。
- Engine 向 Mode 只发送 `ModeEvent` 和只读 `HostContext`。
- `HostContext::present(View)` 通过注入的 `Presenter` 同步构造场景；Mode 只提交借用数据，具体布局归 `presentation`。
- Mode/Plugin 向外只返回 `CommandBatch`；批次只包含 `Command`。
- Engine 将 `Command` 翻译成 `Backend` 调用或新的 Mode 事件。

Mode 不持有 Backend，也不能访问 HWND、NSWindow、UIA 或 AX。Backend 不理解 Grid、
生命周期或绑定继承。

`CommandBatch` 用 safe Rust 内联保存 0、1、2 个命令，第 3 个命令才退化为 `Vec`。大型
`ShowOverlay` 和 `ScanUi` 载荷分别使用 `Arc<OverlayScene>` 与 `Box<UiScanRequest>`，避免
把每个 `Command` 枚举值撑大。调用方优先用 `Command::show_overlay`、`Command::scan_ui`
构造这两个变体。`ModeEvent::Binding` 共享 Engine 已编译的 `Arc<Binding>`，不复制绑定树。
Engine 的 Frame、指针、按键等通用热路径直接调用借用式 `Mode::handle`；只有 `UiScanned`
通过带默认实现的 `Mode::handle_owned` 交付。现有 Mode/Plugin 仍可只实现 `handle`，UI Hint
则消费扫描目标的 Vec 和 String，避免从平台 mailbox 再复制一份。
`Backend::send_keys` 接收拥有所有权的事件 `Vec`：Engine 只构造一次批次，Windows 等异步
后端可直接把它移入原生队列，不需要为了跨线程生命周期再次复制；同步后端的默认实现仍按
顺序逐事件发送。完整 chord 使用带默认实现的 `Backend::send_chord`；内置 Windows 后端把
最多 8 个原生键码内联进异步队列，macOS 直接按切片注入，均不构造 down/up 临时 Vec。
`Backend::update_overlay_positions` 是完整 `present` 之后的可选快路径：Engine 只发送自己
拥有的 cursor/indicator 新坐标；默认返回 `false`，未实现它的后端会自动退回完整场景。

## Engine 拥有什么

`src/app/runtime/mod.rs::Engine` 是组合根；具体状态按所有权分布在相邻协作者中：

- 计划/主题：`EngineSettings`、`PaletteSet` 和当前 `Appearance`；Mode 不读取 TOML。
- 模式：稳定的连续 `ModeRegistry`/`ModeSlot`、缓存的活动 slot、modal stack、插件默认绑定
  和 verb 所有者。slot 同时拥有该 Mode 的 binding table、temporary chords 和 pointer
  interest；Frame/Pointer 的常见分派不再重复走树查找。
- 输入：`InputState` 持有物理按键、每键 consume/forward disposition、held gesture、合成输入 latch。
- 路由：每个 ModeSlot 的 `CompiledKeymap`、预解析的 temporary-mode chord 和当前应用对应的
  override profile key。`CompiledKeymap` 内部仍保持已经验证过的结构，不使用曾回退的排序 Vec。
- 异步：`Scheduler` 持有动作序列、Mode timer 和 frame clock owner；scan id 仍映射 owner。
- 环境：屏幕、权威光标坐标、当前应用。
- 绘制：`OverlayCoordinator` 持有统一 composer 生成的内容 scene、最后 scene、去重和位置快路状态。
- 控制：启用/暂停、退出、配置存储、输入失败抑制。

`RuntimePlan` 不再分别保存 route、Mode 和 Plugin 三套可失配集合。每个 `ModeSpec` 原子包含
实例、`ModeRoute` 和实例种类；Engine 安装计划时从同一项建立 registry slot、路由和插件
manifest/default binding。`registry.rs` 将 Mode 注册、查找、路由编译、事件分派和生命周期放在同一
模块中，避免同一模式子系统横跨多个只包含 `impl Engine` 的文件。输入、scheduler 和 overlay
的状态分别只存于 `InputState`、`Scheduler` 与 `OverlayCoordinator`。

绑定表只在配置、模式注册或实际生效的 per-app override profile 变化时重建。仅窗口标题
变化但合并后的绑定不变，不应触发表重编译。

路由编译同时建立 `char -> Key` 索引，仅收录单键字符 chord。字符查找直接借用索引中的 Key，
不再逐模式寻找字符；真正的绑定仍通过原有模式优先级、继承和 Disabled 规则解析。Engine
在启动、运行时重载和有效应用覆盖变化时，以 `Backend::set_character_bindings` 同步字符观察
需求，普通按键处理不遍历配置或重建该索引。

## 事件循环

`Engine::run`：

1. `Backend::start`，读取主题、屏幕、光标和前台应用。
2. 重建带 per-app override 的绑定表。
3. 激活 `idle`。
4. 循环调用 `Backend::poll(next_timeout)`。
5. 处理一个 `BackendEvent`，随后触发到期 timer 和延迟动作序列。
6. 退出时释放所有 latched 输入、隐藏覆盖层并关闭 backend。

`Engine::run` 是所有目标的统一入口；启动、单次事件处理和结束辅助函数都是运行时私有实现，
`bootstrap` 不引用 `MacOsBackend`、`NSApplication` 或其他平台专用类型。平台事件循环的差异只
存在于 `Backend::poll`：Windows 集成 Win32 message pump，macOS 在主线程有界派发 AppKit
事件，并通过主 run loop 等到生产者唤醒、显示帧或 Engine 的准确 deadline。状态图标、窗口和
AppKit 生命周期因此完全留在 macOS backend 内，同时保持 Hook、timer、sequence 与 shutdown
的原有顺序。

原生输入捕获永久丢失与普通合成注入失败是两条不同恢复路径。前者通过
`InputCaptureLost` 可靠上报，并额外清空物理 pressed/disposition 状态，因为对应 KeyUp
已经不可能到达；两者都会停止帧时钟、取消扫描、释放合成输入并回到 Idle。Quit 必须调用
Backend 的幂等完整关闭流程；普通发布包返回主函数后不得保留 KeySteer 进程或线程。

当 timer、延迟 sequence 和长按队列同时为空时，Engine 直接使用 50ms 上限，不读取单调
时钟，也不进入三类到期扫描。重复 `KeyState::Down` 只在首次进入 `PressedKeys` 时 clone key；
display mode 只在实际改变 temporary chord 成员的物理边沿上重算。

Engine 始终更新权威 cursor 和动态 overlay 坐标；每个 Mode 通过带默认实现的
`wants_pointer_events` 声明是否接收高频指针事件，并通过 `claims_key` 声明自己的原始字符表。

`Backend::poll` 最多阻塞 50ms 或到下一个 timer/sequence/长按截止时间。延迟序列和长按项
按截止时间倒序保存，最近项位于 `Vec` 尾部；等待只读取尾项，到期只 `pop`，不得在每次
poll 后重新分配并拆分整个队列。超时返回 `None` 是 Engine 执行内部定时任务的机会。

## 键盘同步握手

Windows `WH_KEYBOARD_LL` 和 macOS `CGEventTap` 都必须在原生回调返回前知道按键是否
透传：

```text
native callback -> BackendEvent::Input -> Engine::handle_key
                <- Backend::dispose_key(Consume | Forward)
```

Engine 为每个物理键保存 disposition，确保 key-up 与 key-down 使用同一决定；模式切换
不能造成 down 被吞、up 被放行。held binding 还保存其 owner，松开时不会重新查找一个
已经不匹配的 chord。

原生回调超时必须立即使 mailbox generation 失效；迟到的 `dispose_key` 返回错误，即使
尚无下一次按键。Engine 将其作为可恢复输入错误，中止对应绑定动作并回到 Idle，不退出进程。
Windows Hook 另用固定 bitset 保存实际原生 disposition；重复和松键沿用首次按下的决定，
因此超时兜底不会泄漏已消费的重复键，也不会吞掉已透传按下对应的松键。

`KeyDisposition::Defer` 仅保留公开 API 兼容性，Engine 不再产生它，内置后端按
`Forward` 处理。Windows 不再延迟或重放 Alt。

## 绑定处理分工

- Engine 自己执行离散 host verb：切换模式、点击、send、exec、配置写入、退出等。
- `Move`、`Scroll`、`Speed` 等持续动作以 `ModeEvent::Binding` 交给 owner Mode。
- Mode 直接读字符时收到 `ModeEvent::Key`，例如 Grid 单元键和 UI Hint 标签。
- `Binding::Sequence` 是非阻塞序列；`wait` 把剩余动作放入 `pending_sequences`，不 sleep
  事件线程。

## Command/ModeEvent 中的重要语义

- `ShowOverlay` 替换整张 scene；`HideOverlay` 释放可见内容。
- `FinishMode` 发送 `FinishRequested`，不会重新激活 Mode。
- `RestartMode` 给同一实例发送 `Restarted`，保留原 return mode。
- `MouseButton::Click/DoubleClick` 成功后发送一次 `Clicked`；Press/Release/Toggle 不发送。
- `PushMode/PopMode` 使用 `Suspended/Resumed` 保存下层模式状态。
- `ScanUi` 异步返回 `UiScanned`；scan owner 按 id 路由，旧结果不得进入新 session。
- Mode 退出、完成、禁用或异常恢复时，Engine 调用 `Backend::cancel_ui_scan` 并立即移除
  owner；取消只结束 request-scoped 工作，不销毁平台扫描 worker。
- `SetFrameClock` 使用原生显示帧，不使用 Mode timer 模拟移动帧率。

状态栏的 `CheckForUpdates` 经 BackendEvent 进入 Engine，再由 Backend 启动独立 HTTPS
worker。worker 优先查询 GitHub 最新正式 Release，并以 SemVer 对比 `CARGO_PKG_VERSION`；
GitHub 不可用时从 jsDelivr 版本元数据选择最高稳定 SemVer。回退结果可以发现更高版本，但
版本不高于当前程序时不能证明“已经是最新”，必须返回 `Failed` 让用户稍后重试。结果通过
`UpdateChecked` 返回，发现更高版本后下载对应原生包。更新客户端必须显式选择 Cargo 已启用
的 `NativeTls` provider，并使用系统证书库；`ureq` 默认选择的是未启用的 Rustls，不能依赖
其默认值。网络、TLS、HTTP 或响应解析失败统一返回 `Failed`，不得让 worker panic。检查是 single-flight；请求结束后立即
drop request-scoped Agent 及原生 TLS 连接。Windows 提示在线程内使用同线程临时 owner，
关闭后销毁 owner 并结束线程；macOS 提示保持非 modal，OK action 关闭窗口并释放唯一 retained
Alert。不得累积更新 worker、弹窗或连接。它不是启动任务，也没有定时轮询。
Windows 和 macOS 都下载各自架构的发布 ZIP；公共下载器检查 ZIP 文件头、GitHub 提供的大小
和 SHA-256。Windows 用户确认安装后，平台私有 `update_installer` 只从中央目录声明的固定
`KeySteer/KeySteer.exe` 路径提取候选，兼容打包工具产生的 `/` 与 `\` 分隔符，并严格校验 ZIP
边界、重复项、加密/压缩方法、长度和 CRC-32。解压输出受大小上限约束，不允许目录遍历或
ZIP bomb。之后校验 PE 架构、Windows 版本资源和 Authenticode 文件摘要，并要求
候选与当前进程的叶代码签名证书 SHA-256 指纹一致。完整系统信任链成功时直接接受；自签证书
仅允许 `CERT_E_UNTRUSTEDROOT` 这一种失败，并且仍须与当前 EXE 指纹相同，其他摘要、过期、
撤销或链错误全部拒绝。下载文件验证通过后以受限大小复制到安装目录同卷临时文件，并对暂存
副本重复校验；签名
临时 helper 打开父进程并完成握手后，tray 才发送普通 `Quit`，因此所有 Hook、合成输入、扫描和
overlay 仍走 Engine 的有序 shutdown。helper 使用 `ReplaceFileW` 保存旧 EXE，启动新版本并等到
Engine 已完成初始环境、路由和 Idle 激活后的第一次 backend poll；启动失败或超时必须杀掉候选、
原子恢复旧 EXE 并重启旧版本。当前或候选未签名、签名无效、签名者不同、安装目录不可写时
不得修改当前 EXE，只保留手动安装入口。macOS 仍只下载并交给用户手动替换。
更新 worker 使用 512 KiB 临时栈；`ureq` 显式编译完整的 `native-tls` HTTPS connector，
macOS 整个 worker 运行在 autorelease pool 内，结束时集中释放原生临时对象。
两端 Backend 持有更新任务的 cancel token，并通过公共 `WorkerJoin` 管理完成通知和
`JoinHandle`：正常完成会及时 reap；退出时先取消并最多等待 250ms。成功 shutdown 必须完成
join；超时会作为关闭失败传播并令进程退出，不能在仍运行 worker 时继续驻留。元数据请求和
下载分别有 3 秒、60 秒的整体硬超时。
macOS 更新事件在 Hook 有界队列忙时转入 fallback channel，后台线程不会被状态事件阻塞。

状态栏的 `OpenConfigSimulator` 同样先进入 Engine。Engine 从 runtime 定义的
`ConfigurationRepository` 端口取得当前有效源文本，只在点击时以 zlib + Base64URL 生成 URL fragment，再由 `Backend::open_url` 交给
系统浏览器。配置 fragment 不得放进 query、不得启动本地 HTTP 服务，也不得在启动时预计算。
生成的 source/compressed/encoded/url 均为单次局部值且不缓存；Engine 对成功打开后的快速
重复菜单事件做 2 秒防抖，避免重复压缩和连续打开浏览器标签页。

## Window 请求与会话

T 的命令仍走 `WindowOperation::Tabs(TabOperation)`，结果以 `WindowResult.tabs` 返回不含原生句柄的 `TabState`。`WindowTarget::Window` 只表示一个窗口，`WindowTarget::Group` 才展开全部成员；编号不能替代身份类型。模式只持有输入与展示状态，worker 的 `Grouped<WindowAccess>` 持有真实组合及原生事务。`CancelWindowSession` 结束 Mode 会话，不销毁后台组；仅显式解散或 backend shutdown 释放相应栏和监听。组存在时 worker 有界派发原生事件并合并几何通知，不在 Engine 等待系统写入。

Tab 模板使用 `PresetLibraryOperation::Save { template: WindowTemplate::Tabs(..), .. }` 和现有异步备注流程。恢复先收集全部不同 `WindowId`，完成后只发送一次 `TabOperation::Restore`，原生层预检并在失败时恢复快照。普通 Window 几何历史通过同一包装层操作全组；一次恢复按组代表去重，T 的成员／排序历史独立。

`BeginEdit` 异步返回完整库存与最小尺寸快照；`ApplyLayout` 发送事务 id、递增修订号及标准化绝对矩形；`EndEdit` 结束并分组撤销记录，内部恢复路径仍可请求回滚。移动与编辑即时生效；Q 是当前模式的普通绑定，交接前完成最新布局。Restore／Delete 的 Enter 用于编号确认／删除确认。Mode 保持单个在途批次并合并后续目标；worker 只写入变化矩形，拒绝陈旧修订，原生拒绝后回读并恢复上一成功布局。提交失败必须反馈结束状态，避免模式等待不存在的确认。

Mode 的可选 `help_anchor` / `help_previews` 提供纯几何提示数据。Window 的常显面板由 Engine 的 key_help decorator 合并状态、真实绑定和缩略图，并基于锁定窗口定位。其他模式的可选 key_help 和临时 Normal 保持原语义。

五个 Window 模式走普通 Binding::Mode 激活流程。Mode 的可选 session_group 标识共享会话；prepare_transition 可返回命令批次推迟切换，事务完成后再发 SwitchMode。保留树的交接等到最新修订完成，其余编辑交接等待 EndEdit；整个过程不阻塞 Engine。组内切换在 Deactivated 前转移 session→owner；组外退出释放会话，取消计时器和备注。Window 动作即使继承自其他表也发给当前窗口交互实例，绑定来源仍用于帮助提示和覆盖解析。原生 HWND/AX 引用不进入 API。

请求有 session 和递增 id。`CancelPending` 取消较旧的查询/调整并保留目标、Tab 顺序和撤销；模式同时推进接受结果的下界。退出、捕获恢复、计划替换和 shutdown 取消整个会话并清除路由，晚到结果不能 warp。显示器变化取消旧操作，并按新屏幕数据回读当前目标。

按住临时 Normal 激活键时，`TemporaryModeChanged` 在本次 disposition 发布之后送达 Window，停止窗口连续运动、取消过期操作并隐藏其覆盖层；Normal 继续接收真实绑定。Window 的裸方向键不抢占临时 Normal，完整显式组合键仍优先。其他定位模式在临时 Normal 已有真实绑定或 `none` 时保持旧语义；没有临时绑定时可解析完整的全局模式启动键。

## 可恢复输入失败

合成输入在 Windows 高权限窗口等环境可能因权限被拒绝。`runtime/mod.rs` 的命令执行路径把这类
错误记录为 crate-private `RuntimeError::RecoverableInput`，Engine 按类型而非错误字符串
前缀决定是否恢复；平台文字即使包含相同内容也不能误触发。公开接口仍返回兼容的
`String`。恢复时 Engine 会：

- 清除动作序列、held gesture、timer、scan owner、modal stack 和 frame owner；
- 尽力释放所有 latched 键/鼠标按钮；
- 停止帧时钟，停用当前模式并进入 `idle`；
- 清空逻辑 scene 并隐藏原生覆盖层；
- 对连续失败只报告一次，后续成功后解除抑制。

不要把这种失败改成保留当前 targeting 状态，否则用户会停留在没有可用标识的假状态。

## 字符与物理键

`InputEvent.key` 保留物理键身份，`character` 可选地携带平台读取的单个可打印字符。配置中的单字符（包括符号）按字面解析。实际字符不同于物理键时，Engine 先用该字符查询普通绑定继承链，匹配失败才回退物理键/chord；字符已经由 OS 布局生成，不要求它的生产修饰键参与字面匹配。释放、repeat 与 held gesture 始终使用原物理键配对。输入恢复无需新增字符状态；查询借用已编译 Key，避免每次创建字符串。

## 鼠标侧键绑定

`mouse_x1` / `mouse_x2` 通过普通 `BackendEvent::Input` 参与组合键、继承和 held gesture。
按下、松开仍通过 `dispose_key` 配对。未匹配的侧键总是透传，不受 Mode 的
`captures_keyboard` 影响，不派发原始 `ModeEvent::Key`，也不产生语义 `Clicked`。
原生边沿映射和超时配对见 [原生后端](06-platform-backends.md#鼠标侧键绑定)。

WindowAction::Ratio 使用 held 按键生命周期及现有 FrameClock；RemoveRegion 仅在 Tree 可用。LayoutTree::resize_by 接收屏幕像素位移与 work_area，Mode 负责最小尺寸约束与事务合并。区域编号选择返回原生 Select 请求（空区域返回 WarpPointer），展示本身不调用平台 API。

WindowInfo.minimized 在原生结果中传递最小化状态，Mode 保留目标但 presentation 隐藏其边框和编号。WindowAction/WindowChange::CycleState 对应 size_cycle，不提供旧动作名称的兼容别名。

模式切换在公共输入层记录进入时已按住的物理键。涉及这些键的 temporary_mode_keys 必须等对应键松开后重新按下才生效，避免 Alt+W 等启动组合键在释放过程中误激活目标模式的临时 Normal；显式组合绑定仍使用完整物理按键状态，keep 不重新设门槛。

WindowOperation::Undo/Redo 使用后台会话的双向原生快照历史；ResetInitial { group } 恢复本次会话写过的窗口，作为独立可撤销步骤。三者优先于库存查询，保持异步边界。模式仅发送命令/处理结果，不持有 HWND/AX 或执行原生恢复。

WindowOperation::Acquire 在 worker 内获取鼠标下窗口并通过 activate_window 激活，完成后才返回目标供展示；不移动指针，激活失败通过 WindowResult.message 返回。WindowAction::Close（window_close）只在普通 Window 可用，发送 Close(WindowId)，由原生正常关闭请求处理，不创建撤销历史，不提前退役窗口身份；关闭取消/保存对话框保留目标，真实关闭后沿用库存和 closed 清理。
