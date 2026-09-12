# 窗口管理总览

<script setup>
import ModeVideo from '../.vitepress/components/ModeVideo'
</script>

从移动一个窗口开始，逐步安排分屏、平铺和标签组，保存成可复用的工作区。Window 与四个默认子模式都有独立、可自定义的按键绑定。

## 先试一次

鼠标放到普通窗口上 → `Alt+W` → 松开入口键 → `H/J/K/L` 移动 → `Q` 结束。macOS 使用 Option+W，不是 Command+W。

| 要做什么 | 模式 | 从 Window 进入 |
| --- | --- | --- |
| 移动、缩放、居中、切换窗口状态，并控制应用和系统声音。 | [Window](/window-management/window) | — |
| 方向键安排窗口位置与分屏比例。 | [Quick](/window-management/quick) | `A` |
| 自动平铺，再交换窗口、切分与调整区域。 | [Editor](/window-management/editor) | `E` |
| 自动整理同应用的兼容窗口，也可自由组合。 | [Tabs](/window-management/tabs) | `T` |
| 保存的是空间和分组安排，恢复时用当前需要的窗口填入。 | [Restore](/window-management/restore) | `R` |

<ModeVideo file="window.mp4" title="Window 窗口操控演示" description="模拟器下方同步显示按键与当前操作。" />

## 通用操作与问题排查

| 现象 | 可以怎么做 |
| --- | --- |
| 没有看到目标窗口编号 | 确认当前屏幕、最小化状态和窗口兼容性；多屏与最小化候选可在各模式配置中调整 |
| 调整后退出，没有恢复原位 | 退出保留布局；使用 `Z` 撤销，或在 Window/Quick/Editor 用 `Shift+C` 恢复本次会话初始状态 |
| `X` 的效果和预期不同 | Window 关闭窗口，Editor 删除区域，Tabs 解散组，Restore 切换删除状态；先看当前模式提示 |
| 升级后新快捷键没有反应 | 显式 `[模式.bindings]` 会替换该模式默认表；对照最新默认配置补入新绑定 |

继续阅读：[窗口配置](/reference/configuration#window-配置) · [模式与动作参考](/reference/modes-and-actions#window-模式) · [macOS 安装与授权](/guide/macos)。
