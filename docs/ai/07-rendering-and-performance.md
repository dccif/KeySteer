# 覆盖层、帧同步与性能约束


## 统一场景构建

`src/presentation/` 统一负责 Grid、Recursive Grid、UI Hint、Window、屏幕选择器、按键帮助、
模式徽标和光标标记的布局、样式解析与场景原语构造。该层只依赖 `api`，不读取 Mode 实例、
TOML、输入路由或原生句柄；Windows/macOS 后端继续负责字体、窗口、surface 和实际栅格化。

Mode 只维护交互状态，通过 `HostContext::present(api::presentation::View)` 提交借用的视图。
HostContext 注入 `Presenter` 端口，宿主使用无状态 `presentation::Composer`，测试可替换实现。
视图同步消费，直接借用标签、库存、布局树和样式，不复制整个 Mode，也不在命令中保留借用。
输出仍是 `Command::ShowOverlay(Arc<OverlayScene>)`，因此命令顺序、Finish、批次合并和后端协议不变。

UI Hint 的重叠几何、冲突图和分层算法位于 `presentation/hint/`。`api::presentation::VisualLayerPlan`
是由 Mode 会话持有的可复用缓存；Mode 只管理失效、清理及 Shift 轮次，通过 Presenter 重建计划。
原有 inline 层号、宽路径 workspace 和借用式扫描数据继续复用，不增加逐帧重建。

运行时只收集有效绑定、显示模式、按住按钮、锚点和预览数据，再调用同一 presentation 层装饰场景。
`OverlayCoordinator` 保留单会话帮助缓存、scene 共享、去重、批次合并和位置快路径。
新增视觉组件时，在 API 定义借用视图并在 presentation 添加 compositor；复用已有原语即可，
只有新增原生绘制能力才扩展后端。内置 Mode/Plugin 不直接构造标签、形状或样式，架构测试锁定此边界。
低层 `ShowOverlay` 仍保留给直接提交场景的兼容调用方。

## 当前内存策略（2026-08）

Windows DirectComposition 保持设备、字体和紧致 cursor/indicator surface 预热，但 cursor-only Normal 不创建全屏 static surface。只有 backdrop、shape 或 label 存在时才挂载 static visual；回到无静态内容时立即释放 screen-sized surface。

Windows OCR 不属于预热常驻集。Backend ready 后首次事件轮询派发的 discovery 只缓存能力与路径，临时 `OcrEngine`/COM 在探测线程结束前释放；视觉 coordinator、截图、`SoftwareBitmap`、fallback scratch、微信 helper/reader 和临时 PNG 都是 generation-scoped，并在 terminal 结果进入 Engine 前完成清理。不要用 `SetProcessWorkingSetSize` 人为压低任务管理器数字。

热路径使用 inline storage：`CommandBatch` 的 0/1/2 命令不分配，Normal held-key map inline 4 项，Grid/Recursive stack/path inline 12 项，继承 visited inline 8 项。

## 提交模型

Window 的中心编号、区域描边只在状态变化时重新提交；500ms 库存结果相同时不重绘。库存结果以所有权交付，未变化条目直接复用；编号前缀索引只在可见成员、屏幕或区域数量改变时重建，数字输入复用缓冲区和 inline 输出。派生方向表仅在配置变化时重建。Window 的操作提示和 A 单个比例预览共用 key_help 的一个圆角背景、字体和键帽。Mode 只提供 `help_anchor` / `help_previews`，Host 根据有效绑定生成文字；面板优先按目标窗口的可见内部空间排版并贴近内底部，窗口过小时回退到工作区约束。帮助面板不重复列举窗口数字，窗口编号与中央下方的分区编号共用 `window.ui.font_size`（默认 28）。缓存比较锁定矩形、状态文字和缩略图选择；普通鼠标移动不会带走面板。Window 不再额外绘制浮动模式 badge。

