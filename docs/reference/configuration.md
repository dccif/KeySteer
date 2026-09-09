# 配置文件

KeySteer 不要求配置文件。没有配置时直接使用内置默认值，其行为与发布的 `keysteer.default.toml` 一致。

配置格式是 TOML，发布的完整示例可 [直接下载](/generated/keysteer.default.toml)。本文按“先能用、再定制、最后排错”的顺序介绍配置。

::: tip
只想改快捷键：[快速上手](/guide/getting-started) 中有介绍

想了解动作参数、数组、`exec` 和插件动词等高级配置：请参阅 [模式与动作参考](/reference/modes-and-actions)。
:::

## 配置文件位置

程序会在当前目录中查找 `keysteer.<名称>.toml`：不存在用户配置时，会尝试读取 `keysteer.default.toml`。如果连它也不存在，则直接使用内置默认值：

- Windows：可执行文件所在目录。
- macOS `.app`：`~/Library/Application Support/KeySteer/`。

文件名必须是 `keysteer.<名称>.toml`，例如 `keysteer.user.toml`。也可以显式指定：

```bash
keysteer --config keysteer.user.toml
keysteer --config ./profiles/keysteer.work.toml
keysteer --check --config keysteer.user.toml
```

常用诊断命令：

| 命令 | 用途 |
| --- | --- |
| `keysteer --check -c keysteer.user.toml` | 解析并校验配置，不启动运行时。 |
| `keysteer --dump-config` | 输出当前生效的完整配置。 |
| `keysteer --doctor` | 检查后端、键盘、显示器、权限和启动键。 |
| `keysteer --help` | 查看 CLI 选项和默认快捷键。 |

## 最小配置

```toml
[normal.bindings]
# 在 Normal 中用空格返回 Idle
space = "idle"
```

未写的字段保留默认值，但显式提供 `[normal.bindings]` 等绑定表会整体替换对应默认表；删除或改绑的插件快捷键不会被内部默认值补回。建议复制完整默认配置再修改。用户 profile 优先于 default 文件，二者不逐项合并。

保存后点击状态栏菜单的 Reload Configuration 即可生效，程序回到 Idle，再使用启动键进入 Normal。`left_shift = "slow"` 表示按住减速；改为 `shift = "slow_toggle"` 可用任一 Shift 点按切换减速。若保留 `right_shift = "middle_click"`，右 Shift 的专用绑定仍优先。

也可在已有的 `[normal.bindings]` 中加入数字速度档位：

```toml
"1" = "precision_toggle"
"2" = "slow_toggle"
"3" = "fast_toggle"
```

点按选择精确、慢速或快速档；再按同一个键恢复正常速度，按另一个键直接换档。倍率由 `[pointer]` 的 `precision_multiplier`、`slow_multiplier`、`fast_multiplier` 控制。Grid 等标签模式仍优先把数字作为标签输入。

## 配置结构

根配置常用 section 如下：

| Section | 作用 |
| --- | --- |
| `[general]` | 排除应用。 |
| `[key_aliases]` | 自定义键名和跨平台修饰键。 |
| `[hotkeys]` | `Idle` 中可触发 Mode 的入口。 |
| `[normal]`、`[grid]`、`[recursive_grid]`、`[ui_hint]` | 各 Mode 的继承、绑定和参数。 |
| `[pointer]`、`[scroll]` | 鼠标速度和滚动距离。 |
| `[theme]`、`[mode_indicator]` | 颜色和模式提示。 |
| `[[app_configs]]` | 按应用覆盖绑定。 |
| `[plugin_modes]` | 插件设置和插件 Mode 绑定。 |
| `[debug]` | 调试日志类别。 |

## 按键和别名

### 共享前缀的组合键

程序在加载配置时预编译组合键关系，内置动作和插件使用同一规则。默认窗口移动为 `primary+d`（Windows 为 Alt+D，macOS 为 Command+D），与切换鼠标显示器的 `primary+s` 独立触发。也可自定义共享前缀：

```toml
[normal.bindings]
"primary+s" = "screen next"
"primary+s+d" = "move_window next"
```

