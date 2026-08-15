# UI Hint 标签模式

<script setup>
import ModeVideo from '../.vitepress/components/ModeVideo'
</script>

`UI Hint` 会为屏幕上的可交互元素显示短标签。它适合按钮、链接、菜单、复选框、输入框、滑块和列表项；输入标签即可定位，不必估算坐标。

<ModeVideo
  file="uihint.mp4"
  title="UI Hint 模式演示"
  description="展示扫描界面元素、输入标签筛选并把鼠标定位到目标控件。"
/>

从 `Normal` 按 `Primary+F` 进入。默认只定位，不自动点击：标签命中后先移动鼠标，再用 `Normal`状态的点击键确认。

## 默认操作

| 按键 | 作用 |
| --- | --- |
| 标签字符 | 筛选候选元素；完整匹配后定位 |
| `Shift` | 在重叠元素之间切换 |
| `Primary+R` | 重新扫描 |
| `Primary` | 临时使用 Normal 的移动、滚动和点击 |
| `Primary+Q` / `Esc` | 返回 Normal |

## 扫描方式

- Windows 的 `axtree` 使用 UI Automation；`vision` 使用系统/微信 OCR 与内置像素区域回退；`hybrid` 并行合并两条管线。
- macOS 支持 `axtree`、`vision` 和 `hybrid`。

Vision 需要 macOS 的“屏幕录制”权限；键盘捕获仍需要“辅助功能”权限。Windows 会在启动后异步探测系统 OCR 和本机已有的微信 OCR 组件，无需增加配置；OCR 引擎与微信 helper 仅在扫描时创建并在结束前清理，不会在 UI Hint 退出后继续后台识别。两者都不可用或没有有效结果时，使用不依赖 OpenCV 的内置区域识别。

## 常用配置

```toml
[ui_hint]
strategy = "vision"
hint_characters = "asdfghjkl"
scan_timeout_ms = 2500
scan_retry_count = 1
scan_retry_delay_ms = 200
visible_check_enabled = false
placement = "bottom"
label_x_offset = 0
label_y_offset = -4
clickable_roles = ["button", "link", "checkbox", "text_field", "menu_item"]

[ui_hint.lifecycle]
after_finish = "normal"
after_click = "normal"
```

`scan_timeout_ms`：扫描超时设置，有的程序或者界面可能扫描不到任何元素，程序会按 `scan_retry_count` 重试。大型或复杂的页面可以适当提高超时和重试次数。


## 视觉样式

```toml
[ui_hint.ui]
font_size = 15
border_width = 1

[ui_hint.boundary_highlight]
enabled = false
border_width = 1

[ui_hint.search_input_ui]
position = "bottom_center"
width = 320
```

## 视觉识别建议

如果页面没有可用无障碍信息，优先尝试 `strategy = "vision"` 或 `"hybrid"`；如果标签太多，可以尝试缩小 `clickable_roles` 范围。Windows 微信 OCR 是可选的本机增强项，KeySteer 不下载、复制或打包微信二进制。