`Backend::present` 接收 `Arc<OverlayScene>`。Windows 使用 latest-frame 单槽队列：Engine 只替换待绘制帧并立即返回，已经过期的帧不会进入原生绘制。frame/position 更新与 empty→ready 判定在同一次锁内完成，并用 outstanding-wake 合并突发提交；渲染线程每次 drain 才清除标志，积压位置更新合并为一次唤醒，并只保留最终位置。同一输入批次中的 Warp、Show、Finish、Click、Hide 会先合并覆盖层意图；输入注入保持立即执行，批次结束只提交一次最终画面。

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
原子 store 与 Release fence，不再为每个移动执行两次 locked RMW。pointer wake 只由 overlay `present/dismiss`
切换，停止移动帧时钟不会覆盖它。Idle 关闭 wake 后仍只更新 packed 坐标，进入非 Idle 模式
继续通过 `Backend::pointer()` 获取权威位置。

Normal 的活动方向使用四位 mask，而不是每个显示帧收集一个 `BTreeSet`。移动距离仍由
真实 elapsed 的解析积分决定；这个优化只消除逐帧堆分配，不改变对向抵消、对角线归一化
或多物理键绑定到同一方向时的去重语义。

Mode 热路径返回 `CommandBatch`：0/1/2 个命令不分配，第三个命令才 spill 到 `Vec`。
64 位布局测试限制 `Command <= 64 B`、`CommandBatch <= 128 B`、`ModeEvent <= 112 B`；
crate 内部的 `src/tests/performance.rs` 用 instrumented allocator 锁定预热后的 Normal frame
为零分配，并锁定生产 UI Hint owned delivery 的分配预算；用 `cargo test tests::performance::steady_normal_frames_do_not_allocate -- --ignored --exact --test-threads=1` 单独运行，
避免并行测试污染全局 allocator 计数。`cargo bench --features benchmark-hooks --bench core_hot_paths`
使用 release profile 和系统分配器；`benchmark-hooks` 只暴露既有生产构造器，不启用探针、替代算法或运行参数。基准通过该 doc-hidden hook 报告 p50/p95/p99。
Normal 与每个 Hint 规模固定使用 20k 样本；精准候选验收还需进行多轮交替 A/B。temporary-mode chord 与 keymap 一起
预编译，物理修饰键查通用别名时使用借用查表，不在按键路径构造临时 `Key`。
Engine 的稳定 `ModeSlot` 缓存活动模式、pointer interest 和已编译路由；优化只减少分派与
临时所有权开销，不改变 `CompiledKeymap` 的查找语义。UIHint 点击状态的 inline 2 指同时
活动的 `(Key, MouseButton)` 指示器条目，而不是 Hint 标签字符数；常见 0–1 项不分配，第三个
同时活动的点击状态才安全 spill。

cursor marker 由 Engine 在 mode scene 之上装饰；合成鼠标按钮进入 latched 状态，或等待
短按/长按判定的 click 物理触发键仍按住时，仅替换 marker 的填充/轮廓颜色并刷新动态
overlay，不改变 mode scene，也不重建静态 Grid/Hint 内容。latched 的真实按钮状态优先；
待定 click 使用最近仍按住的触发键，并由该键的释放事件清除。颜色提示本身不使用 timer 或 polling；
可配置的长按 Toggle 复用 Engine 的下一 deadline 超时，不创建周期轮询或额外线程。
长按和延迟 sequence 使用 deadline 倒序 `Vec`，等待轮询只查看尾项，到期从尾部弹出；常见
不超过 8 项的 Toggle 目标、回滚快照和按压事务使用栈内 `SmallVec`，超长组合才分配。

## 保存布局时的显示

布局列表由 Window 的只读 detail 提供，每页最多 6 条，与输入状态、分页和有效快捷键一起由 `presentation/key_help.rs` 放在工作区底部的单个面板内；不另建顶部列表场景。恢复状态使用简短独立徽标，列表内容使用面板正文。原生备注输入期间保留编辑场景、抑制帮助面板，以底部无边框输入栏接收备注；结果返回后重建当前视图。磁盘读写和二进制编码仅在布局库操作时执行。

