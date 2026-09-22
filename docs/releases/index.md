---
releaseHistory: true
outline: false
---

# 更新日志 / Release Notes

## 0.10.20

- 新增 Text Input 临时文本输入模式：从 Normal 一键进入，普通文字透传，输入结束后直接返回 Normal；进入、返回和编辑映射均使用普通按键绑定。
- 支持通过 `primary` 临时借用 Normal，也可配置绑定继承；修复临时 Normal 中 `screen next` 等切屏指令的结果未交给临时模式处理的问题。
- Text Input 默认隐藏模式文字提示。默认配置提供方向键、Backspace、Delete、Insert、Home／End 及 Ctrl／Shift 组合的注释示例，按需启用。

- Added Text Input for temporary typing from Normal, with configurable entry, exit, and editing bindings.
- Supports borrowing Normal with `primary` and optional binding inheritance; fixed screen-switch results being delivered to the base mode instead of the temporary mode.
- Text Input hides its text badge by default. Home-row editing mappings and Ctrl/Shift variants are provided as commented, opt-in examples.

### 简单启用与使用

使用新版默认配置时，Normal 中按 **`\`** 即可进入。已有用户配置请将下面的入口加入现有 `[normal.bindings]`，其他配置合并到对应表中；不要重复创建同名表或覆盖原有绑定。

```toml
[normal.bindings]
'\' = "text_input"

[text_input]
inherits = []
temporary_mode = "normal"
temporary_mode_keys = ["primary"]

[text_input.bindings]
'enter \ esc' = "normal"

# 可选：先向应用发送 Enter，再返回 Normal。
# 用下面两行替换上面的合并绑定：
# '\ esc' = "normal"
# enter = ["send enter", "normal"]

