# 内置模式、插件与 Finish 生命周期

## Mode 契约

所有内置模式和插件都实现 `api::Mode`：

```rust
fn id(&self) -> ModeId;
fn handle(&mut self, event: &ModeEvent, ctx: &HostContext) -> CommandBatch;
fn handle_owned(&mut self, event: ModeEvent, ctx: &HostContext) -> CommandBatch; // 默认转发
fn captures_keyboard(&self) -> bool;
```

Mode 是有状态但平台无关的对象。它可以读取屏幕、光标、前台应用、palette 和只读 host
settings；不能注入输入、创建窗口或直接扫描 UI。
内置 Mode/Plugin 用 `ctx.present(View)` 提交状态视图；布局、字体、颜色解析和标签/形状构建均位于
`src/presentation/`，模式只依赖 API 端口，不依赖具体 composer。参见 [统一场景构建](07-rendering-and-performance.md#统一场景构建)。
只有需要消费 `UiScanned` 大型载荷的 Mode 才覆盖 `handle_owned`；Frame、指针和按键仍直接走
`handle`，默认实现保证已有插件源码兼容。

## 六个内置 Mode

### Idle (`src/modes/idle.rs`)

- 启动/恢复失败后的静默状态。
- 不捕获键盘，只依靠 Engine 从 `[hotkeys]` 解析 launcher。
- 激活时隐藏覆盖层，其他事件不产生命令。

### Normal (`src/modes/normal.rs`)

- `normal.passthrough_unbound_keys` 默认开启，因此只捕获完整命中的绑定；关闭后恢复键盘独占。
- Idle 和默认 Normal 使用严格修饰匹配：额外修饰键必须属于 chord，或其物理 down 已被
  KeySteer 自身绑定消费。这样外部 `Alt+H` 不会命中裸 `h`，但 `left_shift=slow` 后的
  `Shift+H` 仍成立。
- 持有方向、滚动和速度手势状态。
- `precision`/`slow`/`fast` 是按住型速度手势；对应的 `*_toggle` 绑定在 Normal 内锁存速度，
  并通过 Engine-owned indicator 第二行反馈当前锁存值。
- 参数化 `toggle` 捕获速度键时只锁存其物理键目标，不重新执行速度动作，避免速度状态影响 toggle。
- 连续移动优先由原生 frame clock 驱动；第一下有 `tap_distance`，避免极短按键无移动。
- 使用真实 elapsed time、可配置的 smootherstep/线性加速度和 sub-pixel remainder；曲线按
  解析积分计算，对角线归一化。
- key repeat 只在 display frame 尚不可用时作为 fallback。
- 离散点击、模式切换、send/exec 等由 Engine 执行，不在 Normal 重复实现。
- Normal click 键和单独按住的无参数 `toggle` 激活键可按 `long_press_toggle_ms` 建立 Engine
  deadline；直接 click/double-click 在 KeyDown 立即 MouseDown，未到期的 KeyUp 立即 MouseUp，
  到期后只把现有按压转交给 latched Toggle，不等待 deadline 才响应，也不先注入完整点击。
  物理键释放不释放已经 Toggle 的鼠标按钮或激活键自身；组合伙伴出现时会取消激活键的自锁 deadline。
  无参数 toggle 的伙伴允许先于激活键按下并立即命中；若伙伴的 Down 已经透传，处理其 Up 后会
  立即重新注入 Down。伙伴按 Normal 最终语义映射为键盘或鼠标目标，并以幂等 Press 累积；已经
  latch 的修饰键参与后续完整 chord 查找。激活后的伙伴边沿在执行其自身 binding 前消费；pending MouseDown 的所有权在
  Release、取消和 Reload 清理前先转交给 latched recovery，失败后仍可由恢复或 shutdown 重试。
  toggle session 可跨 Grid/UI Hint 等临时定位模式保留；返回 Normal、进入 Idle 或 shutdown 时，
  pending MouseDown 与全部 latch 走同一反序释放路径。这些路径只使用现有 deadline 和 disposition，
  不增加 timer、线程或普通按键等待。
- `normal.auto_release_ms` 只给上述直接 click/double-click 长按产生的鼠标 latch 增加一个可选 owner。
  候选出现后，Engine 以一个 `u8` 跟踪后续透传的左右 Shift/Ctrl/Alt/Win(Command)；存在至少一个
  透传修饰键时，首次真实位移建立 deadline，后续物理移动、MovePointer 或 WarpPointer 的有效位移
  只重置同一 deadline。完整显式 chord 始终优先；没有完整匹配时才忽略本次透传修饰键并借用
  Normal 的 `Move`，`none` 与其他动作不会被回退绕过。默认 0 路径不读取时钟，也不增加系统 timer、
  worker、锁或堆分配。自动释放成功后完整刷新一次动态装饰，且长按决议时普通 click feedback 的所有权
  转交给 latch，避免位置快路继续搬运旧的 held badge 或按下色。Reload 只取消自动 owner，保留已经存在的手动 latch；离开 Normal、capture loss、
  Disable 和 shutdown 则走可恢复的 MouseUp 清理。

### Grid (`src/modes/grid.rs`)

- 以当前屏幕为 root，按 `grid_rows * grid_cols` 和 `keys` 逐层缩小。
- depth 0 在每个一级格中央绘制醒目的大号第一键，并在其下绘制淡色的小号第二键装饰
  网格；它不修改 stack/path，第一次选择后的 depth 1 及后续 scene 保持原有单层行为。
- 与 Recursive Grid 共用 `targeting::TargetingSession` 保存选择路径、return mode、finished、
  cursor-follow 和生命周期公共转换；Grid 自己只拥有矩形布局和绘制状态。
- 每层 label 自动缩放以适应单元格；最终层建立完成态再触发生命周期。
- finished 后 Backspace 取消完成态并回退一层；`keep` 不重建 Mode。

### Recursive Grid (`src/modes/recursive_grid.rs`)

- 复用 `targeting::TargetingSession` 保存 Rect stack、选择路径和生命周期状态，每次按键在当前区域继续递归细分。
- `layers` 可按 depth 覆盖形状；`min_size`/`max_depth` 决定自然终点。
- 支持 label background、最小字号、自动隐藏和下层 key preview。
- 当前默认点击后 `keep`，仍可继续输入字母细分；不是冻结完成态。
- Backspace 弹出 stack，Space/重启语义重置当前 session。

### UI Hint (`src/modes/hint/mod.rs`)

- 激活后发送 `ScanUi`；按 scan id 接收多个 Partial 和一个终态。
- 累积/去重 `UiTarget`，使用 `modes/hint/labeling.rs` 重新分配短标签。
- 普通输入筛 label prefix；`/` 进入 accessible-name 搜索。
- Partial 始终在 UIA/Vision 合并、去重、重新分配短标签后立即显示，并按累计发布批次用完整
  当前集合替换视觉计划。扫描期间 Shift 只读取已准备层号；晚到来源会触发完整集合重建，
  不把新层追加到旧计划。已有 Hint 前缀时，晚到 Partial 保留目标但不重排现有键码，前缀
  清空后再统一合入并重建计划。
- 视觉计划按相交连通分量复用冲突 bitset，并以绘制正序、逆序、相交度、行优先和列优先五种
  固定 first-fit 顺序择优、压低层号；中心被遮挡、交叠达到较小标签 20%，或标签背景已经侵入
  另一标签扣除 padding 后的文字内容区时才算冲突。仅边框或留白接触不增加层数。
  不同分量共享全局轮换次数，但按各分量自己的深度映射到非默认层；松开 Shift 显示第 0 层，
  按下只在每个分量的非默认层 `1→2→…→1` 间循环。因而页面其他位置存在更深重叠时，
  `ajh/ajj` 这样的两层小组也会在每次按住时稳定显示第 1 层，不会出现空轮次。Hint 前缀、
  Backspace 和名称搜索每次改变可见集合后都会重建计划。每个分量的所有层都有独立且稳定的
  z-index，当前层始终最高；Engine 在交给原生后端前执行一次稳定 z 排序。分层绝不移动、裁剪、
  删除或重新分配标签。Windows 的冲突矩形使用与 DPI renderer 相同的最终放大及取整几何，
  因而 125%/150% 下肉眼已经重叠的标签不会在计划中仍被误判为互不相交；macOS 保持点坐标。
- 选中后 warp 到目标并 Finish；默认返回 Normal，不自动点击、不重新扫描。
- 只有扫描以 `Success`/`TimedOut` 结束且没有标签时，才按配置安排有上限的自动 retry。目标/焦点/显示器变化直接清空旧结果并启动新 generation，不消耗 retry 次数。
- Windows 每代扫描的是原生提交瞬间鼠标下的窗口组；普通鼠标移动不轮询、不持续重扫。鼠标下没有应用窗口时只显示移动鼠标提示，不假定用户仍使用默认重扫快捷键。
- 离开或完成本轮 UI Hint 时取消仍在进行的原生扫描；再次进入始终重新获取目标，不能复用
  上一轮可能已经过期的控件坐标。`Deactivated` 会标记实例 inactive、清空目标/搜索/重叠计划并释放大型 backing；迟到的异步结果不能让 Normal/Idle 后台重新启动扫描。

`hint/session.rs::ScanSession` 是 scan generation、Partial 去重结果、retry、搜索缓存和
finished/active 标志的唯一 owner；`labeling.rs` 与 `view.rs` 保持纯标签和视觉层算法。前缀匹配
直接作用于 session 的紧凑 Hint 索引，不保留只供测试使用的重复 matching 实现。

### Window (`src/modes/window.rs`)

- 会话保存稳定窗口编号，中心标签包含被遮挡的普通窗口。`numbering.rs` 缓存有效编号的前缀索引，仅歧义前缀启动一次性计时；完整编号才选窗／交换。反引号切换区域编号输入。
- `inventory.rs` 接收拥有所有权的结果，复用未变化库存与可见编号索引；明确关闭通知回收编号与布局引用，取消查询不丢关闭通知。
- `editing.rs` 持有 QuickPlacement/BSP 模型、上一成功布局、32 步本地历史和单个在途修订；连续输入只保留最新目标。进入树编辑后等待异步约束与完整库存，首次空间导入只做一次。
- `interaction.rs` 路由快速布局、树导航、分割、祖先比例、交换、撤销与返回／退出。AA 只在入口双击期限内且没有其他操作时生效。
- 树的 Ctrl＋方向调整借用配置编译后的 `split_ratios`，从当前实际比例跳到指定方向的相邻档位，不在按键路径解析分数或构建数组。
- `view.rs` 借用窗口库存和布局树，`presentation/window.rs` 构造编号和稳定区域描边，复用锚定窗口下方的单个 key_help 面板。布局方向来自 Registry 编译的当前有效 Normal Move 绑定，临时 Normal 优先，E 入口优先于方向。
- 500ms 可见态计时合并后台库存请求，原生查询异步且可被布局／选窗抢占；相同结果不重绘，不在按键或 Frame 枚举。关闭窗口保留空区域和其余编号。
- worker 拥有约束快照与原生事务；修改即时生效，无需 Enter。Esc 返回、Q 退出前发送最新布局并将整轮记为一步撤销；强制退出／重载只释放检查点，保留已应用几何。临时 Normal 保留编辑、停止新布局提交并隐藏覆盖层。
- Tab 普通态按稳定环直接激活锁定，树内仅本屏；焦点被拒绝仍锁定目标并说明原因。所有原生对象留在平台层。

## Finish 不是 Mode

Finish 是当前 targeting session 的幂等完成态：

1. 自然选择终点或 `finish` verb 产生 `FinishRequested`。
2. Mode 设置 `finished = true`，保留最终目标/路径并提交 finished 视图。
3. 执行 `after_finish`。
4. 成功的 KeySteer click 产生一次 `Clicked`，执行 `after_click`。

已经 finished 时再次 Finish 不重复执行 `after_finish`。`keep` 返回空 Command，因此实例、
路径和 return mode 都原样保留；`restart` 才清空本轮状态并收到 `Restarted`。

物理鼠标点击不生成 `Clicked`。合成的 press/release/toggle 也不生成；只有 click 和
double-click 成功后生成。普通 click 仍在物理键按下沿原子执行；成功后 Engine 只保留
一个视觉按钮状态，直到同一个物理键释放。该状态跨 Mode 切换保留，不拥有合成鼠标按钮，
也不改变 `Clicked` 次数。

## return mode 与 modal mode

- 普通 `SwitchMode` 会让新 Mode 在 `Activated { previous }` 中记录 return mode。
- `return` 生命周期动作回到该记录值，不等同硬编码 Normal。
- `PushMode` 将当前 Mode 放入 modal stack，发送 `Suspended`；关闭后 `PopMode` 发送
  `Resumed`，状态没有被销毁。

## Screen Selector 插件

`src/plugins/builtin/screen_selector.rs` 是架构示例：

- 只使用公共 `Manifest`、`Mode`、`Command`、geometry 和 overlay API。
- 导出 `screen` verb，可 next/previous/编号切换，也可 push 一个数字选择 overlay。
- `preserve` 设置决定 Grid/Recursive Grid 跨屏时是否重放逻辑路径。
- 默认建议 `primary+s -> screen next`，但用户已占用时不覆盖。

新增插件能力应优先扩充公共 API，而不是让插件向下依赖 `Engine` 或平台模块。

## Window Mover 插件

`src/plugins/builtin/window_mover.rs` 导出 `move_window previous/next/<编号>`，返回
`Command::MoveWindowToScreen(WindowScreenTarget)`。Engine 委托 `Backend::move_window_to_screen`，
鼠标跟随窗口并保持窗口内的相对位置，不切换 Mode、不发 `Clicked`。默认建议
`primary+d -> move_window next`，与 `primary+s -> screen next` 独立触发；用户可自定义共享前缀组合，由 Engine 仲裁。
插件不判断物理按键，也不拥有等待状态。
默认绑定遵循 key aliases，用户绑定和 `none` 优先，可在任意 Mode 中配置插件 verb。
