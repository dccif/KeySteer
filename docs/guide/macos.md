# macOS

KeySteer 支持 macOS 14 及以上。将 `KeySteer.app` 移动到程序目录即可。

## GitHub Release 手动安装

从 [官方 GitHub Releases](https://github.com/dccif/KeySteer/releases/latest) 下载对应架构的 ZIP，解压后将 `KeySteer.app` 移入 `/Applications`。如果 Gatekeeper 提示应用无法打开，请先确认下载来源，然后运行：

```bash
sudo xattr -cr /Applications/KeySteer.app
```

## 首次授权（推荐）

1. 打开 `KeySteer.app` 之前。
2. 前往“系统设置 → 隐私与安全性 → 辅助功能”。
3. 允许 KeySteer 控制电脑。
4. 重新启动 KeySteer，并按 `Primary+E` 测试。

`UI Hint` 的 Vision 检测还需要“屏幕录制”权限。只使用 `Grid` 或 `Recursive Grid` 时不依赖屏幕内容识别，但键盘捕获仍需要辅助功能权限。

建议一次性就两个授权都完成，避免后续权限问题。

## 文件位置

打包的 `.app` 使用：

```text
~/Library/Application Support/KeySteer/
```

其中包括：

- `keysteer.<名称>.toml`：配置文件。
- `keysteer.log`：运行日志。
- `keysteer.log.1`、`.2`、`.3`：轮换日志。

## 滚动方向

如果你使用自然滚动，默认已开启垂直方向反转；可以在配置中调整：

```toml
[platform.macos.scroll]
invert_horizontal = false
invert_vertical = true
```

## 权限问题

如果菜单栏图标正常，但按键没有反应：

1. 确认辅助功能列表中授权的是 `KeySteer.app`。
2. 关闭并重新打开 `KeySteer`。
3. 运行 `keysteer --doctor` 查看键盘是否可用。
4. 仍有问题时查看数据目录中的 `keysteer.log`。

因为没开发者签名，未来的升级可能都需要 **重新授权**。

## 菜单栏图标

直接启动和开机启动都会在 AppKit 完成启动后创建同一个 KeySteer 程序图标；Tahoe 延迟准备菜单栏按钮时，KeySteer 会通过主事件循环继续完成配置。如果“活动监视器”里能看到 KeySteer，但菜单栏仍没有图标，请在 macOS 26 及以上检查[“系统设置 → 菜单栏”](https://support.apple.com/guide/mac-help/mchlad96d366/mac)中是否允许 KeySteer 显示。若已经允许仍不显示，请在该页面使用“重置控制中心”，再重新启动 KeySteer；Tahoe 26.6.2 可能错误保留其他程序造成的隐藏记录。系统也可能在菜单栏空间不足时临时隐藏部分图标。
