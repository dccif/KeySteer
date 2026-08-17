# 覆盖层、帧同步与性能约束

## 当前内存策略（2026-08）

Windows DirectComposition 保持设备、字体和紧致 cursor/indicator surface 预热，但 cursor-only Normal 不创建全屏 static surface。只有 backdrop、shape 或 label 存在时才挂载 static visual；回到无静态内容时立即释放 screen-sized surface。

Windows OCR 不属于预热常驻集。Backend ready 后首次事件轮询派发的 discovery 只缓存能力与路径，临时 `OcrEngine`/COM 在探测线程结束前释放；视觉 coordinator、截图、`SoftwareBitmap`、fallback scratch、微信 helper/reader 和临时 PNG 都是 generation-scoped，并在 terminal 结果进入 Engine 前完成清理。15 秒空闲基线约为 35.5 MiB working set、51.4 MiB private bytes、343 handles、30 threads，采样保持平稳；高于早期 18 MiB 的主要部分是预热的 DirectComposition/D3D/字体资源及系统工作集缓存，不能用 `SetProcessWorkingSetSize` 人为压低任务管理器数字。

热路径使用 inline storage：`CommandBatch` 的 0/1/2 命令不分配，Normal held-key map inline 4 项，Grid/Recursive stack/path inline 12 项，继承 visited inline 8 项。

## 提交模型

`Backend::present` 接收 `Arc<OverlayScene>`。Windows 使用 latest-frame 单槽队列：Engine 只替换待绘制帧并立即返回，已经过期的帧不会进入原生绘制。frame/position 更新与 empty→ready 判定在同一次锁内完成，并用 outstanding-wake 合并突发提交；渲染线程每次 drain 才清除标志，8000 次积压位置更新只需要一个 wake 并只保留最终位置。同一输入批次中的 Warp、Show、Finish、Click、Hide 会先合并覆盖层意图；输入注入保持立即执行，批次结束只提交一次最终画面。

Windows 截图不再生成约 32 MiB 的 4K BGRA `Vec`。GDI DIB 在所属线程上以验证过长度的临时 slice 借出；截图仍始终只有一次。系统 OCR 从该 slice 直接写入按逻辑线程与至少 64px 核心边长自适应的重叠小块，完成一块即发布；只有微信能力可用时才创建微信专用完整 bitmap，并在 PNG 编码后立即关闭；fallback 直接生成不超过 2,073,600 像素的灰度图，随后销毁 DIB。8,388,608 像素上限让 UHD 4K 原生命中一次 `BitBlt`；只有 5K/8K 等实际超限画面才使用 `StretchBlt + HALFTONE`。fallback 灰度映射使用商余数步进，连通区域按 scanline run 维护活跃组件；形态位图复用并使用最多 2000 项的紧凑 top-K，只有最终候选才构造 role 字符串。

Engine 保存最后一次 scene 并跳过完全相同的提交。`OverlayScene` 的静态 shapes/labels 使用
`OverlayItems<T>` 写时复制存储：移动 cursor/indicator 时 clone 只增加 `Arc` 引用计数，
不会复制数百个标签；共享存储的相等判断也有指针快速路径。场景只在进入 Engine 时排序
一次，逐帧刷新不得再次排序并触发写时复制。序列化仍保持普通数组格式。Grid 使用
`Rect::subdivision` 直接计算单格，Recursive Grid 的层级布局在配置应用阶段编译，选择
热路径不构造完整 `Vec<Cell>`。

Engine 另外缓存自己生成的 cursor/indicator 几何。普通指针移动只调用
`Backend::update_overlay_positions`，不再克隆 scene、重新生成 held 文字或比较静态内容；
跨屏 clip、appearance、样式、held 状态和模式内容变化仍提交完整 scene。未实现快路径、
首帧未就绪或原生更新失败时立即退回完整 `present`。

