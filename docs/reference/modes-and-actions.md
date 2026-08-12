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

## 移动、滚动和速度

| 动作 | 说明 |
| --- | --- |
| `move_left`、`move_down`、`move_up`、`move_right` | 按住时连续移动；短按也会移动一小段。 |
| `scroll_left`、`scroll_right`、`scroll_up`、`scroll_down` | 按 `[scroll].scroll_step` 滚动。 |
| `scroll_half_*` | 按 `scroll_step_half` 滚动。 |
| `scroll_full_*` | 按 `scroll_step_full` 滚动。 |
| `precision`、`slow`、`fast` | 按住时改变移动速度。 |
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
| `left_click`、`right_click`、`middle_click` | 在物理键按下沿立即注入一次完整点击。 |
| `double_click` | 左键双击。 |
| `left_press`、`right_press` | 按住鼠标按钮。 |
| `left_release`、`right_release` | 松开鼠标按钮。 |
| `toggle_left`、`toggle_right` | 切换对应按钮的按住状态。 |
| `toggle` | 无参数时切换同时按住伙伴的输入状态，伙伴先按或后按均可；单独短按释放所有 latched 输入，单独长按达到阈值后锁定激活键自身。 |
| `press <目标...>` | 按住一个或多个键/鼠标按钮。 |
| `release <目标...>` | 释放之前按住的目标。 |
| `toggle <目标...>` | 切换目标状态。 |

目标可以是键名或 `mouse_left`、`mouse_right`、`mouse_middle`：

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
