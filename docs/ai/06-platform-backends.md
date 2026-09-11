# Windows 与 macOS 原生后端

Grouped 按 WindowScope 筛选逻辑快照；活动组的隐藏成员仍继承活动成员的屏幕与最小化状态。编号在同一会话／范围内按窗口身份保留：最小化或暂时未进入候选库存不释放号码，恢复后沿用；仅明确关闭才删除，Acquire 或范围配置变化才清空并从 1 重建。可选数字仍只来自当前库存。持久组不随范围切换解散，范围外原生标签栏仍保留所有标题与点击目标，仅省略无效编号（TabBar 数字 0）。Windows ordinary_window_target 允许最小化库存，scannable_target 继续排除最小化 UI 扫描；最小化的屏幕归属从还原矩形确定。macOS 开启包含最小化时额外检查运行应用 AXWindows，普通窗口仍通过当前可见 Quartz 元数据匹配。

## Native safety boundary（2026-08）

- Windows DIB/GPU/window 尺寸先通过 `NativeDimensions`；i32 narrowing、BGRA 长度和 `isize::MAX` 约束均在 FFI 前完成。
- 低级 Hook callback 使用 `try_send`，队列忙时新按键 fail-open；Windows 已按住的键仍遵循原生 down/up 配对；timeout warning 以原子单槽合并。
- Vision result 由 Rust RAII owner 释放，读取 slice 前验证 count<=2000 和非空指针。
- COM apartment 显式 `!Send/!Sync`，确保 `CoUninitialize` 回到初始化线程。
- 两个平台入口不放行 undocumented unsafe；每个最小块记录 `SAFETY` 契约。机械门禁当前为
  预算以 `tests/safety_budget.rs` 为准；Window Mover 增加四个 Win32 调用和五个 AX 操作（含保留窗口的 messaging timeout），并同时禁止 `transmute`/`transmute_copy`；`domain` 与其余 portable 层使用编译期
  `forbid(unsafe_code)`/测试门禁保持零 unsafe。

## Window 原生会话

活动成员的原生最小化通知会通过 `tab_minimize` 同步收起组内其他成员。Windows 仅提交最小化，不先还原或移动隐藏成员；macOS 直接写逐窗口 AXMinimized。处理同批旧焦点通知时不允许它重开组，最小化组关闭成员也不主动激活剩余成员。恢复仍只显示选中的活动成员，其他成员保持收起，组不解散。

首次编号按程序分批，已锁定程序的已有编号使同程序的新窗口优先获得相邻编号；后续刷新保留已有号码，只追加新窗口。`api::window::application_number_order` 是公共纯排序规则，后台组协调器与 Window 的库存编号共用；不为已有号码连续性而重新洗牌。

标签栏占用独立的顶部空间：Windows 为 `round(30 * scale)` 物理像素，macOS 为 30 逻辑点。共享协调器向布局层暴露含栏外框，向原生层提交扣除栏高的内容框；普通拖动仍只移动活动成员。顶部不足时不再把栏放到底部覆盖内容。Windows 最大化保留 show state，以实际 DWM 边框作一次有界几何修正；解散／退出归还最大化窗口的栏空间。外部几何事件在原有 worker 中校正头部空间，不新增定时器或帧调度。

Windows 栏使用可滚动的标签视口，保留最小可读宽度；纵向滚轮和 WM_MOUSEHWHEEL 都改变水平偏移，保留高分辨率滚轮余量。绘制、点击和拖拽插入共享同一偏移；活动项变化自动进入视口，纯标题更新不重置用户滚动。裁剪复用既有栏缓冲区。

组标签分配最小空闲编号，不重排仍存在的组；全部解散后新组从 `~1` 开始，重入不会重复组合同一成员。组的标签栏不以键盘焦点决定可见性；普通后台窗口的标签栏仍可点击。

切换按“隐藏状态准备新成员 → 显示新成员 → 转移焦点 → 隐藏旧成员”执行，避免先隐藏前台窗口导致系统临时激活第三个窗口。不覆盖外部应用的 DWM transition 属性：该属性不支持读取原值，无法可靠恢复。位置通知直接更新自有栏的位置，后台消息等待同时响应原生事件与命令，不用显示帧率或固定轮询间隔控制移动。完整绘制复用栏大小的后备位图，仅移动不重绘。

