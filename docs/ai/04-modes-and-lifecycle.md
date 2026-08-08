# 内置模式、插件与 Finish 生命周期

## Mode 契约

所有内置模式和插件都实现 `api::Mode`：

```rust
fn id(&self) -> ModeId;
fn handle(&mut self, event: &ModeEvent, ctx: &HostContext) -> Vec<Command>;
fn captures_keyboard(&self) -> bool;
```

Mode 是有状态但平台无关的对象。它可以读取屏幕、光标、前台应用、palette 和只读 host
settings；不能注入输入、创建窗口或直接扫描 UI。

## 五个内置 Mode

### Idle (`src/modes/idle.rs`)

- 启动/恢复失败后的静默状态。
- 不捕获键盘，只依靠 Engine 从 `[hotkeys]` 解析 launcher。
- 激活时隐藏覆盖层，其他事件不产生命令。

### Normal (`src/modes/normal.rs`)

- 持有方向、滚动和速度手势状态。
- 连续移动优先由原生 frame clock 驱动；第一下有 `tap_distance`，避免极短按键无移动。
- 使用真实 elapsed time、可配置的 smootherstep/线性加速度和 sub-pixel remainder；曲线按
  解析积分计算，对角线归一化。
- key repeat 只在 display frame 尚不可用时作为 fallback。
- 离散点击、模式切换、send/exec 等由 Engine 执行，不在 Normal 重复实现。
- Normal click 键可按 `long_press_toggle_ms` 建立 Engine deadline；到期后复用 latched Toggle，
  物理键释放只取消未到期项，不释放已经 Toggle 的鼠标按钮。

### Grid (`src/modes/grid.rs`)

- 以当前屏幕为 root，按 `grid_rows * grid_cols` 和 `keys` 逐层缩小。
- depth 0 在每个一级格中央绘制醒目的大号第一键，并在其下绘制淡色的小号第二键装饰
  网格；它不修改 stack/path，第一次选择后的 depth 1 及后续 scene 保持原有单层行为。
- 保存选择路径、当前区域、return mode、finished、cursor-follow session 状态。
- 每层 label 自动缩放以适应单元格；最终层建立完成态再触发生命周期。
- finished 后 Backspace 取消完成态并回退一层；`keep` 不重建 Mode。

### Recursive Grid (`src/modes/recursive_grid.rs`)

- 保存 Rect stack 和选择路径，每次按键在当前区域继续递归细分。
- `layers` 可按 depth 覆盖形状；`min_size`/`max_depth` 决定自然终点。
- 支持 label background、最小字号、自动隐藏和下层 key preview。
- 当前默认点击后 `keep`，仍可继续输入字母细分；不是冻结完成态。
- Backspace 弹出 stack，Space/重启语义重置当前 session。

### UI Hint (`src/modes/hint.rs`)

- 激活后发送 `ScanUi`；按 scan id 接收多个 Partial 和一个终态。
- 累积/去重 `UiTarget`，使用 `domain/hints` 重新分配短标签。
- 普通输入筛 label prefix；`/` 进入 accessible-name 搜索。
- overlap cycle key 轮换重叠标签层级。
- 选中后 warp 到目标并 Finish；默认返回 Normal，不自动点击、不重新扫描。
- 没有标签且扫描终止时按配置安排有上限的自动 retry。

## Finish 不是 Mode

Finish 是当前 targeting session 的幂等完成态：

1. 自然选择终点或 `finish` verb 产生 `FinishRequested`。
2. Mode 设置 `finished = true`，保留最终目标/路径并绘制 finished scene。
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
