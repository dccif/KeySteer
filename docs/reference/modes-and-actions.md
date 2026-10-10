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

第一次使用？先看 [Window 操作指南](/modes/window)，按场景练习并查阅默认键位。

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
| Ctrl/Cmd+S | 保存当前 Tab 模板及备注 |
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

## 直接切换窗口

`window_activate_next` / `window_activate_previous` 从鼠标下的窗口向后／向前循环，鼠标下无有效窗口时使用当前激活窗口；激活目标后把鼠标移到其中心。
切换列表始终包含全部普通候选窗口（不含最小化窗口）。已有 Tabs 成员按标签顺序相邻排列，未合并窗口按同屏、同程序规则相邻排列；走完同组后继续切换其他程序，不会困在组内。
例如 A1、B1、A2、B2 按 A1 → A2 → B1 → B2 循环。Window 模式保留已有 Tabs 组内切换行为，Normal 的两个动作可以跨出 Tabs 组。
复用 Window 的稳定切换顺序，但不进入 Window 模式，也不显示标题、编号或操作界面。
没有新增默认按键，可自行配置：

```toml
[normal.bindings]
x = "window_activate_next"
c = "window_activate_previous"
```

## 相交窗口切换

`window_overlap_next` / `window_overlap_previous` 复用上述切换逻辑，优先以当前焦点窗口为起点，只选择其所在相交连通组中的窗口（仅边缘接触不算）。相交关系可以传递：A 与 B 重叠、B 与 C 重叠时，A、B、C 属于同一组，即使 A 与 C 不直接重叠。无有效焦点窗口时回退到鼠标下窗口。已有 Tabs 组时优先按标签顺序检查组内候选，再检查全局其他窗口；逐个跳过连通组外候选，不会切换到连通组外窗口。若连通组只有起点窗口，则仅将鼠标移到该窗口中心，不重新激活；无有效起点或没有有效候选时不操作。
每一步都核对最新窗口几何，未变化时复用连通组，变化后重新计算，不固定首次选择的窗口集合；最小化窗口不参与连通。激活成功后鼠标移到中心，不进入 Window 模式、不显示窗口标识。

