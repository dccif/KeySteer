## Windows 下载 / Download

一般下载默认版即可；如果无法启动，请尝试文件名带 `-compatible.zip` 的兼容版。启动后使用「检查更新」，程序会自动选择这台电脑支持的最高等级（AVX-512 / AVX2 / Compatible），同一版本也能切换，无需自行判断；下载完成后确认安装即可。

Download the default build for most PCs. If it will not start, try the compatible package ending in `-compatible.zip`. Once running, use **Check for Updates** to automatically select the highest build your PC supports (AVX-512 / AVX2 / Compatible), even within the same version. No manual CPU checks are needed; confirm installation after the download.

## macOS 手动安装 / Manual installation

如果你从本项目的官方 GitHub Release 手动安装 `KeySteer.app`，但 macOS Gatekeeper 阻止打开，请先将应用移到 `/Applications`，然后执行：

If you install `KeySteer.app` manually from this official GitHub Release and macOS Gatekeeper refuses to open it, move the app to `/Applications` and run:

```bash
sudo xattr -cr /Applications/KeySteer.app
```
