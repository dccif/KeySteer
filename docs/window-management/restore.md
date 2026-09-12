# Restore：恢复布局与标签模板

<script setup>
import ModeVideo from '../.vitepress/components/ModeVideo'
</script>

保存的是空间和分组安排，恢复时用当前需要的窗口填入。

## 保存操作 {#save}

<ModeVideo file="save.mp4" title="保存布局与标签组演示" description="演示填写备注和留空保存；此片段展示保存，恢复步骤请看下文。" />

在 Editor 或 Tabs 按 `Ctrl+S`，在底部输入备注并按 Enter 保存；备注可留空，留空时自动生成默认名称，最多 80 个字符。例如命名为“写代码”或“阅读与笔记”。

## 恢复操作 {#restore}

从 Window 按 `R` 打开统一预设列表，输入编号恢复；`PageUp / PageDown` 翻页，每页六项。

| 保存类型 | 恢复时会发生什么 |
| --- | --- |
| Layout 布局 | 用当前窗口填入保存的区域；成功后默认进入 Editor，继续微调 |
| Tabs 标签模板 | 进入 Tabs，依次选择所需窗口，选满后自动应用；选满前可取消 |

**模板不保存应用身份，也不会启动应用或重新打开文档。** 布局恢复使用当前窗口的最近活动顺序，多余窗口保持原位，窗口不足时保留空区域。标签模板由你重新指定成员。

要删除预设，在 Restore 按 `X` 切到删除状态，输入编号核对名称，再按 `Enter` 删除。再按 `X` 回到恢复状态。删除模板不影响已经运行的标签组。

布局和标签模板共用 `workspace.ksw`：Windows 便携版在程序目录，macOS `.app` 在 `~/Library/Application Support/KeySteer/`。可通过[配置与模拟器](/editor/)导入、练习和导出；网页只改变示例窗口与浏览器存储。将导出的文件替换到程序数据目录，再打开 Restore 读取；替换前可先备份原文件。

[返回窗口管理总览](/window-management/) · [完整配置参考](/reference/configuration)
