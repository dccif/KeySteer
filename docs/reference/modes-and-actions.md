# 模式与动作参考

配置中的每个绑定都写成：

```toml
按键 = "动作"
```

也可以写成动作数组：

```toml
按键 = ["动作 1", "动作 2", "动作 3"]
```

数组按书写顺序执行；空数组不合法。数组不会阻塞输入线程，`wait` 只会暂停当前序列。

## 模式名

| 值 | 作用 |
| --- | --- |
| `idle` | 待机，只监听 `[hotkeys]`，不拦截普通输入。 |
| `normal` | 直接移动、滚动、点击，并进入定位模式。 |
| `grid` | 全屏坐标网格。 |
| `recursive_grid` | 在当前区域中逐层细分。 |
| `ui_hint` | 给可交互元素显示标签。 |
| `window` | 锁定鼠标下的窗口，移动、中心缩放、布局和平铺。 |
| `plugin:<id>` | 插件模式，例如 `plugin:screen-selector`。 |

```toml
[normal.bindings]
g = "grid"
f = "recursive_grid"
"primary+f" = "ui_hint"
"primary+s" = "screen next"

# 可选：向当前应用发送方向键
# "primary+h" = "left"
# "primary+j" = "down"
# "primary+k" = "up"
# "primary+l" = "right"
```

## Window 模式

`Alt+W` 锁定鼠标下的普通窗口。当前屏幕未最小化的普通窗口（包括被遮挡的窗口）中心显示稳定编号 `1–9、10、11……`；关闭窗口不会重排其余编号。输入完整编号会激活并锁定窗口，鼠标移到中心。只有存在更长有效编号的前缀才等待，默认 250ms：23 个窗口时 `1、2` 等待下一位，`3–9` 立即生效；输入 `12` 立即选中 12，不会先激活 1。编号完整或歧义计时结束后自动执行，无需 Enter。

| 默认键 | 动作 | 行为 |
| --- | --- | --- |
| `← ↓ ↑ →` | `window_left/down/up/right` | 普通 Window 中短按移动一步，按住连续移动。 |
| `S` | `window_size` | 切换移动／中心缩放。 |
| `A` | `window_layout` | 立即进入快速布局；入口后 300ms 内再次独立按 A，且没有其他操作，执行一次 AA 平铺。 |
| `E` | `window_edit` | 从快速布局进入布局树编辑。 |
| `D` | `window_screen_next` | 普通 Window 中循环移到下一屏，鼠标跟随。 |
| `F` / `C` | `window_maximize` / `window_center` | 最大化／还原，或居中。 |
| `Tab` | `window_select` | 立即切换并锁定下一窗口，鼠标居中；树编辑中只循环本屏。 |
| `Z` | `window_undo` | 编辑中撤销一步；返回后整轮编辑是普通 Window 的一步撤销。 |
| `Esc` | `window_cancel` | 保留已生效布局并返回普通 Window；普通 Window 中退出。 |
| `Q` / `Primary+Q` | `window_exit` | 退出并保留已生效布局。 |
| 无默认键 | `window_tile` | 直接执行一次平铺。 |

快速布局使用当前生效的 Normal `move_left/right/up/down` 绑定（默认 H/L/K/J），包含继承、应用覆盖和别名。方向语义优先于普通 Window 的同键动作。每轴第一次按方向得到对应半屏，同锚点方向缩小、反方向扩大，比例依次为 `1/4、1/3、1/2、2/3、3/4、1`，到边界停止；另一轴独立控制，例如先左再上得到左上四分之一。窗口实时改变，编号或 Tab 切换目标时保存已完成调整并重置比例。若 A 同时绑定向左，仅入口双击期限内优先 AA；E 的入口冲突会在配置检查中提示。

E 根据进入时目标屏幕上的现有位置生成 BSP 布局树，优先利用空隙并保留相对位置。方向导航区域，Shift+方向二分当前区域，在该方向创建空区域；Ctrl+方向移动最近的同轴祖先分割线，联动两侧后代，按 `[window].split_ratios` 的分数档位递进，默认 `["1/4", "1/3", "1/2", "2/3", "3/4"]`。例如 `split_ratios = ["1/5", "2/5", "3/5", "4/5"]`。数组可乱序并混用分数与小数，例如 `["3/4", 0.3, "1/2", 0.4]`；加载时自动排序、去重。数组须非空，每项大于 0 且小于 1；到端点后停止。应用最小尺寸限制分割位置；无法容纳时拒绝编辑并提示。