上述自定义配置中，保持 `Primary+S` 并按 D 移动窗口，只松开短组合则切换鼠标显示器。动作也可改成
`move_window previous` 或 `move_window 2`，也可使用任意其他组合键。最后一个非修饰键是完成键，
运行时会自动仲裁，不需要配置等待时间，也无需修改插件；`none`、按应用覆盖、继承及左右修饰键限制均参与判断。

模式切换或有效配置重载会取消尚未执行的短动作。单独修饰键保留即时行为；作为冲突前缀的
连续动作只在松开时执行一次完整按下/释放，建议为移动、长按点击或 toggle 保留独立按键。

### 按键写法

按键绑定的左侧支持单键、组合键和“多个单键共享动作”：

```toml
[normal]
long_press_toggle_ms = 500
auto_release_ms = 0

[normal.bindings]
h = "move_left"
"primary+shift+s" = "send primary+shift+s"
"v b" = "fast"
```

- `+` 表示同一个组合键。
- `空格` 表示多个独立按键绑定到同一个动作，不是顺序按键。
- 发布默认配置中，`primary` 在 macOS 是 `Command`、Windows 是左 `Alt` 。它是别名；若你要在 Windows 使用 `Ctrl`，可以在 `[key_aliases.windows]` 注释掉 。
- `ctrl`、`alt`、`shift` 等通用修饰键匹配左右两侧；`left_`/`right_` 前缀 只匹配指定一侧。

常用键名包括 `a-z`、`0-9`、`space`、`enter`、`esc`、`tab`、`delete`、`backspace`、`up`、`down`、`left`、`right`、`home`、`end`、`page_up`、`page_down`、`f1-f20` 和 `numpad_0-numpad_9`。

### 鼠标侧键

Windows 和 macOS 都支持把两个鼠标侧键作为绑定的左值：

| 按键名 | 含义 | 可用别名 |
| --- | --- | --- |
| `mouse_x1` | 第一个侧键，通常是后退键 | `xbutton1`、`mouse4` |
| `mouse_x2` | 第二个侧键，通常是前进键 | `xbutton2`、`mouse5` |

将需要的行加入已有配置的对应段，不要重复创建同名段：

```toml
[hotkeys]
mouse_x1 = "normal"          # 从 Idle 进入工作模式

[normal.bindings]
mouse_x1 = "key_help"        # 在工作模式中切换按键提示
mouse_x2 = "grid"            # 进入网格定位
"ctrl+mouse_x2" = "ui_hint"  # Ctrl + 第二个侧键进入 UI Hint
```

侧键支持普通组合键、绑定继承、应用覆盖和持续动作，例如 `mouse_x2 = "scroll_down"` 会在
按住时滚动、松开时停止。未绑定或设为 `none` 时保留原生前进/后退行为，即使当前模式独占
键盘也不会自动吞掉侧键；匹配绑定后会消费该次侧键的按下与松开。物理侧键不产生语义 `Clicked` 事件。
这些名称也可以作为右值动作，发送真实的鼠标侧键点击：

```toml
[normal.bindings]
t = "mouse_x1"  # 模拟第一个侧键
y = "mouse_x2"  # 模拟第二个侧键
```

同样支持 `press mouse_x1`、`release mouse_x1`、`toggle mouse_x2`。点击沿用普通点击的长按锁定规则；是否后退或前进由接收应用决定。鼠标驱动若已把物理侧键改成键盘快捷键，应绑定驱动实际输出的按键。

### 自定义别名

```toml
[key_aliases]
Hyper = "right_ctrl"

[key_aliases.windows]
Primary = "left_alt"

[key_aliases.macos]
Primary = "left_cmd"
```

顶层别名在所有平台生效；别名值必须是一个键，不能是组合键；不区分大小写。

`Primary` 只是一个 **可解析** 的跨平台别名，不是固定的物理键。发布默认值是 macOS Command、Windows `Alt`；上例正是把 Windows 的 `Primary` 显式设为 `Alt`。大小写不同的 `primary`/`Primary` 指向同一个别名。

### 绑定、数组和继承

右值可以是字符串，也可以是字符串数组：

```toml
[normal.bindings]
h = "move_left"
x = ["press shift", "left_click", "release shift"]
"primary+shift+b" = ["exec say start", "wait 300", "exec say done"]
```

数组中的动作从左到右执行。`wait` 不会阻塞整个事件循环，只暂停该序列；空数组不合法。

