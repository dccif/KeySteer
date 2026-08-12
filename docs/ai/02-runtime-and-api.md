# 核心运行时与公共 API

## 启动链路

```text
main.rs
  -> app::prepare_console_for_cli
  -> app::run_cli
     -> logging::init + panic hook
     -> cli::parse_args
     -> bootstrap::run
        -> Config::discover/load 或 Config::default
        -> platform::backend
        -> Engine::new
        -> modes::built_in 注册
        -> plugins::bundled 注册
        -> Engine::run
```

Windows 二进制默认无控制台；只有携带 CLI 参数时才尝试附加父控制台。无配置文件不是
错误：程序使用 `Config::default()` 静默进入托盘/菜单栏和 `idle`。

诊断统一经过 `app::logging`：`report_error` 与 panic 不受 debug 配置控制，始终写入并
立即 flush；debug/info/warning（包括 `report_warning`）继续受 `debug.enabled` 控制。
首选日志目录不可写时退到系统临时目录的 `KeySteer/keysteer.log`，并把这次降级本身
记录为 ERROR。除日志模块自身的 I/O 故障和 CLI 最后兜底外，应用代码不得直接写 stderr。

## API 边界

`src/api/` 是唯一允许跨层传递的词汇：

- 原生层向上只产生 `BackendEvent`。
- Engine 向 Mode 只发送 `ModeEvent` 和只读 `HostContext`。
- Mode/Plugin 向外只返回 `CommandBatch`；批次只包含 `Command`。
- Engine 将 `Command` 翻译成 `Backend` 调用或新的 Mode 事件。

Mode 不持有 Backend，也不能访问 HWND、NSWindow、UIA 或 AX。Backend 不理解 Grid、
生命周期或绑定继承。

`CommandBatch` 用 safe Rust 内联保存 0、1、2 个命令，第 3 个命令才退化为 `Vec`。大型
`ShowOverlay` 和 `ScanUi` 载荷分别使用 `Arc<OverlayScene>` 与 `Box<UiScanRequest>`，避免
把每个 `Command` 枚举值撑大。调用方优先用 `Command::show_overlay`、`Command::scan_ui`
构造这两个变体。`ModeEvent::Binding` 共享 Engine 已编译的 `Arc<Binding>`，不复制绑定树。
`Backend::send_keys` 接收拥有所有权的事件 `Vec`：Engine 只构造一次批次，Windows 等异步
后端可直接把它移入原生队列，不需要为了跨线程生命周期再次复制；同步后端的默认实现仍按
顺序逐事件发送。
`Backend::update_overlay_positions` 是完整 `present` 之后的可选快路径：Engine 只发送自己
拥有的 cursor/indicator 新坐标；默认返回 `false`，未实现它的后端会自动退回完整场景。

## Engine 拥有什么

`src/app/runtime/mod.rs::Engine` 的状态分为几组：

- 配置/主题：`Config`、`Palette`、当前 `Appearance`。
- 模式：注册表、活动 Mode、modal stack、插件默认绑定和 verb 所有者。
- 输入：当前物理按键、每键 consume/forward disposition、held gesture、合成输入 latch。
- 路由：每个 Mode 的 `CompiledKeymap`、预解析的 temporary-mode chord 和当前应用对应的
  override profile key。
- 异步：动作序列、Mode timer、UI scan id -> owner、frame clock owner。
- 环境：屏幕、权威光标坐标、当前应用。
- 绘制：Mode 原始 scene、加装饰后的最后 scene、可见状态。
- 控制：启用/暂停、退出、配置存储、输入失败抑制。

绑定表只在配置、模式注册或实际生效的 per-app override profile 变化时重建。仅窗口标题
变化但合并后的绑定不变，不应触发表重编译。

## 事件循环

`Engine::run`：

1. `Backend::start`，读取主题、屏幕、光标和前台应用。
2. 重建带 per-app override 的绑定表。
3. 激活 `idle`。
4. 循环调用 `Backend::poll(next_timeout)`。
5. 处理一个 `BackendEvent`，随后触发到期 timer 和延迟动作序列。
6. 退出时释放所有 latched 输入、隐藏覆盖层并关闭 backend。

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
- `SetFrameClock` 使用原生显示帧，不使用 Mode timer 模拟移动帧率。

状态栏的 `CheckForUpdates` 经 BackendEvent 进入 Engine，再由 Backend 启动独立 HTTPS
worker。worker 只查询 GitHub 最新正式 Release，并以 SemVer 对比 `CARGO_PKG_VERSION`；
结果通过 `UpdateChecked` 返回。发现更高版本时原生层直接打开 `/releases/latest`，否则显示
“已经是最新版本”。更新客户端必须显式选择 Cargo 已启用的 `NativeTls` provider，并使用
系统证书库；`ureq` 默认选择的是未启用的 Rustls，不能依赖其默认值。网络、TLS、HTTP 或
响应解析失败统一返回 `Failed`，不得让 worker panic。检查是 single-flight；请求结束后立即
drop request-scoped Agent 及原生 TLS 连接。Windows 提示在线程内使用同线程临时 owner，
关闭后销毁 owner 并结束线程；macOS 提示保持非 modal，OK action 关闭窗口并释放唯一 retained
Alert。不得累积更新 worker、弹窗或连接。它不是启动任务，也没有定时轮询。

## 可恢复输入失败

合成输入在 Windows 高权限窗口等环境可能因权限被拒绝。`command_executor.rs` 把这类
错误标记为 recoverable，Engine 会：

- 清除动作序列、held gesture、timer、scan owner、modal stack 和 frame owner；
- 尽力释放所有 latched 键/鼠标按钮；
- 停止帧时钟，停用当前模式并进入 `idle`；
- 清空逻辑 scene 并隐藏原生覆盖层；
- 对连续失败只报告一次，后续成功后解除抑制。

不要把这种失败改成保留当前 targeting 状态，否则用户会停留在没有可用标识的假状态。
