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
- On Windows, each scan targets the window under the pointer when native submission actually begins, together with its menus, popups, and dialogs. The window does not have to be foreground first. Ordinary pointer movement does not poll or continuously rescan; a target, focus, or display change clears stale hints and immediately retargets from the latest pointer position.
- If the pointer is over the desktop, taskbar, or KeySteer overlay with no application window below it, KeySteer shows `No window under the pointer — move the pointer over a window`. It starts no OCR or screenshot work and does not advertise a fixed rescan shortcut that the user may have changed.
- macOS supports `axtree`, `vision`, and `hybrid`.

Vision needs macOS Screen Recording permission; keyboard capture still needs Accessibility permission. On startup, Windows asynchronously detects system OCR and locally installed WeChat OCR components without configuration. OCR engines and the WeChat helper are created only during scanning and released before it finishes. Leaving UI Hint for Normal or Idle cancels the UIA, OCR, and capture generation and releases its image, bitmap, targets, and oversized buffers. When neither OCR engine returns usable results, KeySteer uses built-in region recognition that does not depend on OpenCV.

## Search and result previews

<ModeVideo
  file="uihint-search.mp4"
  title="UI Hint search, copying, and color picking"
  description="22-second bilingual demo, subtitles only: pinyin initials, result cycling, copying information, point adjustment, and color picking."
/>

https://github.com/user-attachments/assets/84d0872d-81f6-4e42-aa7f-767061cef646

Press `/` to search text, Chinese pinyin initials, or label codes. Matching results immediately show the first ranked result panel. Press `Tab` to advance from the current result and wrap from the last back to the first. Editing the query clears the manual preview, filters again, and shows the new first result. By default, `Enter`, `/`, or `Primary+Q` accepts the current preview and moves the pointer; `Esc` cancels. Cycling itself does not move the pointer.

Press `Ctrl+1/2/3/4` to copy text, accessibility information, coordinates, or color, respectively. Tap `Ctrl` to edit the point, adjust the sampling position with `H/J/K/L`, and use `Ctrl+Shift+4` to cycle through HEX, RGB, and HSL. All these keys are configurable.

Confirmation, cancellation, and input editing use key = action entries in `[ui_hint.search_edit_keys]`, such as `"enter / primary+q" = "accept"` and `"primary+v" = "paste"`. Separate alternative keys with spaces on the left. Omitted actions keep their defaults. Legacy tables remain readable; exports use the new format.

The cycling key uses the existing `point_next` action in `[ui_hint.search_bindings]` and accepts a different key or chord. During point adjustment it still cycles selected points. Space-separated multi-selection and concatenated fields retain their existing behavior.

```toml
[ui_hint]
search_match_priority = ["pinyin", "text", "label"]

[ui_hint.search_bindings]
ctrl = "point_toggle"
tab = "point_next" # Can be changed to f9, alt+f9, etc.
"ctrl+shift+4" = "color_next"
```

Results rank by match quality first: whole words/codes → word/label prefixes → substrings. `search_match_priority` orders groups with equal match quality: `label` means label codes, `text` means OCR/accessibility text and control types, and `pinyin` means Chinese initials. Include each exactly once. The default is initials → text → labels; for example, `["label", "text", "pinyin"]` prefers labels when match quality is equal. An exact label precedes initials/text prefixes and substrings; ties retain scan order, and space-separated items retain input order. `@la` and `la@` always restrict matching to labels. An explicit `search_bindings` table replaces the default map; the example keeps the other default actions.

Search already includes accessibility information and control types. For example, `text_field`, `输入框`, `文本框`, or Chinese initials `srk` can match identified input fields without an extra setting. These matches belong to `text` (initials belong to `pinyin`); accessibility and types do not have separate priority entries. Search uses keywords rather than natural-language commands such as “all input fields.”

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

`scan_timeout_ms` controls how long a scan may run. Automatic retry applies only after `Success` or `TimedOut` produces no hints; a window/context change retargets immediately without consuming the retry count. Increase the timeout and retry count for large or complex pages.

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
