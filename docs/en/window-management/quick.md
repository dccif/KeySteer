# Quick: fast split layouts

<script setup>
import ModeVideo from '../../.vitepress/components/ModeVideo'
</script>

Enter with A, then choose placement and ratios with direction keys.

<ModeVideo file="quick.mp4" title="Quick: fast split layouts" description="Enter with A, then choose placement and ratios with direction keys." />

## How to use it

1. Place the pointer over your browser, press `Alt+W`, release the entry keys, then press `A` for Quick.
2. Press `H` to place the browser on the left half.
3. Type your editor's window number, then press `L` to place it on the right half.
4. Press `Q` to return to Window, then `Q` again to start working.

Quick adjusts each axis independently. The first direction starts with the configured proportion nearest half a screen; repeating the direction shrinks it, while the opposite direction expands it. For example, press `K` after choosing the left half to place the window in the upper-left area. Default proportions are `1/4, 1/3, 1/2, 2/3, 3/4`, plus an automatically included full-screen proportion. Press `Z` to undo.

## Configure the scale

```toml
[window_quick]
split_ratios = ["1/4", "1/3", "1/2", "2/3", "3/4"]
```

[Window management overview](/en/window-management/) · [Full configuration reference](/en/reference/configuration)
