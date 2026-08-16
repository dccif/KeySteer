# UI Hint 扫描链路

## 性能与取消约束（2026-08）

- Windows UIA 以内部 generation/stopping flag 做逐节点取消检查；前台 HWND/PID 每 32 个节点采样一次，并在发布 partial 前强制复核。
- 两个平台使用同一个纯计数 `PartialBatcher`：第一批 24 项，随后按累计数量
  48、96、192……发布，terminal 立即补齐；没有 16ms 条件或刷新率依赖。结果进入带内部
  generation 的 latest-only mailbox。Engine 忙时合并当前扫描的
  Partial；新请求或取消直接丢弃旧槽，旧结果不会排在下一次 UI Hint 的首批标签前。
- macOS ScreenCaptureKit capture 是 native single-flight。timeout 不释放 permit；permit 只由真实 completion callback 释放，晚到图片恰好释放一次。
- macOS 扫描 worker 由 Backend 懒创建并拥有，不再是进程静态 detached worker。退出时先使
  generation 失效、丢弃 pending，再唤醒并有界等待 worker；已经提交且系统不支持取消的
  ScreenCaptureKit completion 由进程退出兜底，不能阻塞 Quit。
- Vision timeout 限制为 1..=30000ms，rectangle candidates 限制为 1..=2000，Rust 与 Objective-C 两侧均校验。

## 公共请求模型

`UiScanRequest` 包含 scan id、软超时、屏幕 bounds、语义 roles、树深度、可见/可点击
过滤、扫描 strategy、Vision 参数和请求时的前台应用。Backend 异步返回
`BackendEvent::UiScanned(UiScanResult)`：

- `Partial`：一批可立即显示的目标。
- `Success`：正常结束。
- `TimedOut`：软预算耗尽，已返回的 Partial 仍有效。
- `ContextChanged`：前台应用或新 scan 已替代本次请求。
- `PermissionDenied` / `Unsupported` / `Failed`：可显示状态或重试。

Engine 用 scan id 记录 owner；HintMode 也只接受当前 `scan_id`。旧 worker 即使晚返回也
不会污染新的页面。Engine 把当前 `UiScanResult` 按所有权交给 HintMode：常见首批可直接
接管平台 Vec，后续目标逐项 move，名称、role 和 native role 不再跨层 clone。

退出或完成 owner 时 Engine 显式取消 scan。Windows 会取消进行中的 WinRT OCR、终止微信
helper、让纯 Rust fallback 在行/连通区域边界退出，并 join 本次 provider 线程；generation-scoped
vision coordinator 在当前与 latest pending 请求完成后直接退出，不留常驻视觉线程。Hint 退出会 drop 目标、标签和字符串，下一次进入
重新扫描。仅复用最多 128 项的空容器
backing，以降低常见不足 100 个标签时的重入分配；大型扫描容量不会留在 Idle。

Windows 每个 generation 的 GDI top-down DIB 只截图一次。视觉线程不会再复制或让 provider
持有一份完整 BGRA `Vec`。DIB 字节稳定且前台 HWND/PID/generation 复核通过后立即释放 overlay
capture gate；系统 OCR 从同一个 DIB 构造重叠小块，微信 OCR 构造一份完整 `SoftwareBitmap`，
fallback 直接下采样灰度图。源窗口与目标 DIB 尺寸相同时使用 `BitBlt` 直接复制；8,388,608
像素的上限完整覆盖 3840x2160，只有 5K/8K 或更大的跨屏交集才使用
`StretchBlt + HALFTONE`。截图始终是一张完整图片，绝不对桌面重复截图。

系统 OCR、微信 OCR 都在 provider 内最多保留 2000 个有效目标。任一 provider 返回非空有效
结果便取消并丢弃 fallback；这个判定发生在 UIA/空间索引去重之前，因此 Hybrid 中 OCR 与
UIA 重叠也不会误跑纯像素检测。系统 OCR 使用 WinRT completion handler 唤醒，不做固定 5ms
状态轮询。provider 取消后按绝对 deadline join；违反取消契约的线程转入有 owner 的 quarantine，
本进程后续禁用视觉扫描，不能阻塞 Engine 或静默 detach。

## HintMode 端

`src/modes/hint.rs` 的职责：

1. 启动 scan 时清空上一轮目标、标签、搜索和完成态。
2. 每个 Partial 立即 append，按矩形/名称/role 空间 key 去重。
3. 使用 `domain/hints::assign` 给当前候选重新分配标签并 redraw。
4. 搜索模式只过滤已扫描目标，不重新遍历平台树。
5. 完整标签选中后保存 target、warp pointer、建立 finished 状态。
6. 只有没有出现标签时才按 `scan_retry_count`/delay 重试；每次预算递增，单次最多 30s。
7. overlap cycle 修饰键按下时立即置顶下一项；按住期间固定当前标签集合，后续 Partial
   只累计目标，释放后再一次性合入，避免增长中的堆叠组自动换项。

因此“流式”是端到端语义，不只是 worker 分批：Mode 会在扫描尚未结束时展示和接受已有
结果。不要为了完整排序而等终态。

