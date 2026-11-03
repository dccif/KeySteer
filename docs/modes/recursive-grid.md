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

从 Normal 按 `Primary+G` 进入，默认是 3 × 3：

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
| `U` | 在当前区域显示 UI Hint；按 `Tab`、`Backspace` 或 `Esc` 返回原网格区域 |
| `I` | 在当前区域启动 Text Select；按 `Esc` 返回当前网格 |
| `Backspace` / `Tab` | 返回上一层；根层再退则退出 |
| `Space` | 回到第一层 |
| `Enter` | 移动到当前区域中心，不点击 |
| `` ` ``（Tab 上方的键） | 切换每层是否跟随鼠标 |
| `Primary` | 临时使用 Normal 的移动、滚动和点击 |
| `Primary+Q` / `Esc` | 返回 Normal |

::: tip 小按钮可以慢慢找
按错一层用 `Tab` 返回，想从头再来用空格。`Enter` 只把鼠标移到中心，需要点击时再按住 `Primary` 使用 Normal 的点击键。
:::

扫描沿用 UI Hint 或 Text Select 的设置，范围是当前网格区域。Text Select 可以用 `Alt+C` 细分并复制部分文字。可在 `[recursive_grid.bindings]` 改绑 `area_hint` 和 `area_text`；旧配置中的同名标签或继承动作会保留。

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

想用方向键逐次二分且不显示字母，可以从 Normal 按 `Primary+F` 使用 [Bisect](/modes/bisect)。