窗口分组在 Windows 与 macOS 共用 `common/window_tabs.rs` 的活动成员模型，应用始终保持独立顶层窗口。每组只显示活动成员；拖动、缩放、布局、跨屏与状态切换只写活动窗口，不向隐藏成员发送跟随移动。切换编号或 Tab 时，先取得前一活动成员的最新矩形，在隐藏状态准备新成员，然后显示新成员并收起旧成员。追加以当前活动成员作为几何基准，不能使用尚未跟随移动的首成员旧坐标。

后台组合寿命独立于 Window 模式。后台屏幕上下文只随有效请求更新；取消模式的空屏幕唤醒不能覆盖它。所有原生操作在串行窗口 worker 中执行。库存保留隐藏成员的稳定身份，对外几何投影为活动成员的矩形；布局将整组折叠成一个代表，写入再路由到实际活动成员。活动成员选择与前台权限分开，系统拒绝焦点不能回滚已经完成的可见性切换。

Windows `window_tabs.rs` 拥有 WinEvent hook，`window_tabs/strip.rs` 拥有独立标签栏及缓存字体/画刷。标签栏使用活动应用作为自身 popup owner，切换前转移 owner，不设全局 TOPMOST，不修改应用父级或嵌入样式。栏的类和鼠标消息明确设置箭头；鼠标拖拽使用原生 capture、拖动阈值和插入提示，拖动标签转移单窗口，拖动组编号合并整组；无有效落点、取消或失去 capture 不提交。只有明确鼠标按下才请求应用焦点。

对原本没有 `WS_EX_LAYERED` 的应用，adapter 借用该合成标志，以 alpha 0/255 收起／显示成员，保持 `WS_VISIBLE`，减少重复显示导致的内容重建。已使用分层绘制或不支持该操作的应用保留有界 `ShowWindowAsync` 回退，不覆盖其透明度。移出、解散和退出时移除本实例添加的标志。最大化／最小化成员在透明状态提交正常位置并有界验证，再恢复不透明；普通拖动仍只移动活动应用。位置跟踪没有帧调度或固定间隔轮询；有界原生确认等待分派自身消息，避免 owned popup 的跨线程同步消息死锁。隐藏成员不做跟随几何写入，旧位置通知不改变选择；焦点事件校验真实前台，标题按事件缓存。

macOS `accessibility/window_tabs.rs` 拥有 AX observer 和 worker run-loop source，回调只排队身份；`macos/window_tabs.rs` 的 mailbox 将标签栏数据交给主线程，由 AppKit 拥有独立 NSPanel/按钮。公开 AX 使用逐窗口最小化收起非活动成员，系统可能显示最小化/恢复动画；不隐藏整个应用、不改变其他未分组窗口。窗口几何与组选择逻辑仍共用跨平台协调器。Mac 原生 UI、动画和第三方应用兼容性需要实机验证，交叉编译只能验证构建。

移出、解散或正常 shutdown 恢复被本实例收起的窗口；关闭标签栏仅发送 `Dissolve`，不会关闭应用。应用窗口关闭会清理历史身份并显示相邻成员，剩一个时解除分组；原生全屏成员退出组合。失败恢复保留必要的隐藏租约供清理重试。分组历史按原生窗口快照恢复成员、顺序和各自位置；常规窗口历史只恢复组的逻辑几何，不拆组。

备注框为原生 modeless 控件，支持系统输入法。Windows `text_prompt.rs` 通过无指针 WM_APP 消息唤醒已有托盘线程，由其创建工作区底部无标题栏的 EDIT/Save/Cancel 输入条、调用 IsDialogMessage 并在 shutdown 销毁；不新建 detached worker。macOS `status_item.rs` 的可获取键盘焦点的 borderless retained NSPanel 子类/NSTextField 由主线程状态目标持有，Save/Cancel 返回带 id 的 BackendEvent，关闭状态栏时一并清理。macOS 窗口枚举在按应用收集 AX 后恢复 Quartz 全局前后顺序，供模板填充使用；顺序是当前系统堆叠顺序，不宣称拥有未观察到的历史焦点记录。

编辑事务在 worker 内捕获窗口原始快照和最小尺寸。批量标准化布局先验证、再写入变化矩形，直接复用已验证的原生结果；严格树布局发生部分失败时恢复前批次。正常返回／退出保留即时布局，把原始快照合为一步撤销；强制清理只释放检查点。内部错误恢复仍可有界回滚。编辑约束在进入编辑与原生拒绝尺寸后查询；普通连续缩放按目标、手势、屏幕与缩放比例复用最小尺寸。

