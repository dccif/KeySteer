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

`n = "toggle"` 可以控制按键的按下还是松开状态。可以和任意按键或者修饰符结合，比如 `Shift` `alt` `ctrl` 等

如果持续按住 `鼠标按键` 键达到 `long_press_toggle_ms`，鼠标会进入 `Toggle` 状态，也就是按下模式；点击 `n = "toggle"` 可以释放
```toml
[normal]
passthrough_unbound_keys = true
long_press_toggle_ms = 500 # 设为 0 可关闭
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
