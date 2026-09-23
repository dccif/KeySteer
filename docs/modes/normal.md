# Normal 普通模式

<script setup>
import ModeVideo from '../.vitepress/components/ModeVideo'
import KeyLayout from '../.vitepress/components/KeyLayout'
</script>

`Normal` 可能是最常用和简单上手的：直接移动鼠标、滚动、点击、拖拽，并进入三种定位模式。按 `Primary+E` 从 Idle 进入

默认只接管已经绑定的按键；未绑定输入仍会到达当前应用、AHK、Quicker 等工具。例如只有 `h = "move_left"` 时，裸 `h` 移动鼠标，而未显式绑定的 `Alt+H` 会完整透传。

```toml
[normal]
passthrough_unbound_keys = true # 设为 false 可恢复键盘独占
```

<ModeVideo
  file="normal.mp4"
  title="Normal 模式演示"
  description="展示键盘移动鼠标、速度修饰、滚动和常用点击操作。"
/>

## 默认按键

<KeyLayout
  layout="q w e r t y u i o p/Caps a s d f g h j k l ; '/Shift z x c v b n m , . Slash RShift/Ctrl Primary Alt Space"
  move="h j k l"
  click="; ' RShift"
  speed="Caps Shift v b"
  scroll="m ,"
  state="n"
  navigation="t y u i"
  mode="e f g q Primary"
  label="Normal 默认键位分区"
  hint="颜色表示动作类型；空白键未占用"
/>

| 按键 | 作用 |
| --- | --- |
| `h` `j` `k` `l` | 左、下、上、右移动 |
| `Caps Lock` / `Left Shift` | 精确 / 慢速移动 |
| `v` 或 `b` | 快速移动 |
| `m` / `,` | 向下 / 向上滚动 |
| `;` / `'` / `Right Shift` | 左键 / 右键 / 中键点击 |
| `n` | 切换任意按键按住状态，适合拖拽 |
| `t` / `y` / `i` / `u` |  `Home` / `End` / `Page Up` / `Page Down` |
| `g` / `f` / `Primary+F` | `Grid` / `Recursive Grid` / `UI Hint` |
| `Primary+S` | 切换到下一块显示器 |
| `q` / `Esc` | 返回 Idle |

## 移动速度

默认的 `precision`、`slow` 和 `fast` 需要按住。若希望点按切换，可在
`[normal.bindings]` 中使用 `precision_toggle`、`slow_toggle` 或 `fast_toggle`；
再次按同一绑定关闭，当前锁存速度会显示在模式指示器下方。

移动速度以像素/秒和加速度计算， 默认的平滑加速会在起步和接近最高速度时放缓速度变化

将 `smooth_acceleration` 设为 `false` 可恢复线性加速。

```toml
[pointer]
initial_speed = 1000.0
max_speed = 2200.0
acceleration = 3000.0
smooth_acceleration = true
tap_distance = 2.5
slow_multiplier = 0.35
precision_multiplier = 0.12
fast_multiplier = 2.0
```

## 拖拽与长按

`n = "toggle"` 可以让任意数量的伙伴保持按下，伙伴先按或后按都可以。伙伴会先按 Normal 绑定转换为实际目标：例如默认的 `;`、`'`、`right_shift` 会分别保持鼠标左、右、中键，而不是这些物理键。单独短按 `n`、返回 Normal 或进入 Idle 会释放全部目标。

如果持续按住 `鼠标按键` 键达到 `long_press_toggle_ms`，鼠标会进入 `Toggle` 状态，也就是按下模式；点击 `n = "toggle"` 可以释放

如果希望“按住修饰键拖动”结束时少按一次释放键，可将 `auto_release_ms` 设为非零值。它仅适用于直接绑定为 click/double-click 的键经长按形成的鼠标候选；一个或多个物理 Shift/Ctrl/Alt/Win 或 Command 键已按住并透传时，首次实际移动才开始计时，后续移动会重置计时。指针停止达到该时间后只释放该鼠标按钮并立即清除按下提示，不影响你手动按住的修饰键；显式 `press`/`toggle` 创建的 latch 不参与。

```toml
[normal]
passthrough_unbound_keys = true
long_press_toggle_ms = 500 # 设为 0 可关闭
auto_release_ms = 0        # 设为 0 保持手动释放
```

## 自定义快捷键

```toml
[normal.bindings]
"primary+space" = "left_click"
"primary+g" = "grid"
q = "idle"
```
::: warning 注意
在定位 Mode 中，`Grid` 标签或 `UI Hint` 标签优先于继承来的 `Normal` 模式按键；按住 `Primary` 可临时使用 `Normal`。
:::

