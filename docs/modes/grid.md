# Grid 网格模式

Grid 把当前显示器划分成带标签的网格。

从 Normal 按 `g` 进入。默认布局为 5 列 × 4 行：

```text
1 2 3 4 5
q w e r t
a s d f g
z x c v b
```

初始画面会在每个一级格中央显示醒目的第一键，并在其内部铺一套较淡、较小的第二键
网格。

默认划分层级 `max_depth = 3`。

因为`Normal`模式默认按键和`Grid`没有冲突（按住 `Primary`）的临时 Normal模式是可选的，可以在不退出 Grid 的情况下移动、滚动或点击。

## 控制

| 按键 | 作用 |
| --- | --- |
| 标签键 | 选择单元格并进入下一层 |
| `` ` (Tab上的那个按键)`` | 切换选择时是否跟随鼠标 |
| `Primary+Q` / `Esc` | 返回 Normal |

网格标签优先于继承自 Normal 的同名按键；例如 `q` 会选择网格，而不是退出。

## 配置

```toml
[grid]
grid_cols = 5
grid_rows = 4
keys = "12345qwertasdfgzxcvb"
max_depth = 3
cursor_follow_selection = true

[grid.ui]
font_size = 20
border_width = 1
# 可选：初始预览的大字和内部边界颜色；小字沿用 text_color 并降低透明度
# matched_text_color = "#8FA2F0FF"
# matched_border_color = "#8FA2F0FF"

[grid.lifecycle]
after_finish = "normal"
after_click = "finish"
```

`keys` 必须正好包含 `grid_cols × grid_rows` 个字符，并按从左到右、从上到下对应单元格。
初始两键预览自动复用这套布局和按键，不改变实际深度，也不需要新增开关。中央大字
使用 matched 配色，内部小字沿用普通文字配色并自动变淡。

想在同一局部区域持续细分，可使用
[Recursive Grid](/modes/recursive-grid)。
