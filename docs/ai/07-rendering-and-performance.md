# 覆盖层、帧同步与性能约束

## 提交模型

`Backend::present` 接收 `Arc<OverlayScene>`。Windows 使用 latest-frame 单槽队列：Engine 只替换待绘制帧并立即返回，已经过期的帧不会进入原生绘制。同一输入批次中的 Warp、Show、Finish、Click、Hide 会先合并覆盖层意图；输入注入保持立即执行，批次结束只提交一次最终画面。

Engine 保存最后一次 scene 并跳过完全相同的提交。`OverlayScene` 的静态 shapes/labels 使用
`OverlayItems<T>` 写时复制存储：移动 cursor/indicator 时 clone 只增加 `Arc` 引用计数，
不会复制数百个标签；共享存储的相等判断也有指针快速路径。场景只在进入 Engine 时排序
一次，逐帧刷新不得再次排序并触发写时复制。序列化仍保持普通数组格式。Grid 使用
`Rect::subdivision` 直接计算单格，Recursive Grid 的层级布局在配置应用阶段编译，选择
热路径不构造完整 `Vec<Cell>`。

Normal 的活动方向使用四位 mask，而不是每个显示帧收集一个 `BTreeSet`。移动距离仍由
真实 elapsed 的解析积分决定；这个优化只消除逐帧堆分配，不改变对向抵消、对角线归一化
或多物理键绑定到同一方向时的去重语义。

Mode 热路径返回 `CommandBatch`：0/1/2 个命令不分配，第三个命令才 spill 到 `Vec`。
64 位布局测试限制 `Command <= 64 B`、`CommandBatch <= 128 B`、`ModeEvent <= 112 B`；
`tests/performance.rs` 用 instrumented allocator 锁定预热后的 Normal frame 为零分配，
`cargo bench --bench core_hot_paths` 报告其 p50/p95/p99。temporary-mode chord 与 keymap 一起
预编译，物理修饰键查通用别名时使用借用查表，不在按键路径构造临时 `Key`。

cursor marker 由 Engine 在 mode scene 之上装饰；合成鼠标按钮进入 latched 状态，或普通
click 的物理触发键仍按住时，仅替换 marker 的填充/轮廓颜色并刷新动态 overlay，不改变
mode scene，也不重建静态 Grid/Hint 内容。latched 的真实按钮状态优先；普通 click 使用
最近仍按住的触发键，并由该键的释放事件清除。颜色提示本身不使用 timer 或 polling；
可配置的长按 Toggle 复用 Engine 的下一 deadline 超时，不创建周期轮询或额外线程。

## Windows GPU 主路径

`src/platform/windows/gpu_overlay.rs` 是默认渲染器：

- 渲染线程独占透明 click-through HWND、D3D11 BGRA device、DXGI device、Direct2D context、DirectWrite factory 和 DirectComposition device。
- Direct2D 直接绘制到 DirectComposition surface；GPU 路径没有全屏 CPU RGBA/DIB，也没有上传 memcpy。
- Rect、Line、文字、cursor 与 indicator 在 GPU surface 中完成；颜色 brush、字体 format 和 UTF-16 scratch buffer 有界复用。
- GPU HWND 使用 `WS_EX_LAYERED | WS_EX_TRANSPARENT | NOACTIVATE | TOOLWINDOW | TOPMOST`；layered + transparent 才保证跨进程点击穿透，`HTTRANSPARENT` 仅作为额外防线，因为它只能继续命中同线程窗口。
- 渲染线程使用 Win32 消息队列作为 latest-frame/control 唤醒源；即使 normal 覆盖层静止也会
  持续响应窗口消息，并对 `WM_NCHITTEST` 返回 `HTTRANSPARENT`。禁止让全屏 HWND 在
  Condvar/普通 channel 上无限等待，否则 Windows 会将其判为挂起并阻塞底层窗口输入。
- Hide 销毁 HWND/visual/surface，只保留轻量 device、brush 和字体描述缓存。
- Windows 高 DPI 标签缩放由渲染线程按“源标签共享存储 + scale”缓存。cursor/indicator
  更新复用已缩放静态标签；源场景变化、缩放变化时才重建。进入空场景、回到 100%
  缩放、Hide 和 Shutdown 都会清空该缓存，不能把上一张大型 Grid 留在 Normal 中。

恢复规则位于 `overlay_worker.rs`：

1. GPU 初始化失败时直接启用持久 DIB renderer。
2. GPU present/commit 失败时重建一次完整 GPU device tree，并重画最新帧。
3. 60 秒内再次失败则本次会话固定使用 DIB，避免设备抖动反复重建。

`src/platform/windows/overlay.rs` 是稳定软件回退。它复用单个 top-down premultiplied DIB、文字 mask、UTF-16 buffer 和有界字体缓存；只在 GPU 不可用或重复丢失时运行。

## macOS typed Core Animation

- 覆盖层 `NSPanel` 在创建和每次复用时都重新断言 `ignoresMouseEvents`，点击穿透由
  WindowServer 属性保证，不依赖主线程及时处理 hit-test；图层更新通过禁用隐式动画的
  `CATransaction` 提交，不调用 `displayIfNeeded` 强制同步绘制。