[mode_indicator.modes.text_input]
enabled = false
```

1. 保存配置，从托盘／状态栏菜单重载配置。
2. 按 `Primary+E` 进入 Normal，再按 `\` 输入文字；按 `Enter`、`\` 或 `Esc` 返回 Normal。
3. 输入期间按住 `Primary` 可临时使用 Normal，例如用 H/J/K/L 移动鼠标；松开继续输入。`Primary` 使用现有 `key_aliases` 配置。

默认返回键会被消费，Enter 不会提交给应用；需要提交时使用上面的注释序列。多行编辑或输入法需要 Enter 时，可把返回绑定改为 `'\ esc' = "normal"`。编辑组合键示例默认全部注释，取消所需行的注释即可启用。详见 [Normal 的临时文本输入](../modes/normal.md#临时输入文本)。

## 0.10.19

- 优化 Windows 和 macOS 的窗口调整、最大化、布局及撤销／重做，分批异步确认，减少慢窗口对其他窗口操作的影响。
- 优化 Normal 模式指针计算，复用窗口快照与卡片文字，减少重复查询和内存分配。
- 改进工作区后台保存、操作取消与恢复，合并重复错误日志，并完善异常时的按键释放和原生资源管理。

- Improved window adjustments, maximization, layouts, and undo/redo on Windows and macOS with batched asynchronous confirmation, reducing interference from slow windows.
- Optimized Normal-mode pointer calculations and reused window snapshots and card text to reduce repeated queries and memory allocations.
- Improved background workspace saves, cancellation and recovery, deduplicated error logs, and refined key release during error recovery and native resource management.

## 0.10.18

- 改进 Windows 和 macOS 的跨屏窗口识别与切换，支持更多窗口并减少多屏、标题变化或鼠标跳转导致的选窗错误和窗口误移动。

- Improved cross-display window detection and switching on Windows and macOS, supporting more windows while reducing wrong selections and unintended window moves caused by multiple displays, title changes, or pointer warps.

## 0.10.17

- 临时模式现在优先匹配完整组合键，并支持 `temporary_mode_passthrough_keys` 让指定按键交给当前模式处理，减少临时层快捷键冲突。
- Quick Switch 的触发键会先立即执行原有绑定，只有持续按住达到 `hold_ms` 才显示切换面板，不再拖慢普通按键响应。
- 修复 macOS 多屏覆盖层定位，跨屏显示时面板和窗口标识不再被错误限制到单个屏幕。

- Temporary modes now resolve full chords first and support `temporary_mode_passthrough_keys` so selected keys can fall through to the active mode, reducing shortcut conflicts.
- Quick Switch triggers now execute their original binding immediately and show the switcher only after being held for `hold_ms`, keeping normal key response fast.
- Fixed macOS multi-display overlay positioning so panels and window indicators are no longer incorrectly constrained to a single screen.

## 0.10.16

- 优化相交窗口切换（`window_overlap_next` / `window_overlap_previous`），减少重复计算。

- Improved overlapping-window switching (`window_overlap_next` / `window_overlap_previous`) with less repeated work.

## 0.10.15

- 新增可配置的 `window_overlap_next` / `window_overlap_previous`，可在 Normal 模式中只切换到与当前窗口有重叠的窗口，并将鼠标移到目标窗口中心。

- Added configurable `window_overlap_next` / `window_overlap_previous` actions to switch between overlapping windows from Normal mode and center the pointer on the selected window.

## 0.10.12

- 新增可配置的 `window_activate_next` / `window_activate_previous` 动作，可在 Normal 模式中直接切换前后窗口并将鼠标移到目标窗口中心，无需进入 Window 模式。

- Added configurable `window_activate_next` and `window_activate_previous` actions to switch between windows from Normal mode and center the pointer on the target without entering Window mode.

## 0.10.11

- Window 移动现在支持 Grid 和 Recursive Grid 精确定位窗口，可使用已配置的快捷键在不同屏幕间快速放置窗口。

- Window Move now supports Grid and Recursive Grid targeting, so you can precisely place windows across displays with your configured keyboard shortcuts.

## 0.10.10

- 优化 Windows 和 macOS 的 Window 模式资源回收。

- Improved resource cleanup in Window mode on Windows and macOS.

## 0.10.9

- 优化 Windows 和 macOS 上的键盘映射，让组合键不再误带入正在按住的 Ctrl、Alt、Shift 或 Command，使用更稳定流畅。

- Improved keyboard mappings on Windows and macOS so shortcuts no longer accidentally inherit held Ctrl, Alt, Shift, or Command keys and feel smoother and more reliable.

## 0.10.8

- 修复自定义组合键的前缀字符提前透传的问题，

- Fixed premature passthrough of custom chord prefixes. 

## 0.10.7

- 修复 macOS Retina 屏幕上快速切换面板过大、模式名称对齐错位的问题。
- 调整快速切换面板宽度，并根据名称长度分布平衡左右留白，保持名称左对齐，让内容在视觉上更居中。

- Fixed oversized quick-switch panels and misaligned mode names on macOS Retina displays.
- Refined quick-switch panel width and balanced side padding based on mode-name lengths for better visual centering while keeping names left-aligned.

## 0.10.6

- 新增常用模式快速切换：操作模式中长按 `Q` 查看面板，按数字切换；支持使用统计、黑名单及面板样式配置。
- 修复快速切换面板在高 DPI 下的重叠与边框异常；递归网格默认使用自动大字号。
- 窗口卡片引导线支持开关、线宽、颜色和透明度配置，网页编辑器同步支持。

- Added quick switching by mode usage: hold `Q` in an operating mode and press a number to switch, with usage statistics, a blacklist, and configurable panel styling.
- Fixed overlapping text and unwanted borders in the quick-switch panel at high DPI; Recursive Grid now uses large automatic font sizing by default.
- Added window card guide-line visibility, width, color, and opacity settings, also available in the web editor.

## 0.10.5

- 拆分窗口最大化与最小化操作：`F` 切换最大化／还原，`Shift+F` 切换最小化／还原，不再经过三态循环。按键提示仍合并在一行。
- 窗口卡片支持自定义颜色、字体、边框和百分比定位；Editor 可独立覆盖位置，自动避让。
- 模拟器新增卡片可视化编辑，支持拖动定位、样式预览及配置导出。

- Split window maximization and minimization into separate toggles: `F` maximizes/restores, while `Shift+F` minimizes/restores, replacing the three-state cycle. Both shortcuts remain on one help row.
- Customize window card colors, fonts, borders, and percentage positioning, with an independent Editor position override and automatic collision avoidance.
- Added visual card editing in the simulator, including drag positioning, live style previews, and configuration export.

## 0.10.4

- 按键提示支持配置默认显示状态：`mouse_key_help` 控制鼠标操作模式，`window_key_help` 控制窗口模式；均可通过 `?` 随时显示或隐藏。

- Configure the initial key-help visibility with `mouse_key_help` for pointer modes and `window_key_help` for window modes. Press `?` to show or hide either panel regardless of its default. 

## 0.10.3

修复 macOS 上的窗口布局、最大化和按键提示问题。

Fixed window layout, maximization, and key hint issues on macOS.

## 0.10.2

修复关闭窗口后，边框、编号等状态仍可能残留的问题。

Fixed an issue where borders, window numbers, and other window state could remain after closing a window.

## 0.10.1

**改进窗口移动、关闭**

- **移动更稳定**：连续移动和缩放窗口时不再反复拉动鼠标；细小位移跨帧累积，避免高刷新率下取整丢失。
- **关闭后及时清理**：修复按 `X` 关闭窗口后，边框和编号仍可能残留的问题。

**Improved window movement and closing**

- **Steadier movement**: continuously moving or resizing a window no longer repeatedly repositions the pointer. Small movements accumulate across frames instead of being lost to rounding at high refresh rates.
- **Cleanup after closing**: fixed an issue where borders and window numbers could remain after closing a window with `X`.

## 0.10.0

**重要功能更新：从操控鼠标，到安排整个工作区。** KeySteer 新增完整的窗口管理模式，移动、分屏、平铺、标签分组与布局复用，都可以留在键盘上完成。

- **Window 窗口管理**：按 `Alt+W` 进入（macOS 为 Option+W），移动、调整大小、居中、最大化／最小化、跨屏移动或关闭窗口，也能调整应用与系统的音量、静音和音频输出设备。
- **按任务选择整理方式**：Quick 快捷分屏；Editor 自动平铺、交换窗口、切分和调整区域；Tabs 自动整理同应用窗口，也能自由组合标签组。通过 Restore 复用已保存的布局和标签模板；保存时备注可留空，模板记住的是安排，不绑定应用名单。所有模式的按键与入口均可自定义。

同时，面向从旧版本升级的用户，回顾两项自 0.9.20 起提供的配置能力：

- **`key_help` 实时快捷键提示**：忘记按键时，打开面板查看当前模式的按键与动作。在现有 `[normal.bindings]` 中加入 `"?" = "key_help"`，重载配置后即可按 `?` 切换显示。
- **Shift 层符号绑定**：`?`、`!`、`+` 等符号可以直接写成绑定键，按实际输入的字符匹配，让常用动作有更多顺手的入口。

详细操作与视频请看 [窗口管理总览](https://dccif.github.io/KeySteer/window-management/)；提示面板与符号绑定见 [配置参考](/reference/configuration)。已有自定义绑定表的用户，请对照最新默认配置补入需要的新绑定。

**Major feature update: from pointer control to a complete keyboard-driven workspace.** Window adds movement, resizing, window-state and audio controls; Quick, Editor, and Tabs provide split layouts, tiling, and tab groups. Restore reuses saved arrangements with your current windows. All mode bindings and entry points are configurable. Also highlighted for users upgrading from older versions: live `key_help` hints and direct Shift-layer symbol bindings, available since 0.9.20. See the [window management guide](https://dccif.github.io/KeySteer/en/window-management/) for instructions and demos.

## 0.9.21

新增键盘模拟鼠标侧键点击，支持 Windows 和 macOS。在现有 `[normal.bindings]` 中加入以下配置，重载后进入 Normal 模式，点按 `T` / `Y` 即可模拟两个侧键：

```toml
t = "mouse_x1"
y = "mouse_x2"
```

侧键通常用于后退 / 前进，具体行为由应用决定；也支持 `press mouse_x1`、`release mouse_x1` 和 `toggle mouse_x2` 等按下、释放及锁定操作。

Added keyboard-triggered mouse side-button clicks on Windows and macOS. Add the bindings above to your existing `[normal.bindings]`, reload the configuration, then tap `T` / `Y` in Normal mode. Side buttons usually navigate Back / Forward, depending on the application. Explicit `press`, `release`, and `toggle` actions also support these buttons.

## 0.9.20

- 新增实时按键提示面板，可查看当前模式可用的按键与动作，并自定义面板样式。
- 新增鼠标侧键绑定，支持组合键及按住执行动作。
- 支持直接绑定 `?`、`!`、`+` 等符号；网页配置模拟器新增完整符号键位和按键提示预览。

Added a live key-help panel with customizable styling, mouse side-button bindings with chord and hold support, and direct symbol bindings such as `?`, `!`, and `+`. The web configuration simulator now includes all shifted symbol keys and a key-help preview.

## 0.9.19

- Window Mover 默认快捷键改为 `Primary+D`，可在配置中自定义；`Primary+S` 独立切换鼠标所在显示器。
- 网页配置模拟器新增窗口移动和 `precision_toggle`、`slow_toggle`、`fast_toggle` 动作配置，支持导入、编辑和导出。

Window Mover now defaults to `Primary+D`, with configurable bindings and independent `Primary+S` pointer-display switching. The web configuration simulator now supports importing, editing, and exporting window-movement and speed-toggle actions.

## 0.9.18

修复状态栏 Reload Configuration 后新配置的按键路由丢失，保存并重载后无需退出程序。

Fixed lost key routes after Reload Configuration, so saved settings take effect without quitting.

## 0.9.17

新增 `precision_toggle`、`slow_toggle` 和 `fast_toggle` 速度模式切换功能，按下即可进入速度模式，无需持续按住。

Added `precision_toggle`, `slow_toggle`, and `fast_toggle` speed-mode toggles. Press once to enter a speed mode without holding the key down.

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