## Windows UI Automation

实现：`src/platform/windows/accessibility.rs`。

- 一个 backend-owned MTA worker 长期持有 COM/UIA；新任务替换 pending 旧任务。
- 退出 owner 时使 generation 失效、清 pending，并以 `CoCancelCall(thread, 0)` 非阻塞请求
  取消 provider call；COM apartment 与 Automation 对象继续保持温热。
- `IUIAutomation2` 可用时设置 connection/transaction timeout；不可用时退回基础 UIA。
- worker 初始化时构建一次并复用 `CacheRequest`、true condition 和 clickable condition；每轮
  仍重新查询 ElementArray，不缓存旧控件或窗口。
- `FindAllBuildCache(TreeScope_Descendants, condition, cache)` 一次让 provider 填充需要的
  属性，避免逐节点跨进程读取。
- `clickable_only` 尽量编译成 provider-side condition；失败时才扫描全部 descendants 并
  在客户端过滤。
- 首批 24 个目标立即发 `Partial`，后续使用公共数量翻倍边界；目标数量、深度、时间和窗口数都有边界。

### popup/dropdown/dialog

扫描不只使用主窗口 descendants：

1. 从 foreground HWND 获取所属 UI thread。
2. `EnumThreadWindows` 收集同线程可见 `WS_POPUP`，要求 owner 关系或与前台窗口共享显示器。
3. 排除不可见、最小化、DWM cloaked 窗口，并限制候选数量。
4. `EnumWindows` 按 Z-order 建立每个候选上方的可点击顶层窗口矩形。
5. 目标中心若落入 occluder 则过滤；KeySteer 自身 click-through overlay 不作为遮挡。

这覆盖独立 HWND 的菜单、下拉框、日期选择器和 dialog，同时避免给被别的窗口盖住的
控件打标签。

### Windows strategy 与视觉管线

- 发布默认是 `hybrid`：UIA 与完整视觉管线由独立 worker 同时调度，共用下述发布器流式去重并合并空间并集；`axtree` 或 `vision` 可分别只调度其中一条管线。
- `src/platform/windows/ui_scan.rs` 是唯一发布点：所有来源共用 64px 覆盖网格、24/48/96…累计边界和 2000 项上限。矩形只保存一次，覆盖超过 32 格的大矩形进入有界 oversize 列表；先显示的目标不被后到重复项替换。
- Backend ready 后的首次事件轮询只异步执行一次 OCR discovery：系统探测在临时 MTA 中创建并立即销毁 `OcrEngine`，微信探测只缓存已验证的绝对路径与文件标识。系统 OCR/WIC 通过本代拥有的 activation factory 调用，禁止使用会跨临时 MTA 残留悬空指针的投影静态 factory cache。这样 ready 热路径不承担探测线程创建；成功和失败结果都缓存到进程退出，不保留 OCR、helper、COM 或 vision worker。首次扫描若探测尚未完成，只由视觉协调线程等待同一个 single-flight 结果。
- `src/platform/windows/vision.rs` 在有视觉请求时懒启动 generation-scoped coordinator。每个 generation 只取得一张 GDI top-down DIB 截图，并在提交和发布边界复核 HWND、PID 与 generation；当前与 latest pending 请求完成后 coordinator 自行退出。
- 截图前由 `overlay_worker.rs` 建立 generation capture gate：渲染线程清空 GPU/CPU overlay。若 overlay 从未显示，或上次隐藏已经由 DWM 确认，则直接 ACK；仅在存在尚未确认消失的可见像素时执行一次 `DwmFlush`。ACK 前 UIA/OCR 提交只覆盖 latest deferred frame；截图复制完成立即释放 gate，只显示最新帧。旧 lease、取消和 shutdown 不能释放新 generation。
- `detect_text=true` 时先从进程内 discovery 快照生成 `None`、`SystemOnly`、`WechatOnly` 或 `Dual` 执行计划。`SystemOnly` 只创建系统 tile，绝不进入微信专用完整位图、WIC、PNG、helper、job、pipe 或 reader 路径；只有 `WechatOnly`/`Dual` 才构造 `WechatFullFrame`。系统 OCR 按可用逻辑线程的下一个平方数选择 `N×N` 网格，并由图片尺寸限制核心块的宽、高都至少为 64px；没有固定 `7×7` 上限。各块向相邻核心区重叠 64px，bitmap 就绪后立即提交独立 `RecognizeAsync`；provider 在继续接收 tile 的同时消费 completion，完成一块就按最多 24 项发布。块内坐标先映射回桌面，目标中心只归属于一个半开核心矩形，因此接缝文字不会漏掉或重复发布。`Dual` 在首个系统 tile 后才提交微信专用完整帧，随后继续排空两路结果；微信结果先发布、再清理 helper 与 PNG。terminal 严格等待全部块和 helper 清理完成，两路最终发布空间并集。
- `detect_rectangles=true` 时并行计算灰度、局部对比/梯度、形态闭合和八邻域连通区域。分析图限制为 2,073,600 像素、最长边 2560，候选最小边为 6px；scratch 只属于本次 generation，连通区域使用逐行 run-length 和活跃组件，不保留最坏覆盖全图的像素队列。形态位图复用且候选维持有界 top-K。任一 OCR 产生有效目标就立即取消这项 CPU-heavy 工作；只有全部 OCR 没有有效目标时才发布缓存。
- `src/platform/windows/wechat_ocr.rs` 自动查找可执行文件旁的 `wcocr.dll`，以及 `%APPDATA%\Tencent\xwechat\XPlugin\plugins\WeChatOcr\<版本>\extracted\wxocr.dll` 中版本最高的微信 4 OCR 组件；微信 4 运行目录按 `Weixin.dll`、`WeixinExt.exe` 或旧布局的 `Weixin.exe` 验证，不要求版本子目录内必须存在主程序。微信 3 继续作为后备。所有 PE 架构都会验证，并且组件只在隐藏 helper 子进程中加载。IPC 与响应有界，超时/崩溃/owner 取消会终止 helper，临时 WIC PNG 总会清理；组件不进入发行包。
- 每次扫描只创建执行计划真正需要的 `OcrEngine`/helper；冷启动和 overlay 隐藏重叠。系统 OCR 各块拥有独立的 `SoftwareBitmap` 与 operation，同一 generation 复用 factory，DIB 区域直接逐行写入 bitmap，不经过 tile BGRA `Vec`。取消先对全部 operation 调用 `Cancel`，再复用 scan/shutdown 的绝对 deadline 和现有 completion channel 收敛终态，不创建 timer、sleep 或轮询线程；终态后立即 `Close`，违约 operation 连同 worker 进入显式 quarantine。微信 WIC PNG 使用完整 `SoftwareBitmap` 直接编码到临时文件，不生成完整内存 PNG，并在编码后立即关闭完整 bitmap。terminal 发布前必须删除 PNG、关闭 IPC、终止并等待 helper、join reader/provider、关闭所有块、释放截图与 GDI surface 并 drop 本次 COM apartment。源码禁止 `SetWindowDisplayAffinity`/`WDA_EXCLUDEFROMCAPTURE`，避免自捕获只依赖上述隐藏确认屏障。