树内第一个完整窗口编号选定交换源，第二个完整编号交换窗口，同号取消。区域标签使用反引号和数字：先点按反引号，再输入区域编号；有源窗口时移入该区域，已占用则交换，没有源时只选择区域。非编号编辑动作清除交换源。Esc 先取消未完成编号或待交换状态，再按一次返回普通 Window，保留已生效布局。窗口关闭只清空区域；固定尺寸和原生全屏窗口仍可编号选择，但不加入树，面板会说明。

单个提示面板锚定在锁定窗口下方，显示实际绑定、比例、区域和交换状态，空间不足时移入工作区。按住 `[window].temporary_mode_keys`（默认 Primary）临时使用 Normal，隐藏覆盖层并保留编辑；松开后继续。Window 默认只继承 hotkeys。

AA 一次整理目标屏幕当前桌面上的普通可缩放窗口，不自动重排新窗口。它按数量与工作区比例选择分区，并根据最小尺寸调整；仍无法无重叠容纳时尽量排列并提示限制。连续移动手势、AA 或整轮编辑各占一个撤销步骤，会话最多保留 32 步。原有 `move_window previous/next/<编号>` 独立于 Window 会话。旧 `layout_keys` 仅兼容解析，不控制编号或布局方向。

## 移动、滚动和速度

| 动作 | 说明 |
| --- | --- |
| `move_left`、`move_down`、`move_up`、`move_right` | 按住时连续移动；短按也会移动一小段。 |
| `scroll_left`、`scroll_right`、`scroll_up`、`scroll_down` | 按 `[scroll].scroll_step` 滚动。 |
| `scroll_half_*` | 按 `scroll_step_half` 滚动。 |
| `scroll_full_*` | 按 `scroll_step_full` 滚动。 |
| `precision`、`slow`、`fast` | 按住时改变移动速度。 |
| `precision_toggle`、`slow_toggle`、`fast_toggle` | 按一下锁存速度，再按同一键关闭；状态显示在模式指示器下方。 |
| `follow` | 切换 Grid/Recursive Grid 的鼠标跟随。 |

`wheel_*` 是 `scroll_*` 的兼容别名。速度动作通常和方向键一起使用：

```toml
[normal.bindings]
h = "move_left"
"v b" = "fast"
```

`"v b"` 表示两个独立按键绑定到同一动作，不是按键序列；组合键使用 `+`。

## 鼠标按钮与拖拽

| 动作 | 说明 |
| --- | --- |
| `left_click`、`right_click`、`middle_click`, `mouse_x1`, `mouse_x2` | 启用长按判定时按下沿立即 MouseDown、松开沿立即 MouseUp；达到阈值只锁定现有按压。设为 0 时在按下沿注入原子点击。 |
| `double_click` | 同样立即开始第一次左键按压；短按松开时完成双击，长按只锁定左键。 |
| `left_press`、`right_press` | 按住鼠标按钮。 |
| `left_release`、`right_release` | 松开鼠标按钮。 |
| `toggle_left`、`toggle_right` | 切换对应按钮的按住状态。 |
| `toggle` | 无参数时按 Normal 绑定锁定伙伴的实际键盘或鼠标目标，伙伴先按或后按均可；单独短按、返回 Normal 或进入 Idle 会释放全部目标，单独长按达到阈值后锁定激活键自身。 |
| `press <目标...>` | 按住一个或多个键/鼠标按钮。 |
| `release <目标...>` | 释放之前按住的目标。 |
| `toggle <目标...>` | 切换目标状态。 |

目标可以是键名或 `mouse_left`、`mouse_right`、`mouse_middle`, `mouse_x1`, `mouse_x2`：

```toml
[normal.bindings]
n = "toggle"
x = ["press shift", "left_click", "release shift"]
```

## 发送按键

裸键名会发送给当前聚焦应用；组合键可以直接使用 `+`，也可以使用更明确的 `send`：

```toml
[normal.bindings]
t = "home"
"primary+shift+s" = "send primary+shift+s"
```

`send` 后面必须是一个合法键或组合键。它不会切换 KeySteer 的模式，只把合成按键注入当前应用。

## 执行外部命令：`exec`

`exec` 适合把 KeySteer 接到脚本、启动器或其他桌面工具。它只负责启动，不等待程序完成，也不把命令输出显示在 KeySteer 界面中。

```toml
[normal.bindings]
"primary+shift+t" = "exec open -a Terminal"
"primary+shift+b" = ["exec say build-started", "wait 500", "exec open ."]
```

语法是：

```text
exec <program> [arg1] [arg2] ...
```

第一个词是程序名，后续每个词都是一个独立参数。KeySteer 使用 Rust 的进程 API 直接启动程序，不经过 shell；因此不会自动展开 `~`、环境变量、管道、重定向或 `&&`。

