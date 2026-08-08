# 快速上手

KeySteer 可以不需要配置文件。启动后它会停留在托盘或菜单栏，并默认处于 `idle`：普通键盘输入不会被拦截。

## 第一次操作

1. 启动 KeySteer。
2. 按 `Primary+E` 进入 Normal。
3. 用 `h/j/k/l` 移动鼠标，用 `;` 左键点击。
4. 按 `g` 进入 Grid，按 `f` 进入 Recursive Grid，按 `Primary+F` 进入 UI Hint。
5. 按 `q` 或 `Esc` 返回 Idle。

`Primary` 是跨平台写法：macOS 为 Command，Windows/Linux 为Alt， 为保持键盘位置的一致性。

它可以在 `[key_aliases]` 中改成你习惯的实体按键；如果使用发布的完整 TOML，请留意其中可能包含平台专属别名覆盖。

## 默认按键

| 按键 | 作用 |
| --- | --- |
| `h j k l` | (vim风格）左、下、上、右移动 |
| `Caps Lock` / `Left Shift` | 精确 / 慢速模式，对移动，滚动生效 |
| `v` 或 `b` | 快速模式，对移动，滚动生效 |
| `m` / `,` | 向下 / 向上滚动 |
| `;` / `'` / `Right Shift` | 左键 / 右键 / 中键点击 |
| `n` | 切换左键按住状态，用于拖拽 |
| `t` / `y` / `i` / `u` | 发送 `Home` / `End` / `Page Up` / `Page Down` |
| `g` / `f` / `Primary+F` | Grid / Recursive Grid / UI Hint |
| `Primary+S` | 切换到下一块显示器 |
| `q` 或 `Esc` | 返回 Idle |

按键不顺手时，不必修改源码：直接编辑 TOML，或打开[配置与模拟器](/editor/)可视化修改。

## 如何定位？

| 目标 | 推荐模式 |
| --- | --- |
| 日常移动、滚动、点击、拖拽 | [Normal](/modes/normal) |
| 快速到达屏幕某个区域 | [Grid](/modes/grid) |
| 精确定位细小目标 | [Recursive Grid](/modes/recursive-grid) |
| 找按钮、链接、菜单或输入框 | [UI Hint](/modes/ui-hint) |

## 状态栏菜单

右键托盘/菜单栏图标可以：

- 暂停或恢复 KeySteer。
- 重载配置。
- 设置或取消开机启动。
- 退出程序。

支持自动发现配置，运行中把 `keysteer.<名称>.toml` 放入数据目录后点击 Reload 即可

也可以命令行参数，使用 `-c --config` 时只会重载指定文件。

## 配置和诊断

```bash
keysteer --check -c keysteer.user.toml
keysteer --doctor
keysteer --dump-config
```

完整默认配置可 [下载](/generated/keysteer.default.toml)，配置语法见 [配置文件](/reference/configuration)，动作参数见 [模式与动作参考](/reference/modes-and-actions)。

## 安装位置

- Windows ：只有portable版本，配置和日志通常在程序旁边。
- macOS ：`.app`：配置和日志在 `~/Library/Application Support/KeySteer/`。

macOS 首次使用还需要授权辅助功能，请阅读 [macOS 指南](/guide/macos)。