Window 帮助使用运行时实际路由结果；名称映射只翻译动作，不过滤未列入映射的有效绑定。窗口数字已显示在场景中，不重复列举。列宽和文字使用同一宽度估算，空间不足时减少列数或换行，保持配置字号和完整动作说明。

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
- 每次 present/dismiss 都有独立 autorelease pool，AppKit/QuartzCore 的临时对象在真正的
  AppKit main run loop 中按当前提交边界释放，不依赖嵌套 event pump。
- Hide 先移除 shape、label、cursor，清除 root view 的 layer 和 window 的 content view，
  再关闭 `NSPanel` 并释放 typed owner；隐藏期间不保留完整 layer tree 或 compositor backing。

窗口仍在 AppKit 主线程创建和更新。Engine、输入 Hook 与扫描工作不通过原生对象共享状态，只通过有界队列/安全 mailbox 通信。

UI Hint 按需为目标名缓存一次 lowercase 结果；退出后立即 drop 所有目标、标签和 String，
不保留旧坐标或扫描结果。为避免常见不足 100 项的重入重新增长容器，最多保留 128 项的
空 `scanned`/`hints`/dedup backing；超过上限的 request-scoped backing 立即释放，因此大型
扫描不会成为 Idle 常驻内存。搜索重标记不得逐目标重新分配小写字符串。目标去重使用矩形
到 canonical index 的小碰撞表。重叠轮换在 UIA/视觉结果全部合并、Hint 分配和最终位置计算
之后，按最终可见矩形建立冲突图；矩形中心被遮挡、交叠面积达到较小标签 20%，或一方背景侵入
另一方扣除 padding 后的文字区才算肉眼堆叠，轻微边框/留白接触不增加 Shift 次数。每个连通
分量固定尝试绘制正序、逆序、相交度、行优先和列优先
五种 first-fit，并执行两次向低层压缩；优先选择层数最少、最保持最终遮挡关系的稳定结果。
不同分量共享全局轮换次数，但按各分量自己的深度映射到 `1..component_layer_count`；松开
Shift 恢复每个分量的第 0 层。这样默认层不会成为一次无变化的按下，两层小组也不会因为页面
其他位置有更深分量而出现空轮次，每次按住都稳定显示自己的第 1 层。

中间 Partial 始终流式显示；UIA/Vision 合并后按 24/48/96…累计发布边界更新同一视觉计划，
不按扫描节点重复计算。这样第一次 Shift 及扫描期间的后续 Shift 都只读取已有层号；晚到来源
加入后，下一个合并 Partial 用完整标签集合替换计划。常见不超过 128 项时冲突矩阵是约 2 KiB 的栈内
`[u128; 128]`，层号和分量深度压进同一个 inline `u16`，构建过程零堆；超出后才使用动态扁平
bitset 和必要的宽层号。宽路径先执行同一个精确 X sweep；完全没有视觉冲突时不分配或清零
冲突 bitset，发现第一条边后仍执行原有精确图构建、五种着色和两次压缩。这个快路不改变
`visually_stacked` 判定、层号或 Shift 映射。计划按完整 Hint 索引保存，筛选压缩不会遗漏标签；每个分量将全部层
映射到互不相同的稳定 z-index，并把当前 Shift 层映射到统一最高值。统一 compositor 单次线性生成标签，
Engine 随后执行已有的一次稳定 z 排序，保证 CPU/GPU 都不会重新遮住提升层；
不移动、裁剪或重新分配标签。Hint 前缀、Backspace 或名称查询改变可见集合时重建
计划；Windows 分层与 `DpiSceneCache` 共享同一个零分配 compact-label 几何 helper，按 scene clip
中心的显示器 scale 使用最终放大、取整后的矩形和 padding，macOS 则保持 1x 点坐标。已有前缀时
晚到 Partial 等前缀清空后才合入并重建。退出 UI Hint 后清空请求级计划。

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

`perf-probe` 启用且设置输出路径时使用固定有界队列；Engine/Hook/renderer 只做
非阻塞时间戳入队，JSONL 写入与 flush 在专用探针线程执行。队列满时丢探针记录而不是反压
输入，shutdown 在非热路径等待写出完成；marker 覆盖 `input_received`、`mode_handled`、
`commands_ready`、`native_submitted` 和原生 `native_presented`；普通 release 不编译该模块。
它是因果顺序诊断工具，插桩数据不能作为未插桩 release 的延迟验收结果。性能 A/B 必须使用
默认 feature 的 release 二进制，或双方都只启用 `benchmark-hooks` 的 `core_hot_paths`，且两侧采用相同构建配置。