- `CGEventTap` callback 内禁止阻塞 channel send：输入队列满时按键立即 fail-open，
  指针合并槽清除发布标记以便下一事件重试，避免渲染压力反向卡住系统输入 tap。

`src/platform/macos/overlay.rs` 不再使用裸 `Id`、手写 `objc_msgSend` 或手工 retain/release：

- `NSPanel`、`NSView`、`CALayer`、`CAShapeLayer`、`CATextLayer` 全部由 typed `Retained<T>` 管理。
- Core Graphics 颜色和路径使用拥有型 Core Foundation wrapper。
- 静态 shapes/labels、cursor 和 indicator 分层；光标移动只更新动态层。
- CALayer/CATextLayer 按当前 scene 所需槽位复用，文字内容不变时不重新创建 `NSString`；
  scene 缩小时，多余 shape 会从 root layer 移除，多余 label 会先断开 mask、子层与父层再释放。
  因此 Grid 首屏的二级预览层不会在返回 Normal 后继续成为常驻高水位缓存。
- 所有属性更新放在禁用隐式动画的 `CATransaction` 中，避免输入后出现动画拖尾。
- 每次 present/dismiss 都有独立 autorelease pool，AppKit/QuartzCore 的临时对象不会依赖
  下一次手动 run-loop pump 才释放。
- Hide 先移除 shape、label、cursor，清除 root view 的 layer 和 window 的 content view，
  再关闭 `NSPanel` 并释放 typed owner；隐藏期间不保留完整 layer tree 或 compositor backing。

窗口仍在 AppKit 主线程创建和更新。Engine、输入 Hook 与扫描工作不通过原生对象共享状态，只通过有界队列/安全 mailbox 通信。

UI Hint 在活动扫描和重试期间复用 `scanned`/`hints` 容量，并为目标名缓存一次 lowercase
结果；搜索重标记不得逐目标重新分配小写字符串。目标去重使用 `HashSet`，标签重叠分组使用
按左边界排序的扫描线与并查集，常见稀疏布局不再全量执行 n² 比较。离开模式时若容量超过
小型扫描上限则直接释放 backing allocation，因此 2000 目标级扫描不会成为 Idle 常驻内存。

## 帧时钟

- Windows 11 优先使用 `DCompositionWaitForCompositorClock` 的 display-independent
  compositor heartbeat。它原生覆盖不同刷新率和多适配器屏幕，并把 stop event 纳入同一次
  无限期等待，因此停止移动不必等下一次 VBlank，也不需要跨屏重建时钟。持续移动期间用
  `DCompositionBoostCompositorClock` 请求动态刷新高频模式，停止和所有错误出口都会撤销。
- Windows 10 或新 API 不可用时，用 `MonitorFromPoint` 将光标位置映射到原生输出，再通过
  `IDXGIOutput::WaitForVBlank` 等待该输出的下一次 VBlank；跨屏移动会重新选择输出，
  不查询或缓存刷新率。远程、无头或 DXGI output 不可用时最终回退 `DwmFlush`。
- Windows 11 函数通过小型 C bridge 从系统 `dcomp.dll` 动态解析，旧系统的导入表不包含
  新符号，因而保持可启动。bridge 只负责可中断等待和 boost；elapsed、队列与生命周期仍
  由 Rust 所有权管理。
- Windows 的 one-slot channel 只合并积压帧，不丢时间：elapsed 始终从上一次成功入队的
  帧算起。Engine 短暂繁忙时不会堆积补帧消息，也不会因被合并的 VBlank 少移动一段距离。
- macOS 用绑定到 overlay `NSView` 的 `CADisplayLink`；AppKit 会让 link 跟随 view 所在
  显示器。callback 使用显示时间戳并累计尚未消费的 elapsed，跨屏和主线程短暂繁忙时
  同样不丢移动时间。
- Mode 始终使用实际 elapsed time，不假定 60 Hz，也不使用固定 16 ms timer、刷新率查询或周期性
  刷新率查询。系统或 Engine 过载时无法承诺绘制每一个物理刷新帧；这里保证的是使用
  原生显示节拍、队列有界，以及合并帧不会改变按墙钟计算的移动距离。

## 修改渲染代码时必须保持

- Hook disposition 和点击注入不能等待渲染。
- producer 不得无限快于消费者；frame/scan channel 必须有界或合并。
- cursor/indicator 变化不得重建静态 Grid/Hint 内容。
- 静态 scene clone 必须保持写时复制；对 `OverlayItems` 排序或可变遍历只能发生在新场景
  构造/高 DPI cache miss 阶段。
- GPU 路径不得引入与屏幕像素数成比例的 CPU buffer。
- Hide 必须释放全屏 native surface、DIB、image 和 layer tree；macOS 不能只 `orderOut` 后
  假设 WindowServer、content view 与子图层会同步解除所有权。
- 新原生调用必须位于平台边界，使用 RAII/typed owner，并为最小 unsafe 块记录 SAFETY 契约。

最终性能结论必须来自双 4K、高 DPI、高刷新率真机的 p95/p99、分配次数、峰值 RAM/VRAM 和截图容差数据；API 名称或 GPU 标签本身不构成性能证明。
