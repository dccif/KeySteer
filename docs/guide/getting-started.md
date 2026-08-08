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
- 手动检查 GitHub Release 更新；有新版时自动下载当前系统和架构对应的 ZIP，无新版时显示当前已是最新。
- 退出程序。

更新检查仅在点击 **Check for Updates...** 时执行，不会在启动时或后台定期联网。GitHub
请求超过 3 秒后会通过 jsDelivr CDN 重试版本发现；CDN 仍超时时会明确提示。新版 ZIP
优先从 GitHub Release 直连下载，连接或响应超时后改用 `gh-proxy.com/<原始链接>` 重试；
代理仍失败或超时时会明确提示。下载采用固定 32 KiB 缓冲区流式写入，最大接受 10 MiB，
有 GitHub 资产摘要时还会校验文件大小和 SHA-256。ZIP 保存到系统“下载”文件夹，并替换
同名的旧下载文件；Windows 和 macOS 都不会在程序运行中直接覆盖已安装版本，避免权限、
文件占用、签名和损坏现有安装的问题。

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