```toml
[normal.bindings]
x = "window_overlap_next"
c = "window_overlap_previous"
```

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
scan_scope = "window" # window / active / screen
```

`window`（默认）按鼠标下窗口 → 当前激活窗口 → 鼠标所在整屏选择；`active` 按当前激活窗口 → 鼠标下窗口 → 鼠标所在整屏选择；`screen` 直接选择鼠标所在整屏。只有窗口无法取得时才使用下一项，选定后只扫描该范围，不会因结果为空或某个识别源失败而换范围再扫。配置 `window` 且选中鼠标窗口时，会先尝试激活该窗口；激活被系统拒绝不改变扫描范围。这些规则在 Windows／macOS 共用。

适用于 Hybrid、Accessibility Tree 和 Vision。整屏模式覆盖当前显示器上的可见内容，
不会把其他屏幕合并进来。扫描期间或标签显示后，按绑定到 `screen next` 的 `Alt+S` 等组合键切换屏幕、直接移动鼠标到另一屏，
都会清空旧标签并自动重新扫描；也可用 `Primary+R` 手动刷新。

普通 Window 模式默认通过 Alt+W 进入，先激活并前置鼠标下窗口，鼠标位置不变。X 执行 `window_close`，请求关闭当前目标窗口；保存或取消由应用处理。在 `[window.bindings]` 中可改键，例如 `"f9" = "window_close"`（同时移除或禁用原 X 绑定）。编辑布局中的 X 仍然是删除区域。

普通 Window 的目标属于 Tab 组时，X 仅请求关闭该组当前活动窗口；真实关闭后剩余一个成员时自动解散该组并显示剩余窗口。A/E/T 及其他模式保留各自的 X 绑定。


### 当前窗口所属应用音量

Window、Quick（A）、Editor（E）的 Common 支持按住 V 后用 J/K 降低／提高音量，每步 1%；V+M 切换静音。按住方向键可以连续调整，松开 V 即停止。共享音频进程的同应用窗口会一起变化；应用尚无音频会话时显示提示。Windows 和 macOS 共用同一组按键和请求接口。

动作名为 `window_volume_down`、`window_volume_up`、`window_volume_mute`。各模式可以独立改绑；如果已自定义方向键，请同步修改音量组合键。已有显式 `[window.bindings]`、`[window_quick.bindings]`、`[window_editor.bindings]` 表需要各自补入：

```toml
"v+j" = "window_volume_down"
"v+k" = "window_volume_up"
"v+m" = "window_volume_mute"
```


### 应用输出与系统音频快捷键

Window、Quick（A）、Editor（E）共同提供：

| 快捷键 | 功能 |
| --- | --- |
| V+J / V+K | 当前应用音量 − / +，每步 1%，按住连续调节 |
| V+M | 当前应用静音 / 取消静音 |
| V+H / V+L | 当前应用上一个 / 下一个输出设备 |
| Shift+V+J / Shift+V+K | 系统总音量 − / +，每步 1%，按住连续调节 |
| Shift+V+H / Shift+V+L | 系统默认上一个 / 下一个输出设备 |

设备按名称排序循环，按一下切换一个；Common 会显示应用/系统的区别，状态显示目标设备名称。应用输出偏好不更改系统默认设备；有些应用需重新开始播放才会采用新设备。自定义绑定表可添加 `window_audio_previous`、`window_audio_next`、`window_system_volume_down`、`window_system_volume_up`、`window_system_audio_previous`、`window_system_audio_next`。


macOS 的系统音频控制使用 Core Audio；应用独立音量、静音和输出要求 macOS 14.2 或更高版本，并需允许 KeySteer 的“系统音频录制”权限。请使用打包的 KeySteer.app。应用音频仅在内存中处理并重放，不录音保存或上传，但会增加少量播放延迟。旧版本或不支持音量调节的设备会显示原因。应用音频设置在模式退出后继续生效，退出 KeySteer 时恢复原始播放路径。权限要求参见 [Apple 的 Core Audio Tap 说明](https://developer.apple.com/documentation/CoreAudio/capturing-system-audio-with-core-audio-taps)。


Window、Quick、Editor 中，`V+M` 切换当前应用静音，`Shift+V+M` 切换系统静音。按住不会反复切换；系统动作可重绑为 `window_system_volume_mute`。已有自定义绑定表需添加 `"shift+v+m" = "window_system_volume_mute"`。

## 窗口目标来源

Window、Quick、Editor、Restore、Tab 可以分别设置入口目标：

```toml
[window]
target = "active"
[window_quick]
target = "mouse"
[window_editor]
target = "active"
[window_restore]
target = "mouse"
[window_tab]
target = "active"
```

`active` 表示优先激活窗口，无有效窗口时再用鼠标下窗口；`mouse` 表示优先鼠标下窗口，无有效窗口时再用激活窗口。它们只调整查找优先级，首选有效时不会查询另一来源。省略时保持原有行为：新会话从鼠标下开始，Window 子模式切换保留已选目标。显式设置时，每次进入该模式重新选择；需要结束的布局先等待提交完成。

快捷键可以在动作后直接加 `active` 或 `mouse`，同一个动作的不同按键互不影响：

```toml
[normal.bindings]
"alt+j" = "window_overlap_next active"
"alt+k" = "window_overlap_next mouse"
"alt+n" = "window_activate_next active"
"alt+w" = "window mouse"
"alt+d" = "move_window next active"
[window.bindings]
"x" = "window_close active"
"shift+x" = "window_close mouse"
"h" = "window_left mouse"
```

入口快捷键优先于模式的 `target`。模式内没有后缀的动作仍操作已选目标；有后缀的动作在开始时异步选窗，按住移动／缩放期间固定窗口，不随鼠标或焦点漂移。两种来源都没有有效窗口时不执行目标操作。相交切换的首选起点有效但没有重叠同伴时，仍只将鼠标移到该窗口中心，不改用另一来源。查询错误、原生窗口变化或操作／权限拒绝仍可能使操作失败，不会因此改去操作另一窗口。

后缀适用于窗口切换、模式入口、跨屏移动、窗口移动／缩放／状态／关闭／应用音频、布局方向／分割／比例／区域移除以及标签成员操作。布局操作只能选现有布局内的窗口；标签操作只影响选中窗口所在组，不会合并组。撤销／重做／恢复初始状态、预设保存／删除／确认、编号输入及系统音频各有独立作用范围，不接受窗口来源后缀。

## UIHint 搜索与信息复制

按 `/` 打开覆盖层直接绘制的搜索框，无须等待首批标签；支持键盘字符、简拼、全选和 Unicode 文本复制粘贴。目前不支持系统输入法组词候选窗口，中文可粘贴输入。输入标签字符、OCR 文字、辅助功能名称或角色都可以过滤；例如 `复制` / `fz`，以及 `按钮` / `an` / `button`。搜索时 label 编号保持不变，完整 label 也仅过滤。

Enter、`/`、primary+Q 结束搜索：清空查询、恢复本轮全部标签，继续留在 UIHint；唯一结果或空格分隔的多项选择会移动鼠标到当前检查点，未调整时为各标签跳转点的等权平均中心；零个结果或未消歧的单项搜索不移动。Esc 取消搜索并恢复标签，不移动鼠标。

| 默认快捷键 | 固定信息槽 |
| --- | --- |
| Ctrl+1 | OCR 文字 |
| Ctrl+2 | 辅助功能名称与角色 |
| Ctrl+3 | 当前点坐标 |
| Ctrl+4 | 当前点实时颜色 |


缺失内容显示空位，快捷键不重排，也不会清空剪贴板。可通过 `ui_hint.search_copy_keys` 配置四个组合键。搜索框编辑键由 `[ui_hint.search_edit_keys]` 配置，例如 `"primary+v" = "paste"`、`"primary+c" = "copy"`、`"primary+a" = "select_all"`；`primary` 遵循 `[key_aliases]`（发布配置在 Windows 为左 Alt，macOS 为 Cmd），也可显式指定 `cmd` 或 `ctrl`。省略的编辑动作继承默认键。复制输入选区保留搜索，复制信息槽成功后退出搜索。

搜索唯一结果或空格分隔的多项选择自动显示点位并开始取色，无需确认；普通标签、无选择及未消歧的单项搜索不取色。单击 Ctrl 切换输入／调整状态；调整时隐藏文字光标和选区，统一使用绿色背景与右侧小圆点提示，不显示 Point 文字。输入保持左对齐和相同垂直位置，过长时按实际字宽滚动显示末尾。移动复用 Normal 的方向键和速度设置。点击等操作先确认当前点、退出搜索，再执行原操作。`Ctrl+Shift+4` 自动进入调整并循环 HEX → RGB → HSL，`Shift+4` 仍可输入 `$`。四个复制键在调整时可直接使用，复制成功后自动确认当前点、移动鼠标并退出搜索；失败保留调整状态。点位检查期间隐藏普通鼠标圆环，只显示当前检查点。单个点与直接输入该标签时的实际鼠标跳转位置一致，不受标签字体、尺寸和显示偏移影响。

```toml
[ui_hint.search_bindings]
ctrl = "point_toggle"
tab = "point_next" # 可换成其他按键或组合键
"ctrl+shift+4" = "color_next"
```

此表按“按键 = 动作”配置，显式表替换默认绑定，空表禁用这些操作。`point_toggle` 使用单键轻触，`point_next` 和 `color_next` 可使用组合键。多点初始为集合中心，序号显示 `0/N`。N 是当前输入中已确定、去重后的有效标签数：完整标签代码优先，文字／简拼仅在唯一命中时计入；未完成、无效、有歧义或重复的项不会增加总数；调整时默认 Tab 切到第一个标签的初始跳转点，然后依次切换，末项后回到首项，序号为 `1/N` 至 `N/N`。点位样式沿用内置值，按需在网页或 `[ui_hint.search_point]` 覆盖 `marker_color`、`marker_radius`、`marker_width`；`color_formats` 指定不重复的格式顺序，首项为默认，例如 `["rgb", "hex", "hsl"]`。颜色来自后台屏幕像素取样，并排除 KeySteer 自己的覆盖层；失败时保留空位，不复制旧颜色。macOS 需要屏幕录制权限。

多点的四栏展示与复制可用数组分别配置，顺序为 OCR、辅助功能、坐标、颜色；`concat` 按选择顺序换行拼接所有标签初始点的信息，`switch` 跟随当前检查点，Tab 切换或方向移动会同步更新。默认第 1、2 栏拼接，第 3、4 栏切换。单点继续按当前检查点命中的扫描元素显示文字，空白处只保留坐标与颜色。字段枚举、快捷键和样式在加载配置时编译，运行时不解析配置；颜色拼接仅在启用后逐点异步采样，保持一个在途请求。网页 **UI Hint → 行为** 可修改数组和切换键。

```toml
[ui_hint.search_point]
field_modes = ["concat", "concat", "switch", "switch"] # 内置默认值，省略即可
```

复制使用面板显示的颜色及格式；当前点尚未完成取样时等待该次取样，颜色拼接则等待各标签样本完成。成功后复制、清空查询并退出搜索。单点或颜色切换时，`Color` 标题右侧显示当前颜色的方形色块，与标题行垂直居中；多点颜色拼接时隐藏单个色块。默认样式不写入默认文件或原生导出；网页的 UI Hint → 外观可编辑并预览，重置后恢复内置值。

```toml
[ui_hint]
search_copy_keys = ["ctrl+1", "ctrl+2", "ctrl+3", "ctrl+4"]

