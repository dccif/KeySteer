# UI Hint 扫描链路

## 性能与取消约束（2026-08）

- Windows UIA 以原子 scan id/stopping flag 做逐节点取消检查；前台 HWND/PID 每 32 个节点采样一次，并在发布 partial 前强制复核。
- macOS AX/Vision/Hybrid 共用纯数量批次：第一批 24 个目标立即发布，随后在累计 48、96、192… 个目标时发布，terminal 立即补发剩余目标；不使用时间间隔，也不依赖显示刷新率。Hint 复用标签 String 和结果 Vec。
- macOS ScreenCaptureKit capture 是 native single-flight。timeout 不释放 permit；permit 只由真实 completion callback 释放，晚到图片恰好释放一次。
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
不会污染新的页面。

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
- `IUIAutomation2` 可用时设置 connection/transaction timeout；不可用时退回基础 UIA。
- `FindAllBuildCache(TreeScope_Descendants, condition, cache)` 一次让 provider 填充需要的
  属性，避免逐节点跨进程读取。
- `clickable_only` 尽量编译成 provider-side condition；失败时才扫描全部 descendants 并
  在客户端过滤。
- 每批最多 24 个目标，立即发 `Partial`；目标数量、深度、时间和窗口数都有边界。

### popup/dropdown/dialog

扫描不只使用主窗口 descendants：

1. 从 foreground HWND 获取所属 UI thread。
2. `EnumThreadWindows` 收集同线程可见 `WS_POPUP`，要求 owner 关系或与前台窗口共享显示器。
3. 排除不可见、最小化、DWM cloaked 窗口，并限制候选数量。
4. `EnumWindows` 按 Z-order 建立每个候选上方的可点击顶层窗口矩形。
5. 目标中心若落入 occluder 则过滤；KeySteer 自身 click-through overlay 不作为遮挡。

这覆盖独立 HWND 的菜单、下拉框、日期选择器和 dialog，同时避免给被别的窗口盖住的
控件打标签。

### Windows strategy

当前 Windows 不运行 Vision；配置为 `vision`/`hybrid` 时记录一次 warning 并使用 UIA。
不要在公共配置层删除这些值，否则同一配置文件不能跨平台共享。

## macOS AX/Vision

调度：`src/platform/macos/ui_scan.rs`。

- 一个持久 worker + 单 pending slot，避免多个全分辨率截图并行扩大内存。
- 新任务替换等待中的旧任务，并向旧请求发送 `ContextChanged`。
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