Windows 低级鼠标 Hook 是 pointer seqlock 的唯一写者，因此写侧使用 odd/payload/even 的普通
原子 store 与 Release fence，不再为每个移动执行两次 locked RMW。release 交错微测中每批
8,000 次写入的 p99 从约 63.1µs 降到 11.1µs；pointer wake 关闭时仍只更新 packed 坐标，
模式切换继续通过 `Backend::pointer()` 获取权威位置。

Normal 的活动方向使用四位 mask，而不是每个显示帧收集一个 `BTreeSet`。移动距离仍由
真实 elapsed 的解析积分决定；这个优化只消除逐帧堆分配，不改变对向抵消、对角线归一化
或多物理键绑定到同一方向时的去重语义。

Mode 热路径返回 `CommandBatch`：0/1/2 个命令不分配，第三个命令才 spill 到 `Vec`。
64 位布局测试限制 `Command <= 64 B`、`CommandBatch <= 128 B`、`ModeEvent <= 112 B`；
`tests/performance.rs` 用 instrumented allocator 锁定预热后的 Normal frame 为零分配，
`cargo bench --bench core_hot_paths` 报告其 p50/p95/p99。temporary-mode chord 与 keymap 一起
预编译，物理修饰键查通用别名时使用借用查表，不在按键路径构造临时 `Key`。
Engine 的稳定 `ModeSlot` 缓存活动模式、pointer interest 和已编译路由；优化只减少分派与
临时所有权开销，不改变 `CompiledKeymap` 的查找语义。UIHint 点击状态的 inline 2 指同时
活动的 `(Key, MouseButton)` 指示器条目，而不是 Hint 标签字符数；常见 0–1 项不分配，第三个
同时活动的点击状态才安全 spill。

cursor marker 由 Engine 在 mode scene 之上装饰；合成鼠标按钮进入 latched 状态，或普通
click 的物理触发键仍按住时，仅替换 marker 的填充/轮廓颜色并刷新动态 overlay，不改变
mode scene，也不重建静态 Grid/Hint 内容。latched 的真实按钮状态优先；普通 click 使用
最近仍按住的触发键，并由该键的释放事件清除。颜色提示本身不使用 timer 或 polling；
可配置的长按 Toggle 复用 Engine 的下一 deadline 超时，不创建周期轮询或额外线程。
长按和延迟 sequence 使用 deadline 倒序 `Vec`，等待轮询只查看尾项，到期从尾部弹出；常见
不超过 8 项的 Toggle 目标、回滚快照和按压事务使用栈内 `SmallVec`，超长组合才分配。

## Windows GPU 主路径

`src/platform/windows/gpu_overlay.rs` 是默认渲染器：

- 渲染线程独占透明 click-through HWND、D3D11 BGRA device、DXGI device、Direct2D context、DirectWrite factory 和 DirectComposition device。
- Direct2D 直接绘制到 DirectComposition surface；GPU 路径没有全屏 CPU RGBA/DIB，也没有上传 memcpy。
- Rect、Line、文字、cursor 与 indicator 在 GPU surface 中完成；颜色 brush、字体 format 和 UTF-16 scratch buffer 有界复用。
- cursor/indicator 的像素内容与屏幕位置分开失效：普通移动只更新 DirectComposition visual
  offset 并提交一次 compositor commit，不再重新 BeginDraw、栅格化圆形或绘制不变文字；
  半径、颜色、文字、held 状态、样式或 surface 尺寸变化时才重绘紧凑 surface。
- 静态 surface 只在静态 scene 或覆盖区域原点变化时访问和重绘；区域不变的逐帧提交不会
  重复调用全屏 `SetWindowPos`，也不会为静态 surface 产生 COM AddRef/Release。
