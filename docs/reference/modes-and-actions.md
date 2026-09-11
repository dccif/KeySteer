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

Window、Quick、Editor、Restore 和 Tabs 各自增加两项独立配置：

```toml
[window]
screens = "current"       # "current" 当前屏幕；"all" 全部屏幕
include_minimized = false # true 时也包含最小化窗口
```

`[window_quick]`、`[window_editor]`、`[window_restore]` 和 `[window_tab]` 使用相同配置项。默认只选择当前屏幕的非最小化窗口；全部屏幕时，编号覆盖所有屏幕，自动布局、自动分组仍在各屏幕内分别执行，不跨屏搬动。开启 `include_minimized` 后，最小化窗口也可编号选择和参与布局。退出后重新进入会根据当前候选从 1 编号，释放关闭、最小化或范围外窗口占用的旧编号；会话内刷新保持存活编号稳定。窗口身份卡片会相互避让，并避开下方帮助面板。

窗口操作由五个独立模式组成。每个模式使用自己的绑定、继承、应用覆盖、临时模式和样式；方向键默认显式配置 H/J/K/L，不随 Normal 改键。

| 模式 | 默认入口 | 默认 Q 目标 |
| --- | --- | --- |
| `window` | 全局 Alt+W | `idle` |
| `window_quick` | Window 的 A | `window` |
| `window_editor` | Window 的 E | `window` |
| `window_restore` | Window 的 R | `window` |
| `window_tab` | Window 的 T | `window` |

Q 是普通模式绑定，目的地与进入路径无关。例如从 Idle 直接进入 Editor，默认 Q 仍进入 Window。可把任意模式的 Q 改为其他模式；再次激活当前模式沿用通用切换到 Idle 的规则。

Alt+W 锁定鼠标下的窗口。默认当前屏幕的非最小化窗口显示稳定编号，关闭窗口不重排其余编号。完整编号立即激活并锁定目标，鼠标移到中心；仅歧义前缀等待，默认 250ms。Tab / Shift+Tab 在当前标签组内向前／向后循环，组外使用普通窗口循环；输入编号可切换到组外窗口；S 切换移动／中心缩放，H/J/K/L 调整，D 循环切屏，F 循环最大化→最小化→还原，C 居中，Z 单步撤销，Shift+Z 重做，Shift+C 恢复本次窗口会话的原始状态。

Quick 使用 H/J/K/L 调整各轴比例，首次取最接近半屏的配置比例，同方向缩小、反方向扩大，另一轴独立控制。`window_quick.split_ratios` 默认包含 1/4、1/3、1/2、2/3、3/4，满屏比例自动加入。Tab 切换目标并重置比例，Z 撤销，Shift+Z 重做，Shift+C 恢复原始状态。

Editor 进入时自动布局。H/J/K/L 选择区域，Shift＋方向切分，默认 Ctrl+H/L 缩小／增大选中区域宽度、Ctrl+K/J 缩小／增大高度，步长与速度取独立的 `resize_step`／`resize_speed`，联动相邻区域并保持平铺；这些键位均可在配置中重绑。X 删除区域，Z 撤销。输入窗口编号再输入另一个窗口编号交换位置；反引号后输入区域编号可选择或移入空区域。所有调整即时生效，切换模式前完成待提交事务；窗口模式之间保留目标和编号。新分割复用最小空闲区域号，已有区域不重新编号。

Editor 的 Ctrl+S 在底部输入备注并保存布局，备注最多 80 字符，可留空。Restore 输入编号恢复，PageUp/PageDown 翻页；仅原生应用成功后执行 `window_restore.lifecycle.after_finish`，默认进入 Editor，失败停留列表显示错误。保存布局只包含几何、窗口数量和备注，不包含应用名称或窗口身份；恢复按最近活动顺序填入区域，多余窗口保持原位，不足时保留空区域。

