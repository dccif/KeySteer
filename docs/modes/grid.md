# Grid 网格模式

<script setup>
import ModeVideo from '../.vitepress/components/ModeVideo'
import KeyLayout from '../.vitepress/components/KeyLayout'
</script>

目标在屏幕另一边？`Grid` 给屏幕铺上带字母和数字的格子。连续输入目标所在格的标签，就能快速缩小范围，不用一路移动过去。

<ModeVideo
  file="grid.mp4"
  title="Grid 模式演示"
  description="展示一级大标签与内部二级预览，以及连续输入标签完成快速定位。"
/>

从 Normal 按 `g` 进入。默认布局为 5 列 × 4 行：

<KeyLayout
  layout="12345/qwert/asdfg/zxcvb"
  target="1 2 3 4 5 q w e r t a s d f g z x c v b"
  label="Grid 默认键位"
/>

初始画面会在每个一级格中央显示醒目的第一键，并在其内部铺一套较淡、较小的第二键网格。

默认划分层级 `max_depth = 3`。

::: tip 先跳过去，再微调
按住 `Primary`，用 Normal 的移动、滚动和点击键微调；松开后继续选格。
:::

## 控制

| 按键 | 作用 |
| --- | --- |
| 标签键 | 选择单元格并进入下一层 |
| `U` | 在当前网格区域显示 UI Hint；按 `Tab`、`Backspace` 或 `Esc` 返回原网格区域 |
| `I` | 对当前网格区域启动 Text Select；按 `Esc` 返回该网格 |
| `` ` ``（Tab 上方的键） | 切换选择时是否跟随鼠标 |
| `Primary+Q` / `Esc` | 返回 `Normal` |

网格标签优先于继承自 `Normal` 的同名按键；例如 `q` 会选择网格，而不是退出。

`U` 和 `I` 分别沿用 UI Hint 与 Text Select 的扫描、显示和操作方式，只扫描当前区域。Text Select 中可按 `Alt+C` 逐级细分文字，然后复制所选部分。可在 `[grid.bindings]` 改绑 `area_hint` 和 `area_text`；旧配置中已用于标签或继承自定义动作的按键会保留原用途。

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

想在同一局部区域持续细分，可使用 [Recursive Grid](/modes/recursive-grid)。