Windows 复用枚举缓冲区，property 标记直接查找稳定身份，几何等待只轮询句柄和矩形；应用名按身份缓存。macOS 按批次复用 Quartz 查询键并缓存应用名。明确销毁的窗口通过 `WindowResult.closed` 回收身份、AX 引用、约束和撤销记录；不可见、最小化或临时 AX 超时不能当作销毁。被取消查询的关闭通知留给下一次结果。Windows adapter 的 Drop 也清理自有 property。错误统一调用 `report_error!` 进入 `support/logging.rs`。

Window 首次使用时启动 `common/window_session::WindowWorker`，原生 adapter 在线程内部创建和释放；backend 仅提交 API 请求并转发事件。队列上限 64，同一手势的相对位移/尺寸增量合并。查询、批量写入和全屏过渡均检查会话及请求取消水位，不能在 Engine 中等待。取消保留会话历史的操作与结束整个会话分开；shutdown 使用现有有界 WorkerJoin。

Windows adapter 持有 HWND、PID/TID 和独有窗口 property 标记；窗口销毁后即使 HWND 被同进程重用，也不能通过旧 WindowId 操作它。重用已有普通窗口过滤（含 cloaked/minimized/self/tool 排除），按需枚举最多 256 个候选，包含普通被遮挡窗口。placement 的读取/异步提交与旧 `window_mover.rs` 共用；读写区分 DWM 可见矩形、原生 resize border 和 workspace 还原坐标。尺寸下限查询使用 50ms SendMessageTimeout，异步移动只在 worker 中进行有界回读。逻辑步长、速度和布局间距按目标 Screen.scale 换算。

macOS adapter 保留 AX 元素，以公开 Quartz on-screen 元数据匹配当前 Space 的普通 AXStandardWindow；无法唯一匹配的重叠候选跳过，所有 AX messaging 有有限超时。AXSize 写入后读取实际尺寸再定位中心；普通最大化以工作区尺寸实现并保留还原矩形。原生全屏跨屏复用既有 WindowMove 状态机，在 worker 中轮询并检查取消；其等待不占用主事件循环。

Window 的 Tab / Shift+Tab 按需枚举，沿同一稳定 ID 顺序向前／向后激活并返回中心鼠标位置，库存包含普通窗口及每个隐藏的分组成员。数字保持同样的逐窗口身份。成员显示成功但系统拒绝焦点时仍提交活动成员状态；旧焦点或成员几何通知不能覆盖明确选窗。显式 window_tile 只处理目标所在屏幕，跳过不可缩放/原生全屏窗口，整个批次不逐个 warp。单窗及撤销使用原生快照回读，窗口拒绝或关闭时保留实际改变/跳过计数。共享几何不调用平台 API。

Windows 显式 Tab 先调用 SetForegroundWindow，被拒绝后使用 SwitchToThisWindow 的键盘切换路径，并在 worker 中有界回读前台。不得用 AttachThreadInput 连接外部输入队列造成无界同步等待。最终拒绝焦点时仍锁定下一窗口、返回其中心坐标并提示。

平铺优先保留满足所有最小尺寸的均分方案，否则重新分配行列空间；没有无重叠方案时按最小尺寸提交并约束位置，不能因格子太小跳过应用。尺寸下限查询之间检查取消。原生回读决定实际变化及撤销记录。

## 共同契约

跨屏窗口移动复用 `common/window_placement.rs`：以窗口与屏幕的最大交集选择源屏，按显示器
编号循环 previous/next。相同屏幕尺寸精确平移（不受两屏任务栏差异影响），不同尺寸按工作区剩余可移动空间的比例映射，
保留窗口大小；超出目标工作区的大窗口至少把左上角放入工作区。
动作开始时只读取一次物理鼠标位置，用于命中和跟随计算。`following_pointer` 保持鼠标在
普通窗口内的偏移；最大化窗口尺寸变化时保留比例，并将结果限制在目标显示器内。
后端提交成功后返回该位置，由 Engine 统一 warp 和同步状态；无目标或请求失败不移动鼠标。

Windows `window_mover.rs` 复用 UIA 的纯 HWND 命中/过滤，不提交扫描。桌面、任务栏、自身及
透明覆盖层不作为目标，也不穿透系统桌面/任务栏移动背后的应用。普通窗口使用异步
`SetWindowPos` 并保留焦点/Z-order；最大化窗口同样先通过 `SetWindowPos` 移动实际边框，
按目标工作区和 DPI 保留不可见边框，再用异步 `SetWindowPlacement` 更新还原位置。
整个过程保留最大化状态，不使用还原再最大化的中间状态；只更新还原位置不能保证可见窗口跨屏。
还原位置显式转换 workspace/screen 坐标。成功表示操作已提交，应用仍可能
限制大小、DPI 行为或拒绝移动；禁止同步等待外部 UI 线程。