Restore 中 X 在恢复／删除之间切换，保留当前页并取消待确认记录。删除状态输入编号后面板显示名称，Enter 才删除；再次 X 回到恢复，Q 按 Restore 的绑定返回 Window。删除后留在删除状态，列表刷新且其他编号不变，最后一条删除后保存有效空库。提交前重新读取记录，外部修改会要求重新选择，写入失败保留原文件。

布局与 Tabs 模板共同保存在 `workspace.ksw`，与应用日志使用相同目录规则。默认名称为 `Layout N` / `Tabs N`，填写备注后直接使用备注；保存、恢复和删除共用同一个预设列表。可与[网页模拟器](/simulator)互导；网页修改只影响示例窗口和浏览器存储，下载后替换程序的同名文件，再进入 Restore 读取。

### T：持续存在的标签组合

首次窗口编号尽量让同一程序连续排列；后续保留已有号码，避免编号跳变。点击组内活动窗口的最小化按钮会收起整组；恢复时显示选中的成员，其余成员继续保持收起。

标签栏会占用组布局顶部的空间，最大化和编辑分屏都会计入这部分高度，不遮挡应用底部。Alt+W 的组卡片列出所有成员的编号、程序与标题，并标记当前成员。Windows 标签较多时可以用纵向滚轮或横向滚动浏览；切换成员会自动滚到当前标签。

Windows 上，退出 Window 模式后也能直接拖动标签：在同一栏调整顺序，拖到另一组标签栏转移窗口；拖动左侧 `~组编号` 可把整组并入目标组。插入线表示落点，移到栏外松开或右键取消。标签栏随所属窗口的前后层级显示。切换时尽量保留应用内容，减少重新显示的闪烁；已有特殊透明绘制的应用使用兼容显隐方式。

Windows 和 macOS 使用同一套分组方式。Alt+W 后按 T，在配置选定的各屏幕内分别组合同一应用的未分组窗口，保留已有组。整理后可用 Z 一步撤销。每组只显示活动成员，独立浮动标签栏可用鼠标选窗；退出模式后分组保留，直到解散或退出 KeySteer。

窗口编号始终表示具体窗口，标签组使用 `~1`、`~2`。输入第一项设定起点，第二项立即组合，之后继续加入；起点已经在组内时使用该组。T 结束本轮并准备下一组，不需要 Enter，也不新增撤销记录。

新组使用最小空闲组编号，已有组不重新编号；全部解散后再建组从 `~1` 开始。切换到组外应用时，标签栏仍保持显示。Windows 的标签栏随窗口位置事件更新，不按固定间隔或显示帧率轮询。

| 输入 | 结果 |
| --- | --- |
| `12t34t` | 窗口 1、2 一组，3、4 另一组（这些编号没有多位歧义时） |
| `123t` | 窗口 1、2、3 一组 |
| `~1` | 选择组 1 |
| `~1 4t` | 将窗口 4 加入组 1 |
| `~1 ~2t` | 整组 2 并入组 1，保留组 1 的编号 |
| `1 空格 2 t` / `12 空格 t` | 明确选择窗口 1、2 / 窗口 12 |

`~` 立即进入组编号输入并突出组编号，完成后返回窗口编号输入。多位编号仍遵循当前可用编号的歧义解析规则；空格可以明确结束编号，T 先处理有效待完成编号再结束本轮。无效编号显示原因并保留当前目标。选择一个窗口只转移该成员，选择 `~组号` 才会合并整组；选择目标组已有成员只切换标签。

| 按键 | 操作 |
| --- | --- |
| T | 结束本轮；只有一个独立窗口或空选择时，仅清除选择 |
| D / X | 移出活动成员并继续以原组为目标 / 解散当前组并结束本轮 |
| Tab / Shift+Tab | 下一个 / 上一个标签 |
| H / L (`move_left` / `move_right`) | 调整活动标签顺序 |
| K / J (`move_up` / `move_down`) | 上一个 / 下一个标签 |
| Z / Shift+Z | 撤销 / 重做组合、移出、解散和排序 |
| Ctrl+S | 保存当前 Tab 模板及备注 |
| Q / Esc | 返回 Window / 退出，保留已完成组合 |