macOS 原生探针使用固定 AppKit fixture 子进程，运行：
`bash tools/benchmark-macos-native.sh 709815c 5`。脚本在临时 detached worktree 构建同一探针，
交替执行基线和当前分支，原始日志与中位数汇总写入 `target/macos-native-bench/`；正式数据
必须在已授予 Accessibility 与 Screen Recording 的 Apple Silicon 实机采集。汇总阶段也会
验证 24 个 fixture 控件：AX 身份与矩形完全一致，Vision/Hybrid 的逻辑坐标误差不超过 1 px。
# Overlay allocation rules

- API v8 uses inline UTF-8 `OverlayText` and COW `SharedLabelStyle` for labels.
  The wire format remains unchanged. On supported 64-bit targets their layout
  gates are 24 bytes, 8 bytes, and at most 80 bytes for `OverlayLabel`.
- Hint resolves one shared label style per scene. Windows DPI scaling interns
  scaled styles by source identity and scale instead of detaching every label.
- Scene sorting first performs an O(n) ordered check and only runs the stable
  sort when required; equal-z source order remains stable.
- Common mode-indicator refreshes resolve configured text lazily and assemble
  held-input text directly into one exact-capacity `String`; do not reintroduce
  per-target canonical/replace/uppercase strings or an intermediate `Vec`.
- Cursor-indicator mode overrides resolve through a borrowed internal view;
  full refreshes must not clone the configured themed-color strings.
- A rejected position-only backend update disables that fast path until the
  overlay is dismissed. Operational failures are reported once through the
  unified logger instead of being swallowed or retried on every pointer event.

## 实时按键提示

`Binding::KeyHelp` 通过普通按键 resolver 和 apply_binding 切换面板；通过 `"?" = "key_help"` 显式启用，省略或注释该绑定即禁用，没有独立输入拦截。OverlayCoordinator 保存显示开关及缓存，沿用离散动作的 repeat 与 Up 配对消费。Idle 不显示面板，进入 Idle 关闭。关闭提示从当前有效 key_help 绑定生成。

面板从编译后绑定枚举候选，复用 Engine 的 active/inherited/temporary resolver 检查实际动作；组合键前缀按住期间筛选对应候选。`Mode::available_keys` 提供默认空实现，内置 Grid、UI Hint 和屏幕选择器按实例当前状态报告原始输入，UI Hint 只报告剩余标签下一字符。该 API 不调用原生能力、不改变模式状态。键盘事件后刷新，流式扫描或模式重绘时同步更新。

提示使用现有 OverlayLabel、主题和原生静态层，在当前屏幕底部居中显示单个圆角面板，复用 mode/toggle indicator 配色和字体；等价动作合并按键，面板内部以透明文字分列排布。面板底色作为最高文字层下的一张空文字标签绘制，以遮住底层 Hint 标签而无需改动原生渲染边界；模式原始 scene 不包含帮助标签，关闭时恢复原 scene。鼠标位置更新沿用已有快路径，不增加定时器、线程或全屏 CPU buffer。

帮助面板的按键以浅底深字圆角键帽展示，同一列键帽右边缘对齐，功能说明使用原生左对齐。`LabelStyle::text_alignment` 默认 Center 保持既有 Grid/Hint/indicator 行为，帮助说明设为 Left；Windows DirectWrite format 缓存包含 alignment，GDI DrawText 与 matched-prefix 起点按同一 alignment 计算；macOS 设置 CATextLayer 对齐并同步前缀裁剪起点。对齐不依赖字符数估算的居中偏移。

帮助面板按每列最长键帽和功能说明估算内容宽度，不固定撑满屏幕；`[key_help]` 配置字体、颜色、边框、圆角与内边距；默认水平内边距 24、垂直内边距 8。行高和标题区随字号计算，列间距 6、键帽与说明间距 8 保持内部布局常量。超长自定义绑定受工作区宽度限制，仍复用现有文字适配。