这里不增加公共 OCR provider/path 配置；运行时状态只进入日志和 `--doctor`。

## macOS AX/Vision

调度：`src/platform/macos/ui_scan.rs`。

- 一个持久 worker + 单 pending slot，避免多个全分辨率截图并行扩大内存。
- 新任务替换等待中的旧任务；旧 generation 的未消费结果直接丢弃，不排入公共事件队列。
- 退出 owner 时清除 pending、失效 native generation；已经提交且无法取消的
  ScreenCaptureKit capture 仍由真实 completion 恰好释放一次。
- 每次发送前检查 `LATEST_SCAN` 和 frontmost pid；新 id 也会让正在遍历的 AX 在节点边界
  停止，并让 Vision 在 capture/识别阶段边界释放过期结果。
- AX 和 Vision 共用纯计数的流式发布器；Hybrid 在 scoped thread 中并行 AX，同时当前 worker
  执行 Vision。合计首批 24 项立即发布，之后在累计数量达到 48、96、192……时发布，终态
  无条件刷新剩余项。这里不使用时间间隔，因此不依赖显示刷新率或机器速度。

AX：`src/platform/macos/accessibility.rs`。

- 从 frontmost pid 创建 `AXUIElement` application。
- 设置每节点 messaging timeout，受总 deadline 和 max depth 限制。
- 读取 role、frame、enabled、actions 和 accessible name，语义 role 映射在本文件。
- 一次扫描复用固定的 AX 属性名 `CFString`；矩形去重使用预分配 `HashSet`。
- 遍历过程把拥有所有权的 24 项 batch 直接交给共享发布器，不保留完整目标数组或深拷贝
  已发送的 Partial。

Vision：`src/platform/macos/vision.rs` + `vision_bridge.m`。

- 获取 focused window bounds，与请求屏幕 bounds 取交集。
- ScreenCaptureKit 截图后使用 Vision text/rectangle requests。
- Retina 原生分辨率保持不变；Objective-C bridge 直接填充最多 2000 项的 C region 数组，
  不为每个 observation 创建 `NSDictionary`/`NSNumber` 中间对象。
- Rust 侧按 confidence、尺寸、宽高比和 IoU 分类/合并候选。
- 原生请求超时后可能晚结束，但单 worker 保证不会叠加第二次全分辨率 capture。

## 修改扫描时的约束

- 不阻塞 Engine/UI thread。
- 每个昂贵 native provider 都必须有 timeout、取消/过期检查和结果上限。
- Partial 先于终态是正常顺序；终态可以不携带 targets。
- retry 只属于 HintMode 生命周期，Backend 不应自行无限重试。
- 前台应用/显示器变化必须使旧结果失效。
- 过滤应尽可能下推 provider，但必须保留失败后的安全 fallback。
