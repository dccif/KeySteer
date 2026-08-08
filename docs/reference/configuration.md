# 配置文件

KeySteer 不要求配置文件。没有配置时直接使用内置默认值，其行为与发布的 `keysteer.default.toml` 完全一致；该文件只是带注释的可复制示例。标量、开关和独立配置段可以只写需要改变的项；绑定表见下面的“绑定、数组和继承”。

配置格式是 TOML，发布的完整示例可[直接下载](/generated/keysteer.default.toml)。本文按“先能用、再定制、最后排错”的顺序介绍配置。

> 只想改快捷键：从[快速上手](/guide/getting-started)开始。想了解动作参数、数组、`exec` 和插件动词：直接看[模式与动作参考](/reference/modes-and-actions)。

## 配置文件位置

自动发现时，程序会在数据目录中查找 `keysteer.<名称>.toml`：非 `default` 的用户配置优先并按文件名排序；只有不存在用户配置时，才读取 `keysteer.default.toml`。如果连它也不存在，则直接使用内置默认值：

- Windows portable 或裸二进制：可执行文件所在目录。
- 打包的 macOS `.app`：`~/Library/Application Support/KeySteer/`。

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

所有未写的**非表项**字段都会保留默认值。`[normal.bindings]` 这类映射一旦写出，会成为该 Mode 的本地绑定表：想保留默认绑定时，请从 `--dump-config` 或默认 TOML 复制需要的条目，再做修改。配置字段拼写错误不会被静默忽略，程序会在加载时报告错误。

## 配置结构

根配置常用 section 如下：

| Section | 作用 |
| --- | --- |
| `[general]` | 排除应用。 |
| `[key_aliases]` | 自定义键名和跨平台修饰键。 |
| `[hotkeys]` | Idle 中可触发 Mode 的入口。 |
| `[normal]`、`[grid]`、`[recursive_grid]`、`[ui_hint]` | 各 Mode 的继承、绑定和参数。 |
| `[pointer]`、`[scroll]` | 鼠标速度和滚动距离。 |
| `[theme]`、`[mode_indicator]` | 颜色和模式提示。 |
| `[[app_configs]]` | 按应用覆盖绑定。 |
| `[plugin_modes]` | 插件设置和插件 Mode 绑定。 |
| `[debug]` | 调试日志类别。 |

## 按键和别名

### 按键写法

按键绑定的左侧支持单键、组合键和“多个单键共享动作”：

```toml
[normal]
long_press_toggle_ms = 500

[normal.bindings]
h = "move_left"
"primary+shift+s" = "send primary+shift+s"
"v b" = "fast"
```

- `+` 表示同一个组合键。
- 空格表示多个独立按键绑定到同一个动作，不是顺序按键。
- 发布默认配置中，`primary` 在 macOS 是 Command、Windows 是左 Alt、Linux 是 Ctrl。它是别名；若你要在 Windows 使用 Ctrl，请在 `[key_aliases.windows]` 覆盖它。
- `ctrl`、`alt`、`shift` 等通用修饰键匹配左右两侧；`left_`/`right_` 只匹配指定一侧。

常用键名包括 `a-z`、`0-9`、`space`、`enter`、`esc`、`tab`、`delete`、`backspace`、`up`、`down`、`left`、`right`、`home`、`end`、`page_up`、`page_down`、`f1-f20` 和 `numpad_0-numpad_9`。

### 自定义别名

```toml
[key_aliases]
Hyper = "right_ctrl"

[key_aliases.windows]
Primary = "left_alt"

[key_aliases.macos]
Primary = "left_cmd"
```

顶层别名在所有平台生效；当前平台的子表覆盖同名值。别名值必须是一个键，不能是组合键；别名可以链式引用。别名不区分大小写。

`Primary` 只是一个可解析的别名，不是永远固定的物理键。发布默认值是 macOS Command、Windows `Alt`、Linux Ctrl；上例正是把 Windows 的 `Primary` 显式设为 `Alt`。大小写不同的 `primary`/`Primary` 指向同一个别名。