一个 Mode 的有效绑定按以下规则合并：

1. Mode 自己的 `[<mode>.bindings]`。
2. `inherits` 中列出的父 Mode，按书写顺序查找。
3. 当前应用匹配的 `app_configs` 覆盖合并结果。
4. 插件的建议绑定只填补空位，不覆盖用户设置。

这里的“合并”指运行时的**有效按键表**。程序只会进行覆盖替换。

```toml
[grid]
inherits = ["hotkeys", "normal"]

[grid.bindings]
q = "none" # 屏蔽从 normal 继承的 q
```

`none` 和 `__disabled__` 都表示明确禁用。建议保留至少一个 `[hotkeys]` 入口，否则程序仍会运行，但无法从 Idle 进入其他 Mode。

### 动作序列
```toml
[normal.bindings]
x = ["press shift", "left_click", "release shift"]
"primary+shift+b" = ["exec say start", "wait 300", "exec say done"]
```

完整动作、参数和 `exec` 规则见 [模式与动作](/reference/modes-and-actions)。

## Normal 和定位 Mode

### Normal

`Normal` 是控制鼠标直接移动、点击、滚动和进入其他 Mode 的平台：

```toml
[normal]
long_press_toggle_ms = 500
auto_release_ms = 0

[normal.bindings]
h = "move_left"
j = "move_down"
k = "move_up"
l = "move_right"
";" = "left_click"
g = "grid"
f = "recursive_grid"
"primary+f" = "ui_hint"
"primary+s" = "screen next"

# 可选：把 Primary+H/J/K/L 发送为应用的方向键。
# "primary+h" = "left"
# "primary+j" = "down"
# "primary+k" = "up"
# "primary+l" = "right"
```

`passthrough_unbound_keys = true` 是默认行为：Normal 只吞掉命中完整 KeySteer 绑定的输入，未绑定键及未配置的修饰组合会保持原始 down/up 生命周期并透传。

设为 `false` 时，Normal 恢复键盘独占，并保留旧的宽松组合匹配；`Grid`、`Recursive Grid` 和 `UI Hint` 始终保持独占。`Idle` 始终透传未命中的输入，并同样采用完整修饰组合匹配。

`long_press_toggle_ms` 作用于绑定为 `鼠标键` 的键和单独按住的无参数 `toggle` 激活键。鼠标键达到阈值后保持对应按钮按下；无参数 `toggle` 达到阈值后保持激活键自身按下，松开物理键不会撤销 latch。无参数 `toggle` 可绑定到任意激活键，并一次锁定任意数量的伙伴；伙伴与激活键无论谁先按下都会立即锁定，不等待长按阈值。伙伴按 Normal 绑定转换为实际目标，例如默认的 `;`、`'`、`right_shift` 分别保持鼠标左、右、中键，而不是保持这些物理键。单独短按无参数 `toggle`、返回 Normal 或进入 Idle 都会释放全部 latch。设为 `0` 禁用这两种长按行为，允许范围为 `0..=60000` 毫秒。

`auto_release_ms` 默认为 `0`，即保持上述手动释放行为。它仅适用于直接 click/double-click 绑定经长按形成的鼠标候选；一个或多个物理 Shift/Ctrl/Alt/Win 或 Command 键已按住并透传时，首次实际移动指针才开始计时，后续移动会重置计时。指针保持静止达到该时间后仅释放该鼠标按钮，并立即清除对应的按下提示，不影响物理按住的修饰键；显式 `press`/`toggle` 的 latch 不参与。允许范围为 `0..=60000` 毫秒。

### Grid

```toml
[grid]
grid_cols = 5
grid_rows = 4
keys = "12345qwertasdfgzxcvb"
max_depth = 3
cursor_follow_selection = true

[grid.lifecycle]
after_finish = "normal"
after_click = "finish"
```

`keys` 必须正好包含 `grid_cols × grid_rows` 个字符，按从左到右、从上到下填入。`max_depth` 是确认目标前的最大层数。初始画面会在一级格中央显示大号第一键，并在内部预览小号第二键；`[grid.ui]` 的 `matched_text_color` 控制大字，`text_color` 控制小字的基色，`matched_border_color` 控制内部细线。这个预览只影响绘制，不提前改变选择深度。

