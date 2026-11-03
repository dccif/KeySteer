# Bisect 二分定位

<script setup>
import ModeVideo from '../.vitepress/components/ModeVideo'
</script>

Bisect 是不显示字母的递归定位模式。从 Normal 按 `Primary+F` 进入，用四个方向键反复保留当前区域的一半，鼠标默认跟随到区域中心。

<ModeVideo
  file="bisect-zh.mp4"
  title="Bisect 二分定位演示"
  description="用 H/J/K/L 连续二分，再按 Enter 将鼠标定位到目标；Enter 不会发送点击。"
/>

| 按键 | 作用 |
| --- | --- |
| `H` / `L` | 保留左／右半区，宽度减半 |
| `K` / `J` | 保留上／下半区，高度减半 |
| `U` | 在当前区域显示 UI Hint；按 `Tab`、`Backspace` 或 `Esc` 返回原二分区域 |
| `I` | 在当前区域启动 Text Select；按 `Esc` 返回当前二分区域 |
| `Tab` / `Backspace` | 撤销一次二分；根层再退则返回来源模式 |
| `Space` | 回到整屏 |
| `Enter` | 移到当前区域中心并完成选择，默认不点击 |
| `` ` `` | 切换光标跟随 |
| `Primary` | 临时使用 Normal 的移动、滚动和点击（发布配置已启用） |
| `Esc` | 返回来源模式 |
| `Primary+Q` | 返回 Normal；从 Window 借用时恢复窗口会话 |

两条中心参考线表示左右与上下的切分位置，每次按键只选择一个方向的半区。例如 `L → J → H` 会依次保留右半屏、该区域的下半区、再取其左半区。没有字母表、可调行列数或四分动作；方向键可以独立改绑。

区域 UI Hint 和 Text Select 使用各自现有的扫描和显示设置，Text Select 可以用 `Alt+C` 细分并复制部分文字。可在 `[bisect.bindings]` 改绑 `area_hint` 和 `area_text`；旧配置中已有的继承动作会保留。

## 和 Recursive Grid 共用的行为

区域填充、描边、到达终点后的背景沿用 Recursive Grid 的样式，颜色和线宽在 `[bisect.ui]` 单独配置。点击后默认保持当前区域，可以继续二分；确认或到达限制后，先撤销一层或重置才能继续选择。切屏插件开启 `preserve` 时会在新屏重放方向路径。

默认最多二分 20 次，最小宽高各为 2 个坐标单位。当宽度无法再缩小时仍可上下二分，反之亦然。达到最大次数或两个轴均无法继续时，执行 `after_finish`（默认 `keep`）。

## 独立配置

```toml
[normal.bindings]
"primary+f" = "bisect"

[bisect]
max_depth = 20
min_size_width = 2
min_size_height = 2
cursor_follow_selection = true
inherits = ["hotkeys", "normal"]
temporary_mode = "normal"
temporary_mode_keys = ["primary"]

[bisect.bindings]
h = "bisect_left"
j = "bisect_down"
k = "bisect_up"
l = "bisect_right"

[bisect.lifecycle]
after_finish = "keep"
after_click = "keep"

[bisect.ui]
line_width = 1
```

要改用方向箭头，例如设置 `h = "none"`、`arrow_left = "bisect_left"`，其余方向同理。临时 Normal 层会消耗激活键；若希望从其他模式按 `Primary+F` 直接进入 Bisect，请在该来源模式的 `bindings` 中显式配置这个组合键。

更多参数见[配置参考](/reference/configuration#bisect)。