需要 shell 语法时，建议把逻辑放进一个独立脚本，再直接执行脚本文件；Windows 也可以显式执行 `cmd`：

```toml
# 直接执行不含空格的脚本或程序
x = "exec /usr/local/bin/keysteer-script"

# Windows：参数按空格拆分
x = "exec cmd /C start notepad"
```

配置值按空格切分，不提供引号转义语法；包含空格的路径或复杂参数建议使用脚本或不含空格的包装程序。命令以 detached 方式启动，KeySteer 不等待退出码，也不会把 stdout/stderr 显示到文档或界面。程序不存在或无法启动时会记录错误。

## 插件动词与参数

插件可以在 Manifest 中注册动词。带参数时可以直接写：

```toml
[normal.bindings]
"primary+s" = "screen next"
"primary+1" = "screen 1"
"primary+shift+s" = "call screen"
```

- `screen next`：调用插件动词 `screen`，参数是 `next`。
- `screen 1`：调用同一个动词，参数是 `1`。
- `call screen`：显式调用无参数动词。

显式 `call` 适合无参数调用或避免和其他绑定语义混淆。未知的小写动词加参数会作为插件调用；拼写错误的内置动作会在加载时失败，而不是静默发送按键。

## 跨屏移动窗口

内置 Window Mover 插件提供 `move_window next`、`move_window previous`（或 `prev`）和
`move_window 2` 等显示器编号。移动对象是执行时鼠标下的应用窗口，鼠标随窗口一起移动，
保持在窗口内的相对位置；前台焦点及当前模式保持不变。编号顺序与 `screen` 插件一致；
只有一个显示器或没有可移动窗口时不操作。

相同屏幕尺寸保持原偏移与窗口大小，即使任务栏布局不同；不同尺寸按可移动空间比例保持位置，尽量放入目标工作区。
Windows 最大化窗口直接跨屏，全程保持最大化，还原位置也一起迁移；尺寸变化时按比例保留鼠标位置，
超大或部分位于屏幕外的窗口会将鼠标限制在目标屏幕内。macOS 原生全屏窗口会自动退出全屏、跨屏、
再恢复全屏，保留系统过渡动画，完成后鼠标跟随。应用自身可能限制窗口移动、全屏切换或 DPI 下的大小。
默认键是 `Primary+S+D → move_window next`；松开 `Primary+S` 仍只切换鼠标所在显示器。可在普通
绑定表任意改绑。

## 其他动作

| 动作 | 参数和作用 |
| --- | --- |
| `move_mouse <x> <y>` | 移动到绝对桌面坐标；需要两个整数。 |
| `wait` 或 `wait 0` | 等待默认 `100ms`。 |
| `wait <max_ms>` | 在 `0` 到上限之间随机等待。 |
| `wait <min_ms> <max_ms>` | 在范围内随机等待；最大为 `86400000ms`。 |
| `finish` | 完成当前定位会话。 |
| `restart_mode` | 清空当前定位会话并重新开始。 |
| `rescan` | 重新扫描 UI Hint。 |
| `escape` | 离开当前模式并返回 Idle。 |
| `reload_config` | 从磁盘重新加载配置。 |
| `set_config <path> <TOML值>` | 修改点号路径并持久化，例如 `set_config pointer.max_speed 800`。 |
| `quit` | 退出程序。 |
| `none` | 禁用该绑定，常用于屏蔽继承的按键。 |

`set_config` 的值必须是合法 TOML 值；字符串要带引号，数组和表也可以直接传入。它只修改当前加载的配置文件；若本次是纯内置默认值启动，请先用 `--config` 指定一个文件：

```toml
[normal.bindings]
"primary+1" = "set_config pointer.max_speed 800"
"primary+2" = "set_config general.excluded_apps [\"com.example.App\"]"
```

修改会先解析和校验，成功后才写入配置；失败时保留最后一份有效配置。

## 绑定解析顺序

右值的解析顺序是：

1. `none` / `__disabled__`。
2. 显式动作：`call`、`send`、`exec`、`move_mouse`、`set_config`、`press`、`release`、`toggle`、`wait`。
3. 内置动作，如 `move_left`、`left_click`、`fast`、`finish`。
4. 带参数的插件动词。
5. `+` 组合键或已知裸键，作为发送给当前应用的按键。
6. 内置模式名或命名空间插件模式名。

完整默认示例见 [默认配置文件](/generated/keysteer.default.toml)。

## `key_help`

切换实时按键提示，不重启当前模式或清除筛选状态。在 Normal 绑定表中写入 `"?" = "key_help"` 启用，省略或注释即禁用，targeting 模式可继承。样式由 `[key_help]` 配置。