- GPU HWND 使用 `WS_EX_LAYERED | WS_EX_TRANSPARENT | NOACTIVATE | TOOLWINDOW | TOPMOST`；layered + transparent 才保证跨进程点击穿透，`HTTRANSPARENT` 仅作为额外防线，因为它只能继续命中同线程窗口。
- overlay 不使用 capture affinity。视觉扫描控制优先于普通帧：generation gate 在已有 deferred 完整帧时才保留其后的最终位置；没有基准帧的位置更新直接丢弃，release 不产生 position-only wake。GPU 仅在存在 content 时清空 tree 并异步 `Commit`，CPU 销毁 layered HWND。renderer 维护 capture-clean 状态：从未显示或上次隐藏已经确认时跳过屏障；只有可能仍有标签像素参与合成时，截图路径才执行且只执行一次 `DwmFlush`。普通 dismiss 只进入 latest control 槽并唤醒 renderer，不等待回执；Engine、Hook 和普通 overlay 提交路径同样不等待 compositor。wake 失败或 renderer 退出会原子清空 frame、position、control 与 capture 槽并唤醒 waiter，旧 scene 不会保留到 backend drop。
- 渲染线程使用 Win32 消息队列作为 latest-frame/control 唤醒源；即使 normal 覆盖层静止也会
  持续响应窗口消息，并对 `WM_NCHITTEST` 返回 `HTTRANSPARENT`。禁止让全屏 HWND 在
  Condvar/普通 channel 上无限等待，否则 Windows 会将其判为挂起并阻塞底层窗口输入。
- Hide 销毁 HWND/visual/surface，只保留轻量 device、brush 和字体描述缓存。
- Idle 的物理鼠标移动只更新 Hook 的原子 latest-pointer，不唤醒 Engine；进入非 Idle 模式前由 Engine 调用 `Backend::pointer()` 刷新权威坐标。Hook 的事件 sink 是线程私有状态，不在每个按键边缘获取全局 mutex。
- Windows 高 DPI 标签缩放由渲染线程按“源标签共享存储 + scale”缓存。cursor/indicator
  更新复用已缩放静态标签；源场景变化、缩放变化时才重建。进入空场景、回到 100%
  缩放、Hide 和 Shutdown 都会清空该缓存，不能把上一张大型 Grid 留在 Normal 中。

恢复规则位于 `overlay_worker.rs`：

1. GPU 初始化失败时直接启用持久 DIB renderer。
2. GPU present/commit 失败时重建一次完整 GPU device tree，并重画最新帧。
3. 60 秒内再次失败则本次会话固定使用 DIB，避免设备抖动反复重建。

Windows 原生探针以 ignored release tests 保存，避免进入普通 CI 或发布二进制。使用
`cargo test --release native_performance_probe -- --ignored --nocapture --test-threads=1`
运行聚合入口；它报告键盘批次、Hook FIFO/disposition、GPU 初始化/首帧/移动帧的
p50/p95/p99，以及 GPU ready、first present、steady motion、dismissed 四个阶段的进程
working set、private bytes、handle 和 thread 数。

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
- area、backing scale、静态内容和两个动态层分别失效。区域不变时不重复枚举屏幕或设置
  `NSPanel`/root view/root layer frame；已显示的 panel 不重复 `orderFrontRegardless`。
- cursor 的内容与中心点分开缓存，普通移动只写一次 frame。indicator 使用独立容器 layer，
  普通移动只写容器 frame；文字、held、样式、尺寸或 Retina scale 改变才配置内部文字层。
- CALayer/CATextLayer 按逻辑标签身份而不是 scene 排序下标复用；UI Hint 输入筛选或 Shift
  调整重叠层级时，现有原生文字层不会换绑到另一段文字。每个标签保留普通色和高亮色两份
  同源 attributed `CATextLayer`，输入前缀只改变轻量裁剪容器宽度，不替换文字对象；裁剪边界
  由当前字体、字号和实际 UTF-16 前缀排版宽度计算，不使用字符数比例。公共层一次无分配分析
  UTF-16 范围和下伸字母，Windows/macOS 只应用各自的垂直策略；位置、背景、边框、输入前缀
  或 z-index 变化都不会触发文字重新栅格化。
  scene 缩小时，多余 shape 与 label 会从父层移除并释放。
  因此 Grid 首屏的二级预览层不会在返回 Normal 后继续成为常驻高水位缓存。