等价动作的按键保持为一个完整组，不按字符长度拆分。Grid、Recursive Grid 不在帮助面板重复列出网格上已有的选择字符，仅报告返回、确认、重启等控制键。

键帽沿用模式提示字体和 5 像素逻辑左右内边距。

帮助列表的键帽与功能说明统一使用 key_help.font_size（默认 12）；列宽按同一字号计算。工作区不足时只允许整张列表统一缩放，不按单条文本长度分别缩小；标题和关闭提示按正文字号的 1.25 倍自动计算。

帮助正文按实际列总宽度在面板中居中，包括最小面板宽度产生的剩余空间；功能说明使用较紧凑的独立文字框，避免右侧保留键帽宽度估算带来的额外空白。

## 帮助面板缓存与寿命

居中没有固定横坐标：外层 `area.x + (area.width - width) / 2`，正文 `panel.x + (panel.width - body_width) / 2`。工作区、DPI、当前按键集合及各列文字宽度估算共同决定布局；边距与间距是样式尺寸，不是中心偏移补偿。

`OverlayCoordinator::key_help_cache` 只保存一个可见会话的缓存。源标签和最终装饰标签使用 `OverlayItems` 的 Arc 共享，不额外复制已提交的标签数组；相同输入场景、模式、主题和屏幕几何直接借用整组最终标签，跳过候选解析、String 拼装、样式分配、COW 和排序。普通位置更新仍走原有快路径，非空 Grid/Hint 场景跨屏也检查帮助面板的屏幕几何，触发必要的重新居中。

物理输入边沿、非运动 ModeEvent、新 ShowOverlay、扫描结果和路由重编译使缓存失效；主题、显示模式、工作区及 DPI 还在命中判断中检查。Frame/PointerMoved 不直接使缓存失效；若自定义 Mode 在这两类事件中改变其 `available_keys`，应像视觉内容改变一样发出 ShowOverlay。未注入的重复输入不再单独追加一次帮助刷新；Mode 需要重绘时仍可通过 ShowOverlay 刷新。焦点改变后帮助开启时显式刷新最终生效路由。

关闭、Idle、Reload/overlay reset 与 shutdown 释放缓存；没有跨会话历史表、后台任务或定时轮询。构建时的 String/Vec 为临时所有权，显示期间所需文字由场景持有，最后一个 Arc 释放后回收。后端可能还持有最后一帧或字体缓存，这是已有有界渲染资源，不是每次帮助刷新累积的新条目。

性能探针位于 runtime/tests/overlay.rs：release 下单线程运行 `key_help_decoration_probe`、`key_help_cache_close_cycles_release_allocations`、`key_help_cache_position_updates_allocate_nothing`，每项 20k 次；前两项分别验证缓存命中零分配、反复开关分配与释放平衡，第三项验证开启帮助后的鼠标位置路径零分配。功能测试覆盖标签存储共享、键输入/流式结果失效、路由/主题更新和跨屏动态居中。

帮助面板关闭时，`handle_key` 直接进入既有输入处理，不追加提示刷新收尾。注册表查询复用已有的活动模式槽位缓存，取用前验证槽位中的 ModeId，避免按键路由重复搜索模式索引；切换模式、重载和其他模式查询仍安全回退到索引表。没有字符数据的输入直接走物理键解析；仅有带修饰键的组合前缀时，单个普通键跳过前缀仲裁。上述优化均位于公共运行时。

Window compositor 统一生成大编号、加粗应用名与更长标题卡片；背景保持最终物理矩形，文字使用同一精确 DPI 布局规则，避免背景与文字缩放不一致。按完整 footprint 避让，偏移卡片绘制 3px 逻辑引线。编辑区域使用网格遮罩、深浅边界与区域中央编号；区域编号不依赖窗口卡片位置。帮助面板只用目标矩形决定位置，始终保持配置字号，小窗口放不下时移到工作区内；模式状态用独立键帽，应用名与标题各占一行。长按分割线复用现有帧时钟与布局事务的 latest-desired 合并，不新增定时器或原生绘制窗口。
