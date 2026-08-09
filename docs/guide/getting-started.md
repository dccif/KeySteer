# 快速上手

<script setup>
import KeyLayout from '../.vitepress/components/KeyLayout'
</script>

KeySteer 启动后默认安静地待在托盘或菜单栏，不会影响正常打字。第一次使用时，**可以先不去记操作 ，也不需要创建配置文件**。

::: tip 简单的一次启动
`Primary+E` 开始 → `h j k l` 移动 → `;` 点击 → `Esc` 结束

`Primary` 是跨平台名称：发布默认值在 Windows 上是左 `Alt`，在 macOS 上是 `Command`。尽可能保持手感一致
:::

## 第一次使用

1. 启动 KeySteer。
2. 按进入键：

   | Windows | macOS |
   | --- | --- |
   | `左 Alt + E` | `Command + E` |

3. 按住下面四个键移动鼠标：

   ```text
          K  上
   H  左  J  下  L  右
   ```

4. 按 `;` 左键点击。
5. 按 `Esc` 返回待机，键盘恢复正常输入。

看到鼠标移动并完成一次点击，就已经学会 KeySteer 最常用的操作了。

::: warning macOS 第一次没有反应？
需要先授予辅助功能权限，参见 [macOS 安装与授权](/guide/macos)。
:::

## 一张图记住工作方式

```mermaid
flowchart LR
    idle["待机<br/>正常打字"]
    normal["Normal<br/>移动、点击、滚动"]
    target["快速定位<br/>Grid<br/>Recursive Grid<br/>UI Hint"]

    idle -->|"Primary+E"| normal
    normal -->|"需要时再进入"| target
    target -->|"Esc"| normal
    normal -->|"Esc / q"| idle
```

平时只需在“待机”和 “Normal” 之间切换。三种定位模式是可选的，不需要一开始全部记住。



## 鼠标移动太远时

| 你想做什么 | 按键 | 模式 |
| --- | --- | --- |
| 快速到达屏幕的大致区域 | `g` | [Grid](/modes/grid) |
| 精确定位很小的目标 | `f` | [Recursive Grid](/modes/recursive-grid) |
| 直接选择按钮、链接或输入框 | `Primary+F` | [UI Hint](/modes/ui-hint) |

进入定位模式后按 `Esc` 返回 Normal；再按一次 `Esc` 返回待机。

## 常用操作

<KeyLayout
  layout="q w e r t y u i o p/Caps a s d f g h j k l ; '/Shift z x c v b n m , . Slash RShift/Ctrl Primary Alt Space"
  move="h j k l"
  click="; ' RShift"
  speed="Caps Shift v b"
  scroll="m ,"
  state="n"
  navigation="t y u i"
  mode="e f g q Primary"
  label="常用键位速记"
  hint="先认颜色，再按需记住其他键"
/>

| 按键 | 作用 | 记忆方式 |
| --- | --- | --- |
| `m` / `,` | 向下 / 向上滚动 | 在主键区直接滚动 |
| `Caps Lock` / `左 Shift` | 精确 / 慢速移动 | 按住后再按 `h j k l` |
| `v` 或 `b` | 快速移动 | 按住后再移动 |
| `'` / `右 Shift` | 右键 / 中键点击 | 与 `;` 左键相邻 |

<details open>
<summary><strong>查看完整默认键位</strong>（熟悉以后再看）</summary>

| 按键 | 作用 |
| --- | --- |
| `h j k l` | 左、下、上、右移动 |
| `Caps Lock` / `左 Shift` | 精确 / 慢速模式，对移动和滚动生效 |
| `v` 或 `b` | 快速模式，对移动和滚动生效 |
| `m` / `,` | 向下 / 向上滚动 |
| `;` / `'` / `右 Shift` | 左键 / 右键 / 中键点击 |
| `n` | 切换左键持续按下，用于拖拽 |
| `t` / `y` / `i` / `u` | 发送 `Home` / `End` / `Page Up` / `Page Down` |
| `g` / `f` / `Primary+F` | `Grid` / `Recursive Grid` / `UI Hint` |
| `Primary+S` | 切换到下一块显示器 |
| `q` 或 `Esc` | 返回`Idle`待机 |

</details>

## 按键不顺手？

打开 [配置与模拟器](/editor/) 直接查看键盘、修改绑定和颜色，然后下载自己的 TOML。KeySteer 不依赖配置文件；建议在 [默认文件](/generated/keysteer.default.toml) 的基础之上修改。

`Primary` 是跨平台名称：发布默认值在 Windows 上是左 `Alt`，在 macOS 上是 `Command`。高级用户可以通过 `[key_aliases]` 改成其他实体键。

<details>
<summary><strong>状态栏、诊断和配置位置</strong></summary>

右键托盘或菜单栏图标可以暂停、重载配置、设置开机启动、检查更新或退出。

需要检查配置或诊断环境时运行：

```bash
keysteer --check -c keysteer.user.toml
keysteer --doctor
keysteer --dump-config
```

- Windows 便携版的配置和日志通常在程序旁边。
- macOS `.app` 的配置和日志在 `~/Library/Application Support/KeySteer/`。

完整默认配置可 [下载](/generated/keysteer.default.toml)。更多内容见 [配置文件](/reference/configuration) 和 [模式与动作](/reference/modes-and-actions)。

</details>
