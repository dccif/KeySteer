# macOS

KeySteer 支持 macOS 14 及以上。将 `KeySteer.app` 移动到程序目录即可。

## 首次授权（推荐）

1. 打开 `KeySteer.app` 之前。
2. 前往“系统设置 → 隐私与安全性 → 辅助功能”。
3. 允许 KeySteer 控制电脑。
4. 重新启动 KeySteer，并按 `Primary+E` 测试。

UI Hint 的 Vision 检测还需要“屏幕录制”权限。只使用 Grid 或 Recursive Grid 时不依赖屏幕内容识别，但键盘捕获仍需要辅助功能权限。

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

裸二进制保持 portable 行为，把配置和日志放在可执行文件旁边。

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

因为没开发者签名，未来的升级可能都需要**重新授权**。