# 可选覆盖，下面展示内置默认值（逻辑像素）
[ui_hint.search_point.color_preview]
enabled = true
width = 16         # 1–64
height = 16        # 1–64；与 width 相同即为正方形
x_offset = 4       # -200–200，相对标题末尾，正值向右
y_offset = 0       # -200–200，相对标题行中心，正值向下
border_width = 1   # 0–10，0 不画边框
```

Point 状态的背景和边框均可在网页 **UI Hint → 外观** 单独编辑，支持浅／深主题、透明度和重置；普通搜索框样式也在该页。默认背景为浅色 `#E8F6F0FF`／深色 `#284D44FF`，与窗口多选卡片一致；边框沿用 `search_input_ui.border_color`。输入与调整状态共用文字垂直位置，并保持左对齐。以下为可选覆盖，默认文件不写出：

```toml
[ui_hint.search_point]
input_border_color = { light = "#16856BFF", dark = "#68D9B1FF" }
# 可选覆盖默认绿色背景
# input_background_color = { light = "#E5F3ECF2", dark = "#16382FF2" }
```

```toml
[ui_hint.search_input_ui]
position_mode = "screen" # 或 window
position = ["100%", "50%", "0%", "50%"] # 上、右、下、左
width = 420
font_family = "Microsoft YaHei UI"
font_size = 16
padding_x = 12
padding_y = 6
border_radius = 8
border_width = 1
background_color = "#0A1338F2"
text_color = "#E8EEFFFF"
border_color = "#6E82D6FF"
y_offset = 24

[ui_hint.search_info_ui]
position_mode = "search_input" # 也支持 screen / window
position = ["0%", "50%", "100%", "50%"]
width = 520
y_offset = 12
font_size = 14
```