- 所有属性更新放在禁用隐式动画的 `CATransaction` 中，避免输入后出现动画拖尾。
- 每次 present/dismiss 都有独立 autorelease pool，AppKit/QuartzCore 的临时对象不会依赖
  下一次手动 run-loop pump 才释放。
- Hide 先移除 shape、label、cursor，清除 root view 的 layer 和 window 的 content view，
  再关闭 `NSPanel` 并释放 typed owner；隐藏期间不保留完整 layer tree 或 compositor backing。

窗口仍在 AppKit 主线程创建和更新。Engine、输入 Hook 与扫描工作不通过原生对象共享状态，只通过有界队列/安全 mailbox 通信。

UI Hint 按需为目标名缓存一次 lowercase 结果；退出后立即 drop 所有目标、标签和 String，
不保留旧坐标或扫描结果。为避免常见不足 100 项的重入重新增长容器，最多保留 128 项的
空 `scanned`/`hints`/dedup backing；超过上限的 request-scoped backing 立即释放，因此大型
扫描不会成为 Idle 常驻内存。搜索重标记不得逐目标重新分配小写字符串。目标去重使用矩形
到 canonical index 的小碰撞表，标签重叠分组使用按左边界排序的扫描线与并查集。
重叠轮换使用一次横向扫描和贪心层标记，将全部相交标签分配到全局非相交层；每次 Shift
整体提升一个层。order/layers/active/marks 工作表使用 inline 128 的 `SmallVec`，常见不足
100 项的切换不触发临时堆分配，超出 128 项仍安全 spill；不再维护并查集或对每个连通组
单独取模。

## 帧时钟

- Windows 11 优先使用 `DCompositionWaitForCompositorClock` 的 display-independent
  compositor heartbeat。它原生覆盖不同刷新率和多适配器屏幕，并把 stop event 纳入同一次
  无限期等待，因此停止移动不必等下一次 VBlank，也不需要跨屏重建时钟。持续移动期间用
  `DCompositionBoostCompositorClock` 请求动态刷新高频模式，停止和所有错误出口都会撤销。
- compositor-clock 路径不读取目标 monitor，Engine 的每帧 retarget 因而跳过
  `MonitorFromPoint`；只有 DXGI fallback 才在目标越出缓存屏幕或拓扑变化时重新解析输出。
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

每个候选必须先单独运行至少 20k 样本的小测。非安全项只有 p99 改善至少 3%或分配/内存
下降至少 5%，且其他关键 p99 回退不超过 2%时才能进入组合测试；例如 UIA control-type
位集在常见内置 role 上慢于小常量线性表，已经撤销，不能因理论复杂度更低而保留。

`perf-probe` 启用且设置输出路径时使用固定 4096 项的有界队列；Engine/Hook/renderer 只做
非阻塞时间戳入队，JSONL 写入与 flush 在专用探针线程执行。队列满时丢探针记录而不是反压
输入，shutdown 在非热路径等待写出完成；marker 覆盖 `input_received`、`mode_handled`、
`commands_ready`、`native_submitted` 和原生 `native_presented`；普通 release 不编译该模块。

macOS 原生探针使用固定 AppKit fixture 子进程，运行：
`bash tools/benchmark-macos-native.sh 709815c 5`。脚本在临时 detached worktree 构建同一探针，
交替执行基线和当前分支，原始日志与中位数汇总写入 `target/macos-native-bench/`；正式数据
必须在已授予 Accessibility 与 Screen Recording 的 Apple Silicon 实机采集。汇总阶段也会
验证 24 个 fixture 控件：AX 身份与矩形完全一致，Vision/Hybrid 的逻辑坐标误差不超过 1 px。
