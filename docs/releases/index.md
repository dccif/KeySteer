# 更新日志 / Release Notes

## 0.8.10

UI Hint 退出后会立即取消过期扫描、重复进入依然快速，并修复 macOS 撤销辅助功能权限时可能卡住的问题。

UI Hint now cancels stale scans immediately on exit, keeps repeated entry fast, and fixes a potential hang when macOS Accessibility permission is revoked.

## 0.8.9

UI Hint 扫描与重叠切换更稳定，启动和输入响应更快、内存占用更低，并进一步收紧原生资源与 Unsafe 安全边界。

UI Hint scanning and overlap switching are more reliable, startup and input are faster with lower memory use, and native resource and unsafe boundaries are tighter.

## 0.8.8

输入响应和 UI Hint 扫描更快、内存与安装体积更小。

Input response and UI Hint scanning are faster with lower memory use and a smaller package.

## 0.8.7

Windows 和 macOS 的光标与提示移动更加流畅，组合按键和长按操作也更快、更省内存。

Cursor and indicator movement is now smoother on Windows and macOS, while key combinations and hold actions are faster and use less memory.

## 0.8.6

修复 `n = "toggle"`：可单独长按 `n` 让它保持按下，和键盘或鼠标按键组合时无论先后顺序都能正确锁定，短按 `n` 仍会全部松开。

Fixed `n = "toggle"`: hold `n` alone to keep it pressed, use it with keyboard or mouse keys in either order to lock them correctly, and tap `n` to release everything.

## 0.8.5 

Windows 和 macOS 的移动、显示、按键响应及界面查找更快、更省内存，同时保持原有配置和操作方式不变。

Windows and macOS now feel faster and use less memory for movement, display, keyboard input, and UI search, with no changes to existing configuration or controls.