这些操作都是 `[window_tab.bindings]` 中可修改的绑定，包括 `window_tab_group`（组编号前缀）和 `window_number_end`（编号分隔符）。分组编辑操作在 T 内生效。Window 模式沿用数字直选、Tab 下一个和 Shift+Tab 上一个，当前在组内时优先按标签顺序循环，数字仍可直选任意窗口；前后切换可在 `[window.bindings]` 配置为 `window_select` / `window_select_previous`。退出 Window 后不接管应用的 Tab 或数字；标签栏仍可用鼠标切换。

应用始终是独立窗口，每组只显示活动成员。拖动、缩放只移动当前窗口，切换时才把新成员对齐到当前组的位置并显示。Window、Quick 和 Editor 布局将每组当成一个目标。关闭标签栏只解散分组并恢复成员可见；关闭应用窗口只移除该成员，并显示相邻成员，剩一个时自动解散。普通窗口历史恢复组的几何，T 独立记录成员和顺序历史。

Tab 模板与布局共用 Restore/Delete 列表，名称标注 Tabs。模板保存成员数量、选择顺序、活动标签位置、组合区域及备注，不保存窗口身份。选择模板后进入 T，依次输入所需窗口；选满后自动应用。选择完成前不改动窗口，Q/Esc 可以取消。删除模板不影响正在运行的组合。

支持当前桌面的普通可缩放窗口，原生全屏或不兼容窗口会提示不支持。Windows 直接隐藏非活动成员，不改变应用父级或嵌入样式；macOS 通过辅助功能逐窗口最小化，因此可能出现系统最小化／恢复动画。解散或正常退出 KeySteer 会恢复本程序收起的窗口。两端都可以保存和恢复 Tab 模板。

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

窗口历史动作均可在对应模式的原有 `bindings` 表中修改，保留其他绑定：

| 默认按键 | 动作 | 说明 |
| --- | --- | --- |
| Z | `window_undo` | 撤销一步；一次连续移动/缩放按一组处理 |
| Shift+Z | `window_redo` | 重做刚撤销的步骤；新的调整会清空重做记录 |
| Shift+C | `window_reset_initial` | 恢复本次会话修改过的窗口的位置、尺寸和最大化/最小化状态 |
| C（Window） | `window_center` | 居中当前窗口 |

前三项在 Window、Quick、Editor 中默认可用。五个窗口模式之间切换保留会话基准；退出到 Idle 等组外模式后重新进入，会建立新的基准。重置本身可用 Z 撤销、Shift+Z 重做；不会重开已关闭窗口、恢复应用内容或改动本会话未调整的窗口。普通模式保存最近 32 组历史，编辑内逐步撤销，离开编辑后整轮作为一组；原始状态记录不受 32 组历史限制。

## UI Hint 扫描范围

```toml
[ui_hint]
scan_scope = "screen" # window：鼠标下窗口（默认）；screen：鼠标所在整屏
```

适用于 Hybrid、Accessibility Tree 和 Vision。整屏模式覆盖当前显示器上的可见内容，
不会把其他屏幕合并进来。扫描期间或标签显示后，按绑定到 `screen next` 的 `Alt+S` 等组合键切换屏幕、直接移动鼠标到另一屏，
都会清空旧标签并自动重新扫描；也可用 `Primary+R` 手动刷新。

普通 Window 模式默认通过 Alt+W 进入，先激活并前置鼠标下窗口，鼠标位置不变。X 执行 `window_close`，请求关闭当前目标窗口；保存或取消由应用处理。在 `[window.bindings]` 中可改键，例如 `"f9" = "window_close"`（同时移除或禁用原 X 绑定）。编辑布局中的 X 仍然是删除区域。

普通 Window 的目标属于 Tab 组时，X 仅请求关闭该组当前活动窗口；真实关闭后剩余一个成员时自动解散该组并显示剩余窗口。A/E/T 及其他模式保留各自的 X 绑定。