macOS `accessibility::window_under_pointer` 通过 AX 命中鼠标下元素及 `AXWindow`，普通窗口直接设置
`AXPosition`。CF 引用由 `OwnedCf` 释放，系统与保留窗口均使用有限 messaging timeout。
`window_move.rs` 保留同一个 AX 窗口，在后端 poll 中自动退出原生全屏、按还原后的窗口尺寸跨屏、
恢复全屏。实际位置映射与 AXPosition 写入复用普通窗口的 `move_window_frame`；全屏流程仅包裹前后的状态切换及观察。
使用系统过渡动画，系统可能随全屏切换焦点。每 50ms 至多推进一次，全屏退出/进入要求
AX 状态与位置持续稳定 750ms，普通位移阶段要求 150ms；变化或读取失败重置稳定计时。
这是防止过早跨屏的保守时序保护，并非系统动画完成通知，异常缓慢的动画仍需实机验证。
不 sleep 等动画，不发送模拟快捷键，也不创建后台 AX 线程。确认目标屏幕全屏后
通过 `WindowMoveCompleted` 让 Engine 同步鼠标。每阶段 5 秒截止；显示器变化、失败或 shutdown
尝试恢复全屏，记录恢复失败并释放引用，不保证系统接受恢复。进行中重复请求不重复启动。
全屏过渡中的 AX 写入返回 `kAXErrorCannotComplete (-25204)` 时，只表示回执未确认，应用可能
已经执行：保留事务进入对应观察阶段，不立即回滚，也不重复发送退出、位移或恢复请求。以窗口
实际状态确认后继续；超过阶段截止才报告失败并尝试恢复。其他明确拒绝仍按失败处理。

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

Windows Hook 用固定 bitset 记录真正返回给系统的按键消费决定。重复和松键沿用首次按下
的结果，包括队列满或 100ms 握手超时；首次按下没有决定时仍透传。常规 Hook 更新保留
这些状态，会话恢复时清空。两端共享的 disposition mailbox 在超时的同一把锁内作废
generation，迟到确认向 Engine 返回可恢复错误，不再继续执行已经失去消费确认的动作。

Windows 低级 Hook 可能被系统静默移除，而所属线程仍存活。Hook 线程持有 30 秒的原生消息
timer，在回调之间安装新句柄并释放旧句柄；安装失败保留原句柄，下次重试。常规更新保留
按键状态和活动模式。disposition 超时也投递更新请求，不在回调内安装 Hook。
托盘窗口接收系统恢复、解锁和会话重新连接通知，投递更新请求并通过 `InputCaptureLost`
使 Hook 与 Engine 清理失效的输入状态。后台事件通道断开时先 join 原线程，再重建；失败
按 30 秒退避重试，不能把断开当成普通空队列。timer 和会话通知均由原生 RAII owner 清理。

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

## 可打印字符输入

物理按键映射保留不变。Engine 在初始路由编译、配置重载和有效应用覆盖配置变化后，通过
`Backend::set_character_bindings` 交付单字符绑定需求。两端复用原生物理键映射，只有物理键
路径无法表示的字符才启用额外观察；默认配置、无字符需求的配置和移除最后一个字符绑定后，
非 repeat Down 只检查关闭标志，不查询布局、不读取 Unicode、不分配字符缓冲区。

两端共用 `common/character_candidates.rs::CharacterDemand`：编译实际需要额外观察的字符、
管理开关，并用 128-bit ASCII 位图在 UTF-16 解码之前排除未绑定的普通字符。单个 ASCII
字符只读取一个原子字，命中时直接返回已校验的 ASCII 字符；不需要遍历绑定或解码迭代器。
非 ASCII、代理对和多字符文本仍走原有
校验，不把 Unicode 大小写关系简化为 ASCII。Windows 的原生候选快照也放在该模块，
但布局枚举和原生调用仍在平台目录中。