### Recursive Grid

```toml
[recursive_grid]
grid_cols = 3
grid_rows = 3
keys = "qweasdzxc"
max_depth = 10
min_size_width = 1
min_size_height = 1

[recursive_grid.lifecycle]
after_finish = "keep"
after_click = "keep"
```

`max_depth` 必须在 `1..=20`。`layers` 可以按深度覆盖网格形状；未写的字段继承基础设置：

```toml
[recursive_grid]
layers = [
  { depth = 0, grid_cols = 2, grid_rows = 2, keys = "crtn" },
]
```

### UI Hint

```toml
[ui_hint]
strategy = "hybrid" # axtree、vision 或 hybrid
hint_characters = "asdfghjkl"
scan_timeout_ms = 2500
scan_retry_count = 1
scan_retry_delay_ms = 200
visible_check_enabled = false
clickable_roles = ["button", "link", "checkbox", "text_field", "menu_item"]

[ui_hint.lifecycle]
after_finish = "normal"
after_click = "normal"
```

macOS 支持 Accessibility tree、Vision 和 Hybrid。Windows 默认使用 `hybrid`，将 UIA 与完整视觉管线并行执行、流式显示并去重合并；这能补足最小化、最大化、关闭等原生窗口按钮。`vision` 并行使用可用的系统 OCR 与自动发现的微信 OCR，并在 OCR 无结果时回退内置像素区域识别。OCR 不增加配置字段，也不随发行包分发微信组件。`clickable_roles` 是跨平台语义角色，也可以用 `ax:` 或 `uia:` 指定原生角色。

## Window 配置

`[window]` 控制窗口调整，`[window.bindings]` 控制独立的操作键。默认入口位于 `[hotkeys]`：`"alt+w" = "window"`。Window 默认只继承 `hotkeys`，按住 Primary 才临时使用 Normal；Window 的移动、缩放和布局方向统一跟随 Normal 的 `move_left/right/up/down` 绑定，默认 H/J/K/L，不再额外绑定箭头键。

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `enabled` | `true` | 注册 Window 模式。 |
| `inherits` | `["hotkeys"]` | 未被本地绑定覆盖时按顺序继承。 |
| `temporary_mode` / `temporary_mode_keys` | `"normal"` / `["primary"]` | 保留目标和子状态的临时模式。 |
| `double_tap_ms` | 兼容字段 | 旧配置可保留；AA 双击行为及计时已移除。 |
| `move_step` / `move_speed` | `20.0` / `600.0` | 短按步长／连续移动速度。 |
| `resize_step` / `resize_speed` | `20.0` / `500.0` | 中心缩放步长／速度。 |
| `gap` | `8.0` | 布局间距。 |
| `number_timeout_ms` | `250` | 仅歧义数字前缀等待，允许 100–2000ms。 |
| `split_ratios` | `["1/4", "1/3", "1/2", "2/3", "3/4"]` | 旧配置兼容字段，不再控制分割线。树编辑与窗口缩放共用 `resize_step` 和 `resize_speed`。 |
| `layout_keys` | `"123456789qwe"` | 旧配置解析兼容，不再控制新布局交互。 |
| `border_width` | `3.0` | 目标描边宽度。 |
| `ui.border_color` | 跟随主题 | 锁定目标的描边颜色；合并操作面板的字体、颜色及圆角使用 `[key_help]`。 |

步长和间距使用逻辑像素，速度使用逻辑像素／秒；Windows 按目标屏幕 DPI 换算。Tab 直接循环切换窗口并居中鼠标，不需要窗口标签或确认配置。

```toml
[window]
move_step = 12.0
resize_step = 10.0
gap = 12.0

[window.ui]
border_color = { dark = "#58A6FFFF", light = "#0969DAFF" }

[key_help]
font_size = 12
```