两个面板共用字体、颜色、边框、圆角和内边距设置方式；百分比位置与 Window 卡片共用解析。搜索索引提前准备并在同一轮 UIHint 中复用，最终退出或重新扫描后才释放。

搜索面板的百分比、浅深主题样式、信息标题、四栏行为枚举和快捷键在加载阶段编译。搜索过程中复用已编译结果；目标文本及中文简拼索引随扫描提前准备。成员集合只在查询或扫描几何改变时更新，帧处理和取样回调不重建集合。

默认复制快捷键在 Windows 和 macOS 均为 `ctrl+1` 至 `ctrl+4`。`search_copy_keys` 数组可为四个条目分别指定任意有效且不冲突的组合键；显式使用 `primary` 时遵循配置别名。面板只显示编号键帽与类别标题，不重复提示快捷键。默认搜索框宽度为 280，上下内边距 `padding_y = 6`。

空格是多选搜索分隔符：`fz button ab` 分别搜索并按输入顺序合并，重复目标只保留第一次。连续空格不添加空项；尾部空格显示完整候选，便于开始下一项。修改或删除任何项都会更新结果。多项选择显示四栏，默认 OCR／辅助功能按选择顺序拼接，坐标／颜色跟随当前检查点，可用 `field_modes` 修改。复制成功关闭搜索并保留 UIHint 和全部标签；失败或空字段保留搜索。长文字仅在显示时省略，复制保留原文。

结束搜索键默认 `"enter / primary+q" = "accept"`，`Enter`、`/`、`Primary+Q` 都能确认当前预览并移动鼠标。所有编辑绑定统一为“按键 = 动作”；左侧用空格分隔多个快捷键，也可用多行绑定同一动作。配置某个动作会替换该动作的全部默认键，省略的动作保留默认键。旧“动作 = 按键”格式仍可读取，导出使用新格式；启动时完成解析与冲突校验。

输入 `@la` 只精确匹配标签 `la`；`la` 仍混合匹配标签前缀、文字与简拼。`@la @ka` 选择两个标签，`@la 按钮` 可混用精确标签和普通搜索词，结果仍按输入项顺序合并去重。

单独输入 `@` 保留候选标签显示，不代表全选；输入 `@l` 按标签前缀缩小范围，完整 `@la` 对应唯一标签。空格后的 `@` 同样显示下一项候选，已选项及复制顺序保持不变。

`@` 也可后置：`ld@` 与 `@ld` 等价，只匹配标签。普通搜索词后追加 `@` 即可切换到标签匹配，删除标记则恢复混合搜索；多选可混用，如 `ld@ @ka`。

Window Move 的定位入口由 `[window.bindings]` 独立配置，不再自动复制 Normal 的 grid / recursive_grid 入口。默认如下，可改键或设为 `"none"` 禁用；Normal 的改键和应用覆盖不会改变 window 自有绑定；按住 primary 后仍保留原有临时层优先规则。已有显式 `[window.bindings]` 表请按需补入。

```toml
[window.bindings]
g = "grid"
"primary+f" = "recursive_grid"
```

`primary+g` 不在 Window 默认绑定中；按住 primary 后，是否由 g 进入 Grid 取决于临时层的配置。其他 Window 子模式不增加这两个默认定位入口。
