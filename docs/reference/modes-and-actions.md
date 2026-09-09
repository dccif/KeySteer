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
| `window_quick` | Quick 比例布局。 |
| `window_editor` | 自动布局与分区编辑。 |
| `window_restore` | 恢复保存布局。 |
| `window_delete` | 确认删除保存布局。 |
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

窗口操作由五个独立模式组成。每个模式使用自己的绑定、继承、应用覆盖、临时模式和样式；方向键默认显式配置 H/J/K/L，不随 Normal 改键。

| 模式 | 默认入口 | 默认 Q 目标 |
| --- | --- | --- |
| `window` | 全局 Alt+W | `idle` |
| `window_quick` | Window 的 A | `window` |
| `window_editor` | Window 的 E | `window` |
| `window_restore` | Window 的 R | `window` |
| `window_delete` | Restore 的 X | `window_restore` |

Q 是普通模式绑定，目的地与进入路径无关。例如从 Idle 直接进入 Editor，默认 Q 仍进入 Window。可把任意模式的 Q 改为其他模式；再次激活当前模式沿用通用切换到 Idle 的规则。

Alt+W 锁定鼠标下的窗口。当前屏幕普通窗口显示稳定编号，关闭窗口不重排其余编号。完整编号立即激活并锁定目标，鼠标移到中心；仅歧义前缀等待，默认 250ms。Tab 循环切窗；S 切换移动／中心缩放，H/J/K/L 调整，D 循环切屏，F 循环最大化→最小化→还原，C 居中，Z 撤销。

Quick 使用 H/J/K/L 调整各轴比例，首次取最接近半屏的配置比例，同方向缩小、反方向扩大，另一轴独立控制。`window_quick.split_ratios` 默认包含 1/4、1/3、1/2、2/3、3/4，满屏比例自动加入。Tab 切换目标并重置比例，Z 撤销。

Editor 进入时自动布局。H/J/K/L 选择区域，Shift＋方向切分，Ctrl＋方向按独立的 `resize_step`／`resize_speed` 移动分割线，X 删除区域，Z 撤销。输入窗口编号再输入另一个窗口编号交换位置；反引号后输入区域编号可选择或移入空区域。所有调整即时生效，切换模式前完成待提交事务；窗口模式之间保留目标和编号。

Editor 的 Ctrl+S 在底部输入备注并保存布局，备注最多 80 字符，可留空。Restore 输入编号恢复，PageUp/PageDown 翻页；仅原生应用成功后执行 `window_restore.lifecycle.after_finish`，默认进入 Editor，失败停留列表显示错误。保存布局只包含几何、窗口数量和备注，不包含应用名称或窗口身份；恢复按最近活动顺序填入区域，多余窗口保持原位，不足时保留空区域。

Restore 中 X 进入 Delete。输入编号后面板显示名称，Enter 才删除；Q 返回 Restore 并取消待确认记录。删除后留在 Delete，列表刷新且其他编号不变，最后一条删除后保存有效空库。提交前重新读取记录，外部修改会要求重新选择，写入失败保留原文件。

布局文件 `window-layouts.kslayout` 与应用日志使用相同目录规则。格式保持不变，可与[网页模拟器](/simulator)互导；网页修改只影响示例窗口和浏览器布局库，下载文件后可替换程序的同名文件，再进入 Restore 读取。

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

默认 `f = "size_cycle"`。最小化后继续按 F 可恢复同一窗口的原位置和尺寸，再按 F 重新开始循环。