配置只写上述参数时保留默认操作键。如果写入 `[window.bindings]`，该绑定表会整体替换默认表；请从完整默认配置复制所需动作再改键，例如把 `z = "window_undo"` 改成 `x = "window_undo"`。应用覆盖支持 `[[window.app_configs]]`。全部动作见 [Window 模式](/reference/modes-and-actions#window-模式)，也可在配置编辑器的 Window 页签编辑并预览。

## 指针、滚动和主题

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

[scroll]
scroll_step = 50
scroll_step_half = 500
scroll_step_full = 1000000

[platform.macos.scroll]
invert_horizontal = false
invert_vertical = true
```

速度单位是像素/秒，加速度单位是像素/秒²，与显示器刷新率无关。`smooth_acceleration = true` 使用起步和收尾更柔和的 S 曲线；设为 `false` 使用线性加速。

主题颜色使用 `#RRGGBBAA`，可以为浅色和深色外观分别设置：

```toml
[theme.dark]
surface = "#0A1338FF"
accent = "#6E82D6FF"
accent_alt = "#8FA2F0FF"
on_accent_alt = "#081022FF"
text = "#E8EEFFFF"

[mode_indicator.cursor]
left_pressed_color = "#00FF00FF"
middle_pressed_color = "#FF00FFFF"
right_pressed_color = "#00FFFFFF"
```

鼠标按钮通过 `press` 或 `toggle` 保持按下时，透明圆形指示器使用对应的 `*_pressed_color`：填充使用配置颜色 20% 的不透明度。

## 应用覆盖：`[[app_configs]]`

应用覆盖可以禁用或替换某些程序里的绑定：

```toml
[[app_configs]]
bundle_id = "com.apple.Terminal"
bindings = { "primary+shift+e" = "none" }

[[normal.app_configs]]
bundle_id = "Figma"
bindings = { v = "none", "primary+f" = "grid" }
```

根级 `[[app_configs]]` 对所有 Mode 生效；`[[normal.app_configs]]` 只在 Normal 生效。匹配值可以是 macOS bundle id、Windows 可执行文件名、或窗口标题的子串。

## 插件设置

插件 Mode 使用命名空间：

```toml
[plugin_modes."plugin:screen-selector".settings]
preserve = true

[plugin_modes."plugin:screen-selector"]
inherits = ["hotkeys", "normal"]
```

自带 Screen Selector 的 `preserve = true` 会在切屏时保留 `Grid`/`Recursive Grid` 的选择路径；设为 `false` 则从目标显示器重新开始。

## 运行时修改与调试

`set_config` 可以修改点号路径，并在解析、校验通过后原子写回配置：

```toml
[normal.bindings]
"primary+1" = "set_config pointer.max_speed 800"
"primary+2" = "set_config theme.dark.accent \"#FF8800FF\""
```

无效修改不会替换当前有效配置。状态栏的 Reload Configuration 会重新加载配置。

```toml
[debug]
enabled = true
keys = true
actions = true
modes = true
backend = true
pointer = false
motion = false
overlay = true
timers = true
```

建议只在排查问题时开启调试日志；日志会写入数据目录中的 `keysteer.log`。

## 实时按键提示

`key_help` 动词切换按键提示面板。在 `[normal.bindings]` 中写入 `"?" = "key_help"` 即可启用；省略或注释该项即禁用，不需要 `none`。`?` 按字面匹配操作系统产生的问号字符，解析器不推测键盘布局或按法；targeting 模式按原有规则继承。进入 Idle 自动关闭。

`[key_help]` 只提供常用样式项：`enabled`、`font_family`、`font_size`、`background_color`、`text_color`、`border_color`、`border_width`、`border_radius`、`padding_x`、`padding_y`。空字体和未指定的背景/文字色跟随模式指示器与主题。颜色支持 `#RRGGBBAA` 或 `{ light = "#RRGGBBAA", dark = "#RRGGBBAA" }`。标题大小、分列和居中自动适配。

在[模拟器](/simulator)点击“编辑按键提示样式”，即可修改、预览并导出 TOML。


Window 的中央编号与分区编号使用 `[window.ui].font_size`，默认 28。分区编号位于窗口中心编号下方；帮助面板优先放在目标窗口内底部，不重复列出数字选窗键。


Window 的 `[window].exit_mode` 默认 `"return"`，退出根层时恢复进入前的模式和选择状态，也可配置为 `"normal"`、`"idle"` 或其他已注册模式。`[window.bindings]` 中 `q = "window_cancel"` 逐层返回，`a = "window_layout"`、`e = "window_edit"` 分别直接进入快速布局和编辑布局；`window_exit` 直接退出全部 Window 层级。这些动作可绑定其他按键，默认无需 Esc。
