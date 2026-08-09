# Recursive Grid 递归网格

<script setup>
import ModeVideo from '../.vitepress/components/ModeVideo'
import KeyLayout from '../.vitepress/components/KeyLayout'
</script>

`Recursive Grid` 会在当前区域中反复细分，像逐层放大的地图。它适合小图标、细小按钮、画布对象，以及需要连续操作同一区域的场景。

<ModeVideo
  file="recursivegrid.mp4"
  title="Recursive Grid 模式演示"
  description="展示在当前区域逐层细分、回退和继续定位细小目标。"
/>

从 Normal 按 `f` 进入，默认是 3 × 3：

<KeyLayout
  layout="qwe/asd/zxc"
  target="q w e a s d z x c"
  label="Recursive Grid 默认键位"
/>

## 和 `Grid` 的区别

- `Grid` 通常在固定层数后完成，适合快速到达一个区域。
- `Recursive Grid` 默认在点击后保留会话，可以继续从当前区域细分。

## 控制

| 按键 | 作用 |
| --- | --- |
| 标签键 | 在当前格中继续细分 |
| `Backspace` / `Tab` | 返回上一层；根层再退则退出 |
| `Space` | 回到第一层 |
| `Enter` | 移动到当前区域中心，不点击 |
| `` ` (Tab上的那个按键)`` | 切换每层是否跟随鼠标 |
| `Primary` | 临时使用 Normal 的移动、滚动和点击 |
| `Primary+Q` / `Esc` | 返回 Normal |

## 配置

```toml
[recursive_grid]
grid_cols = 3
grid_rows = 3
keys = "qweasdzxc"
max_depth = 10
min_size_width = 1
min_size_height = 1
cursor_follow_selection = true

[recursive_grid.lifecycle]
after_finish = "keep"
after_click = "keep"
```
