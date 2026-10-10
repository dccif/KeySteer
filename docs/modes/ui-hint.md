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

- Windows 默认使用 `hybrid`：UI Automation 与完整视觉管线并行扫描并合并结果，可由 UIA 补足最小化、最大化、关闭等原生窗口按钮，同时保留 OCR 与内置像素区域回退对自绘界面的覆盖。`axtree` 和 `vision` 可分别只启用其中一条管线。
- Windows 扫描目标是本轮扫描真正开始时鼠标下的窗口及其菜单、弹窗和对话框；不要求它先成为前台窗口。普通鼠标移动不会持续触发扫描，目标、焦点或显示器变化时才会清除旧标签并立即按最新鼠标位置重新扫描。
- 鼠标位于桌面、任务栏或 KeySteer 覆盖层而没有可扫描窗口时，会提示 `No window under the pointer — move the pointer over a window`。该情况不会创建 OCR/截图任务，也不会显示可能已被用户修改的固定重扫快捷键。
- macOS 支持 `axtree`、`vision` 和 `hybrid`。

Vision 需要 macOS 的“屏幕录制”权限；键盘捕获仍需要“辅助功能”权限。Windows 会在启动后异步探测系统 OCR 和本机已有的微信 OCR 组件，无需增加配置；OCR 引擎与微信 helper 仅在扫描时创建并在结束前清理。退出 UI Hint 返回 Normal/Idle 会取消 UIA、OCR 和截图 generation，释放本轮图片、bitmap、目标与大型缓存，不会继续后台识别。两者都不可用或没有有效结果时，使用不依赖 OpenCV 的内置区域识别。

## 搜索与结果预览

<ModeVideo
  file="uihint-search.mp4"
  title="UI Hint 搜索、复制与取色"
  description="22 秒中英双语演示，纯字幕：简拼搜索、切换结果、复制信息、微调点位与取色。"
/>

https://github.com/user-attachments/assets/84d0872d-81f6-4e42-aa7f-767061cef646

按 `/` 搜索文字、中文简拼或标签。有匹配结果时自动显示排序后的首项信息面板，按 `Tab` 从当前结果继续切换，末项回到首项；继续输入会清除手动预览，重新筛选并显示新的首项。默认按 `Enter`、`/` 或 `Primary+Q` 接受当前预览并移动鼠标，`Esc` 取消；切换本身不会移动鼠标。

按 `Ctrl+1/2/3/4` 分别复制文字、辅助信息、坐标或颜色。轻按 `Ctrl` 进入点编辑模式，用 `H/J/K/L` 微调取色位置，`Ctrl+Shift+4` 切换 HEX、RGB、HSL。以上按键均可配置。

确认、取消及搜索框编辑键统一使用 `[ui_hint.search_edit_keys]` 的“按键 = 动作”格式，例如 `"enter / primary+q" = "accept"`、`"primary+v" = "paste"`。左侧用空格分隔多个按键，省略的动作保留默认键；旧格式仍可读取，导出使用新格式。

切换键沿用 `[ui_hint.search_bindings]` 的 `point_next`，可改为其他按键或组合键。进入点位调整后，该键仍切换已选点；空格分隔的多选和信息栏拼接规则保持不变。

```toml
[ui_hint]
search_match_priority = ["pinyin", "text", "label"]

[ui_hint.search_bindings]
ctrl = "point_toggle"
tab = "point_next" # 可改为 f9、alt+f9 等
"ctrl+shift+4" = "color_next"
```

先按匹配程度排序：完整词／完整标签 → 词首／标签前缀 → 包含匹配。`search_match_priority` 从前到后决定同等匹配程度下的类别顺序：`label` 为标签代码，`text` 为 OCR／辅助功能文字及控件类型，`pinyin` 为中文简拼。三项必须各出现一次，默认简拼 → 文字 → 标签；例如 `["label", "text", "pinyin"]` 在同等匹配程度下优先预览标签结果。完整标签会排在简拼或文字的前缀／包含匹配之前，同分保持扫描顺序，空格项保持输入顺序。`@la` 或 `la@` 始终只搜索标签。显式 `search_bindings` 表会替换整张默认映射，上例保留了其他默认操作。

搜索内容已包含辅助功能信息和控件类型，无需额外开启。例如输入 `输入框`、`文本框`、`text_field` 或简拼 `srk` 可匹配已识别的输入框；这些匹配归入 `text`（简拼归入 `pinyin`），目前没有独立的辅助信息／类型优先级。搜索按关键词匹配，不解析“所有输入框”这样的自然语言指令。

## 常用配置

```toml
[ui_hint]
strategy = "hybrid"
hint_characters = "asdfghjkl"
scan_timeout_ms = 2500
scan_retry_count = 1
scan_retry_delay_ms = 200
visible_check_enabled = false
placement = "bottom"
label_x_offset = 0
label_y_offset = -8
clickable_roles = ["button", "link", "checkbox", "text_field", "menu_item"]

[ui_hint.lifecycle]
after_finish = "normal"
after_click = "normal"
```

`scan_timeout_ms`：扫描超时设置。只有扫描成功/超时后仍没有标签时，程序才会按 `scan_retry_count` 重试；窗口变化属于立即重定向，不占用重试次数。大型或复杂的页面可以适当提高超时和重试次数。


## 视觉样式

```toml
[ui_hint.ui]
font_size = 17
padding_x = -1
padding_y = -1
border_width = 1

[ui_hint.boundary_highlight]
enabled = false
border_width = 1

[ui_hint.search_input_ui]
position = "bottom_center"
width = 320
```

## 视觉识别建议

如果页面没有可用无障碍信息，可尝试 `strategy = "vision"`；如果标签太多，可以尝试缩小 `clickable_roles` 范围。Windows 微信 OCR 是可选的本机增强项，KeySteer 不下载、复制或打包微信二进制。