## 临时输入文本

Normal 默认按 `\` 进入 Text Input，普通文字透传到应用。进入和返回均使用普通绑定：

```toml
[normal.bindings]
'\' = "text_input"

[text_input]
inherits = []
temporary_mode = "normal"
temporary_mode_keys = ["primary"]
temporary_mode_passthrough_keys = []

[text_input.bindings]
'enter \ esc' = "normal"

# 可选：先把 Enter 发送给应用，再返回 Normal。
# 用下面两行替换上面的合并绑定：
# '\ esc' = "normal"
# enter = ["send enter", "normal"]
```

默认返回键会被消费，不发送给应用。需要 Enter 正常用于多行编辑或输入法选词时，从返回绑定中移除 enter 即可。显式 bindings 表替换默认值，请在现有 Normal 绑定表中追加入口，保留其他操作。

按住 Primary 临时使用 Normal，松开继续输入；通过 `key_aliases` 解析，与其他模式一致。也支持 `inherits = ["normal"]`，本模式绑定优先；完整继承会让未覆盖的 HJKL 等字母执行 Normal 动作，因此连续打字通常使用临时层。进入文本输入或松开临时层触发键时停止借用的手势并释放锁定输入，长按 Q 不启动快速切换。

### 可选主键区编辑映射

默认配置中的编辑组合键全部为注释示例，按需取消注释启用：

| 组合键 | 发送的按键 |
| --- | --- |
| Primary + H/J/K/L | 左／下／上／右箭头 |
| Primary + U/I/O | Backspace／Delete／Insert |
| Primary + T/Y | Home／End |

还提供 Ctrl、Shift、Ctrl+Shift 的注释示例。请选择与 Primary 实际映射不重复的修饰键。例如 Primary 是 Ctrl 时，不使用 Ctrl+Primary 的示例。

```toml
# 在 [text_input.bindings] 中按需启用：
# "primary+h" = "arrow_left"
# "ctrl+primary+h" = "send ctrl+arrow_left"
# "shift+primary+h" = "send shift+arrow_left"
```

显式完整组合键优先于临时层中去掉触发键后的普通绑定。映射使用现有 Send 的源修饰键暂停／恢复和系统按键重复逻辑，不改变应用对目标快捷键的处理。

文本输入模式默认不显示文字提示，可在 `[mode_indicator.modes.text_input]` 中用 `enabled` 自定义。临时 Normal 仍使用 Normal 的提示设置，且 `s = "screen next"` 等插件动作的结果由 Normal 处理。
## 模式标识符样式与位置

程序和网页现在使用相同的标识符尺寸与定位公式，移除了额外的预览屏幕缩放补偿。坐标原点是系统鼠标热点（通常为箭头尖端，也是圆形指示器中心），不是箭头图片的中心。Windows 使用桌面坐标像素，macOS 使用逻辑点；标识符后端不再二次缩放尺寸，字体栅格仍可能有细微差异。

通常只需修改全局 `[mode_indicator.ui]`；默认配置已集中列出字体、圆角、内边距、边框及位置。网页模拟器的「全局设置 → 模式标识符」可直接编辑并实时预览。各模式「外观」页保留独立覆盖，重置字段即可恢复继承全局样式。

模式标识符可单独启用、改名，并覆盖全局 `[mode_indicator.ui]` 样式。将下面配置合并到现有同名表中：

```toml
[mode_indicator.modes.normal]
enabled = true
text = "Normal"

[mode_indicator.ui]
indicator_offset = [-12, 18]
font_size = 13
background_color = "#0A1338FF"
text_color = "#E8EEFFFF"
border_radius = 6
padding_x = 8
padding_y = 4

[mode_indicator.modes.text_input]
enabled = false
```

`indicator_offset = [X, Y]` 定位标识符的**右上角**：X 正数向右、负数向左；Y 正数向下、负数向上。两项都是 -32768～32767 的整数。默认 `[-12, 18]` 表示右上角在鼠标左侧 12、下方 18。网页可拖动橙色锚点、用方向键微调（Shift 为 10），或直接输入数组；显示器边缘会限制整个标识符的位置。

各模式 `[mode_indicator.modes.normal.ui]` 等表可覆盖同名字段。从 0.10.22 起，旧 `position`、`indicator_x_offset` 和 `indicator_y_offset` 已移除，请使用 `indicator_offset = [X, Y]`；未设置时使用默认值或继承全局配置。Text Input 默认隐藏文字，若需要显示，设 `enabled = true`，并在 `[mode_indicator.modes.text_input.ui]` 中配置相同字段。
