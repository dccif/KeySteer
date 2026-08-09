## macOS manual installation

If you install `KeySteer.app` manually from this official GitHub Release and macOS Gatekeeper refuses to open it, move the app to `/Applications` and run:

如果你从本项目的官方 GitHub Release 手动安装 `KeySteer.app`，但 macOS Gatekeeper 阻止打开，请先将应用移到 `/Applications`，然后执行：

```bash
sudo xattr -cr /Applications/KeySteer.app
```