Windows 在后端线程按当前布局预编译 `virtual key × Shift/Ctrl/Alt` 候选位图，正向枚举锁定
状态和小键盘等多种产生方式，不把 `VkKeyScanEx` 的单个反查结果当成完整候选集合。Hook
启用字符观察后仍须读取真实前台布局：`WH_KEYBOARD_LL` 不携带 HKL，不能依赖异步通知缓存
而漏掉布局切换后的第一键。布局一致、scan code 一致且位图排除的键直接返回；只有候选、
未知扫描码或失效缓存才构造按键状态并调用 `ToUnicodeEx`。按键状态按已按下 bitset 的置位项
填充，不再遍历全部 256 个键。遇到新布局先保守观察，并请求后端在没有待交付事件时重建；
重建不在 Hook 回调中执行。发布以版本号保护，读到更新中的位图不得据此拒绝字符。
dead-key 布局或无法枚举的字符保留完整观察回退；flag 4 保持系统 dead-key 状态不变。

macOS 的 CGEvent 已携带系统转换好的 Unicode 文本：无需求时完全跳过读取；有需求时
读取固定栈缓冲区，马上通过共享 ASCII 位图过滤，再对候选执行 UTF-16 校验。它仍需
读取事件文本来识别实际字符，**没有 Windows 那种物理键/布局候选表**。不在 Hook 中
调用 TIS 输入源 API，也不增加主线程同步、Carbon 布局枚举或布局通知缓存，避免引入
主线程约束和布局切换时的漏键风险。两端只在非 repeat、非修饰键 Down
观察字符，用固定栈缓冲区拒绝控制字符、无效 UTF-16 和多个字符，不把 IME 文本提交当成
快捷键，也不维护某个符号的 Shift 映射表。`hook_received` 性能标记覆盖字符读取的耗时。

## 鼠标侧键绑定

`mouse_x1` / `mouse_x2` 是物理触发键（别名 `xbutton1`/`mouse4`、`xbutton2`/`mouse5`）。
同名右值动作通过 `Binding::Click` 映射到 `MouseButton::X1/X2`；Windows 发送带
`XBUTTON1/2` 的原生 XDOWN/XUP，macOS 发送按钮编号 3/4 的 OtherMouseDown/Up。
成功合成点击沿用一次 `Clicked` 通知；显式 press/release/toggle 不产生该通知。
Windows Hook 将 XBUTTON1/2 的 down/up、macOS Hook 将 OtherMouse 的按钮 3/4 边沿转换成
普通 `BackendEvent::Input`，character 为空，复用现有绑定、继承、held gesture 和 disposition
握手。平台缓存 canonical Key；忽略自身注入，原生侧保存实际消费决定，超时/跨模式松开
也不能拆散 down/up。Windows 使用 VK_XBUTTON1/2 的独立 bitset 槽，并复用 Alt 菜单抑制。
未匹配绑定的侧键必须透传，不能受 `captures_keyboard` 影响，也不发送原始 `ModeEvent::Key`；
物理侧键不产生语义 `Clicked`。左右键、中键和滚轮不接入此次绑定链路。

Windows 窗口布局的异步恢复等待同时核对最大化样式与目标几何，不能把旧最大化帧短暂稳定误认为恢复完成。严格布局拒绝时，未变化或仍最大化的几何不能提升缓存的最小尺寸。保留原生 checkpoint 的 show state，撤销可恢复最大化。

`size_cycle`依据窗口状态循环最大化→最小化→恢复普通大小。Windows 保留可见原始矩形并核对 IsIconic；macOS 保留恢复矩形并写入/回读 AXMinimized。最小化不释放身份或目标，不移动指针、不绘制目标边框/编号；checkpoint 包含 minimized 状态供撤销使用。退出会话仍遵守原有身份释放规则。

窗口 worker 在首次 Acquire/库存发现/编辑入口保存原生初始快照，后续相同库存不重复读取。撤销历史最多 32 组；初始快照独立保存到会话结束，重置仅触及本会话实际改过的窗口。Undo/Redo 回读恢复结果，为实际发生的变化保存逆快照；取消保留未处理项，拒绝恢复可重试，明确关闭同时清理初始记录及双向历史。ResetInitial 本身保存一组逆快照，不把新建/关闭窗口或未修改应用当作可还原的窗口内容。

Window 入口复用 worker 的 activate_window（Windows 前台激活、macOS 应用激活与 AXRaise），在呈现目标边框之前完成。Close 经 Grouped 转发到原生目标：Windows 异步 PostMessageW(WM_CLOSE)，macOS 对 AXCloseButton 执行 AXPress；不终止进程，不绕过应用的保存/取消流程。

Grouped::close 将组内任意代表解析为活动成员，只向它发正常关闭请求；确认关闭后由既有 close_member/forget 清理成员，剩一个自动解散、撤下标签栏并显示剩余窗口。请求阶段不修改成员表，取消保存不会丢失分组。
