# 更新日志 / Release Notes

## 0.9.4

跨平台 UI Hint 统一使用零分配文字分析和精确前缀高亮；macOS 同时校正文字垂直位置，并以单层渲染减少原生图层与内存开销。

Cross-platform UI Hint now shares allocation-free text analysis and exact prefix highlighting, while macOS gains corrected vertical positioning and a single-layer renderer with lower native-layer and memory overhead.

## 0.9.3

UI Hint 标签进一步收紧边距并校正跨平台文字垂直居中，输入前缀现在按实际字符范围精确变色；macOS 同时减少每个标签的原生图层，避免窄字母高亮残缺或溢出。

UI Hint labels now use tighter spacing and improved cross-platform vertical centering, while exact typed-prefix coloring on macOS also uses fewer native layers to prevent incomplete or overflowing highlights.

## 0.9.2

Windows UI Hint 默认改用 Hybrid，并行合并 UI Automation 与视觉结果以补足窗口控制按钮；标签更紧凑易读且位置略微上移，同时进一步加快分块 OCR 的首批显示与覆盖层响应，修复重扫位置更新竞态，并收紧资源清理与安全边界。

Windows UI Hint now defaults to Hybrid, merging UI Automation and visual results in parallel to cover window controls, with more compact, readable, and slightly higher labels, earlier tiled OCR results, faster overlay response, a rescan position-race fix, and tighter cleanup and safety boundaries.

## 0.9.1

Windows 系统 OCR 会按 CPU 与图片尺寸自动并行切分、流式显示每块结果，并按实际可用能力跳过不需要的识别资源。

Windows system OCR now tiles by CPU and image size, streams completed regions, and skips unavailable OCR resources entirely.

## 0.9.0

Windows UI Hint 新增按需双 OCR，扫描结果更完整；同时降低截图延迟与峰值内存，退出后立即清理识别资源。

Windows UI Hint adds on-demand dual OCR with lower capture latency, lower peak memory, and immediate cleanup after scanning.

## 0.8.14

进一步降低普通输入与 UI Hint 的尾延迟和临时分配，并收紧 Windows/macOS 原生线程与 Unsafe 安全边界。

Further reduce input and UI Hint tail latency and temporary allocations while tightening native thread and unsafe boundaries on Windows and macOS.

## 0.8.13

UI Hint 扫描结果改为零拷贝传递，快捷键注入减少临时分配，并进一步收紧跨平台 Unsafe 边界。

UI Hint now consumes scan results without cloning, chord injection avoids temporary allocations, and cross-platform unsafe boundaries are tighter.

## 0.8.12

修复 Windows/macOS 检查更新失败、Windows 模拟器入口误开文件管理器及更新线程退出清理问题。

Fix update checks on Windows and macOS, open the simulator in the Windows browser, and clean up update workers reliably.

## 0.8.11

新增从托盘或菜单栏一键将当前配置安全带入网页模拟器，并修复 macOS 检查更新闪退及更新线程退出清理问题。

Open the current configuration safely in the web simulator from the tray or menu bar, and fix macOS update-check crashes and worker cleanup on exit.

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