## 绑定、数组和继承

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

这里的“合并”指运行时的**有效按键表**。程序不会把你写出的 `[normal.bindings]` 与内置的本地 Normal 表逐键深合并；它会替换该 Mode 的本地表，再按上述继承规则取得父表和插件建议绑定。因此，下面的最小示例只保留 `space` 作为 Normal 的本地绑定；需要 `hjkl`、点击或进入 Grid 的默认键时应一并写出。

```toml
[grid]
inherits = ["hotkeys", "normal"]

[grid.bindings]
q = "none" # 屏蔽从 normal 继承的 q
```

`none` 和 `__disabled__` 都表示明确禁用。建议保留至少一个 `[hotkeys]` 入口，否则程序仍会运行，但无法从 Idle 进入其他 Mode。

## 动作序列
```toml
[normal.bindings]
x = ["press shift", "left_click", "release shift"]
"primary+shift+b" = ["exec say start", "wait 300", "exec say done"]
```

完整动作、参数和 `exec` 规则见[模式与动作参考](/reference/modes-and-actions)。
## Normal 和定位 Mode

### Normal

Normal 是直接移动、点击、滚动和进入其他 Mode 的工作台：

```toml
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

`long_press_toggle_ms` 只作用于绑定为 鼠标键。持续按住达到阈值后，将对应鼠标按钮保持按下，松开物理键不会释放。再次长按同一点击键或使用无参数 `toggle` 组合可释放。设为 `0` 禁用，允许范围为 `0..=60000` 毫秒。

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
strategy = "vision" # axtree、vision 或 hybrid
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

macOS 支持 Accessibility tree、Vision 和 Hybrid；Windows 使用 UI Automation，配置为 `vision`/`hybrid` 时会安全回退到 UIA。`clickable_roles` 是跨平台语义角色，也可以用 `ax:` 或 `uia:` 指定原生角色。

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

速度单位是像素/秒，加速度单位是像素/秒²，与显示器刷新率无关。`smooth_acceleration = true` 使用起步和收尾更柔和的 S 曲线，同时保持相同的加速时长和总行程；设为 `false` 使用线性加速。释放最后一个方向键后两种模式都会立即停止。主题颜色使用 `#RRGGBBAA`，可以为浅色和深色外观分别设置：

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

鼠标按钮通过 `press` 或 `toggle` 保持按下时，透明圆形指示器使用对应的 `*_pressed_color`：填充使用配置颜色 20% 的不透明度，轮廓使用配置颜色本身。普通 `click`/`double-click` 在物理触发键释放前也使用对应颜色，但点击本身仍在按下沿立即完整执行。真实的 `press`/`toggle` 状态优先；否则显示最近仍按住的点击键颜色。这个提示不使用定时器，也不表示普通点击正在保持鼠标按钮。

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

根级 `[[app_configs]]` 对所有 Mode 生效；`[[normal.app_configs]]` 只在 Normal 生效。匹配值可以是 macOS bundle id、Windows 可执行文件名、Linux WM_CLASS/app_id，或窗口标题的子串。

## 插件设置

插件 Mode 使用命名空间：

```toml
[plugin_modes."plugin:screen-selector".settings]
preserve = true

[plugin_modes."plugin:screen-selector"]
inherits = ["hotkeys", "normal"]
```

自带 Screen Selector 的 `preserve = true` 会在切屏时保留 Grid/Recursive Grid 的选择路径；设为 `false` 则从目标显示器重新开始。

## 运行时修改与调试

`set_config` 可以修改点号路径，并在解析、校验通过后原子写回配置：

```toml
[normal.bindings]
"primary+1" = "set_config pointer.max_speed 800"
"primary+2" = "set_config theme.dark.accent \"#FF8800FF\""
```

无效修改不会替换当前有效配置。状态栏的 Reload Configuration 会重新加载配置，并通知所有 Mode 刷新缓存。

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
