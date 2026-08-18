# UI Hint mode

<script setup>
import ModeVideo from '../../.vitepress/components/ModeVideo'
</script>

`UI Hint` displays short labels for interactive elements on screen. It works well with buttons, links, menus, checkboxes, inputs, sliders, and list items: type a label to target the element without estimating coordinates.

<ModeVideo file="uihint.mp4" title="UI Hint mode demonstration" description="Scanning interface elements, filtering with typed labels, and targeting a control." />

Enter from Normal with `Primary+F`. By default it only moves the pointer: after a label matches, use a Normal click binding to confirm the click.

## Default controls

| Key | Action |
| --- | --- |
| Label characters | Filter candidates; target on a complete match |
| `Shift` | Cycle through overlapping elements |
| `Primary+R` | Scan again |
| `Primary` | Temporarily use Normal movement, scrolling, and clicking |
| `Primary+Q` / `Esc` | Return to Normal |

## Scanning strategies

- Windows uses `hybrid` by default: UI Automation and the full visual pipeline scan in parallel and merge results. UIA fills in native window buttons such as minimise, maximise, and close while OCR and built-in pixel-region fallback cover custom interfaces. Use `axtree` or `vision` to enable one pipeline only.
- macOS supports `axtree`, `vision`, and `hybrid`.

Vision needs macOS Screen Recording permission; keyboard capture still needs Accessibility permission. On startup, Windows asynchronously detects system OCR and locally installed WeChat OCR components without configuration. OCR engines and the WeChat helper are created only during scanning and released before it finishes. When neither OCR engine returns usable results, KeySteer uses built-in region recognition that does not depend on OpenCV.

## Common configuration

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

`scan_timeout_ms` controls how long a scan may run. A page that produces no elements is retried `scan_retry_count` times; increase the timeout and retry count for large or complex pages.

## Visual style

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

## Visual-recognition guidance

If a page has no useful accessibility information, try `strategy = "vision"`. If it produces too many labels, narrow `clickable_roles`. Windows WeChat OCR is an optional local enhancement; KeySteer never downloads, copies, or packages WeChat binaries.
