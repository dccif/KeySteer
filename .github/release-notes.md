## macOS manual installation

If you install `KeySteer.app` manually from this official GitHub Release and macOS Gatekeeper refuses to open it, move the app to `/Applications` and run:

如果你从本项目的官方 GitHub Release 手动安装 `KeySteer.app`，但 macOS Gatekeeper 阻止打开，请先将应用移到 `/Applications`，然后执行：

```bash
sudo xattr -cr /Applications/KeySteer.app
```

This command recursively clears the app bundle's extended attributes. Only use it for KeySteer downloaded from this repository's official Releases page. Then open KeySteer again and grant Accessibility and Screen Recording permissions when macOS requests them.

该命令会递归清除应用包的扩展属性。仅应对从本项目官方 Releases 页面下载的 KeySteer 使用此命令。随后重新打开 KeySteer，并按系统提示授予“辅助功能”和 “屏幕录制”权限。
