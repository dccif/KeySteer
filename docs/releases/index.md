# 更新日志 / Release Notes

## 0.9.16

改进 Window Mover：最大化窗口现在可直接移到另一块显示器。

Improved Window Mover: maximized windows now move directly to another display while staying maximized。

## 0.9.15

新增 Window Mover 插件：将鼠标放在应用窗口上，按 `Primary+S+D` 即可把窗口和鼠标一起移到下一块显示器，并保持窗口内的相对位置。单独按 `Primary+S` 仍只切换鼠标所在显示器；可使用 `move_window previous` 或 `move_window <显示器编号>` 自定义其他目标。

Added the Window Mover plugin: point at an application window and press `Primary+S+D` to move both the window and pointer to the next display while preserving the pointer's relative position. `Primary+S` alone still switches the pointer display. Configure `move_window previous` or `move_window <display number>` for other destinations.

## 0.9.13

重整内部配置、模式与运行时边界并优化大批量 UI Hint 标签生成，在保持原有匹配、遮挡和显示层切换语义的同时减少分配；Windows 正式签名版现在支持下载后验证同一发布者、自动替换和重启，启动失败会恢复旧版本。

Reorganized the internal configuration, mode, and runtime boundaries and optimized large UI Hint label batches without changing matching, occlusion, or display-layer switching semantics. Signed Windows releases can now verify the same publisher, replace and restart automatically after downloading, and restore the previous version if startup fails.

## 0.9.12

修复 macOS 上 UI Hint 视觉识别失败、状态栏图标不显示、运行中撤销辅助功能权限可能造成输入卡住，以及滚动列表的窗口外元素仍生成标签的问题。

Fixed UI Hint visual recognition failures, missing top status icons, input becoming unresponsive after Accessibility access was revoked, and hints appearing for off-screen list items on macOS.

## 0.9.11

提升输入与 UI Hint 的响应速度，减少 OCR 扫描等待，并加强 Windows 与 macOS 的取消和资源清理，让连续使用更流畅稳定。

Improved input and UI Hint responsiveness, reduced OCR scan waits, and strengthened cancellation and resource cleanup on Windows and macOS for smoother, more reliable repeated use.

## 0.9.10

修复拖拽自动释放后鼠标按住提示和按下颜色可能残留的问题，并减少模式指示器刷新时的临时分配与重复处理，让输入状态清理和指针提示更稳定、轻快。

Fixed held-mouse text and pressed colors sometimes remaining after automatic drag release, while reducing temporary allocations and duplicate work in shared indicator and input-state handling.

## 0.9.9

新增可选的修饰键拖拽自动释放：长按鼠标键后可透传任意组合的修饰键，并在指针停止移动后自动松开鼠标键；默认关闭。

Added optional automatic release for modifier-assisted dragging: after holding a mouse button, any modifier combination can pass through and the button is released when pointer movement stops; disabled by default.

## 0.9.8

修复与此前鼠标长按同类的键盘 `toggle` 状态问题：多个修饰键现在可在激活键前后立即锁定，松开物理键后仍保持按下，且不会把 `toggle` 激活键组合透传给当前窗口。

Fixed the keyboard `toggle` state issue caused by the same kind of input transition as the earlier mouse long-press bug: multiple modifiers can now latch immediately in either order, remain held after their physical keys are released, and no longer pass the toggle activation chord to the focused window.

## 0.9.7

修复 [Issue #1](https://github.com/dccif/KeySteer/issues/1)：鼠标按键进入按下状态前可能意外触发一次点击；同时将 Rust 工具链升级至 1.98。

Fixed [Issue #1](https://github.com/dccif/KeySteer/issues/1), where a click could be triggered accidentally before a mouse button entered its held state, and upgraded the Rust toolchain to 1.98.

## 0.9.6

UI Hint 针对常见的 129–256 个标签新增会话级复用工作区、精确的 X 轴扫描和二元重叠组快速路径，

UI Hint now uses a reusable session workspace, an exact X-axis sweep, and a fast path for two-label overlap groups in the common 129–256-label range while preserving the inline path through 128 labels. 

## 0.9.5

Windows UI Hint 现在扫描每轮提交时鼠标下的窗口组，窗口上下文变化会立即清除旧标签并重新定位，不占用失败重试次数；Hybrid 同时共享单份扫描计划，并在返回 Normal 或 Idle 时完整取消识别与释放本轮资源。

Windows UI Hint now scans the window group under the pointer at submission time. Window-context changes immediately clear stale hints and retarget without consuming retries, while Hybrid shares one scan plan and fully cancels recognition and releases generation resources when returning to Normal or Idle.

## 0.9.4

跨平台 UI Hint 统一使用零分配文字分析和精确前缀高亮，并修正最终视觉层切换。

Cross-platform UI Hint now shares allocation-free text analysis and exact prefix highlighting, with corrected final visual-layer cycling.

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

新增从 Windows 托盘或 macOS 顶部状态图标一键将当前配置安全带入网页模拟器，并修复 macOS 检查更新闪退及更新线程退出清理问题。

Open the current configuration safely in the web simulator from the Windows tray or macOS top status icon, and fix macOS update-check crashes and worker cleanup on exit.

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
