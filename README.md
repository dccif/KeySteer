# KeySteer

<p align="center">
  <a href="https://github.com/dccif/KeySteer/actions/workflows/pages.yml"><img alt="Page status" src="https://img.shields.io/github/actions/workflow/status/dccif/KeySteer/pages.yml?branch=main&amp;label=Page&amp;style=flat&amp;logo=github&amp;logoColor=white"></a>
  <a href="https://github.com/dccif/KeySteer/actions/workflows/build.yml"><img alt="Build status" src="https://img.shields.io/github/actions/workflow/status/dccif/KeySteer/build.yml?branch=main&amp;label=Build&amp;style=flat&amp;logo=github&amp;logoColor=white"></a>
  <a href="https://github.com/dccif/KeySteer/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/dccif/KeySteer?display_name=tag&amp;sort=semver&amp;label=Release&amp;style=flat"></a>
  <a href="rust-toolchain.toml"><img alt="Rust 1.98" src="https://img.shields.io/badge/Rust-1.98-dea584?style=flat&amp;logo=rust&amp;logoColor=white"></a>
  <img alt="Windows 10 and 11" src="https://img.shields.io/badge/Windows-10%2F11-0078D4?style=flat&amp;logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCI%2BPHBhdGggZmlsbD0iI2ZmZiIgZD0iTTEgMy4yIDEwLjYgMS44djkuNEgxVjMuMlptMTEgNy45VjEuNkwyMyAwdjExLjFIMTJaTTEgMTIuOGg5LjZ2OS40TDEgMjAuOHYtOFptMTEgMGgxMXYxMS4yTDEyIDIyLjR2LTkuNloiLz48L3N2Zz4%3D">
  <img alt="macOS 14 or later" src="https://img.shields.io/badge/macOS-14%2B-000000?style=flat&amp;logo=apple&amp;logoColor=white">
  <a href="LICENSE"><img alt="License: GPL-3.0-or-later" src="https://img.shields.io/github/license/dccif/KeySteer?label=License&amp;style=flat"></a>
</p>

<p align="center">
  <img src="assets/brand/keysteer-wordmark.webp" alt="KeySteer" width="760">
</p>

<p align="center">
  <strong>从点击到分屏，把整个工作区交给键盘。</strong>
</p>

<p align="center">
  <sub>语言 / Language · <strong>简体中文</strong> · <a href="README.en.md">English</a></sub>
</p>

KeySteer 是 Windows 和 macOS 上的原生键盘操控工具。用 `hjkl` 移动和点击，用标签定位界面，再用 Window 模式移动、分屏、组合窗口。少一些来回伸手，多一些连贯操作。

[下载体验](https://github.com/dccif/KeySteer/releases/latest) · [快速上手](https://dccif.github.io/KeySteer/guide/getting-started) · [Window 操作指南](https://dccif.github.io/KeySteer/modes/window) · [在线模拟器](https://dccif.github.io/KeySteer/editor/)

## 功能

- **Window 窗口操控**：`Alt+W` 进入，移动、缩放、居中、跨屏都在键盘上完成。
- **Quick 与 Editor 布局**：单窗口半屏定位，多窗口自动排列，再按需要分配空间。
- **Tabs 与工作区预设**：把窗口收成持续存在的标签组，保存布局和标签模板，下次继续使用。
- **音频控制**：整理窗口时顺手调节应用或系统音量、切换输出设备；平台要求见 [Window 指南](docs/modes/window.md)。

- **Normal**：`hjkl` vim风格移动鼠标。
- **长按,拖拽**：可将鼠标左/中/右键转为按下状态，适合拖拽。
- **Grid**：快速定位二键组合。
- **Recursive Grid**：区域持续递归细分。
- **UI Hint**：辅助功能，OCR，并行异步扫描为按钮、链接、菜单和输入框显示可键入标签。
- **多显示器**：`Primary+S` 切换到下一块显示器。
- **外观与配置**：`Grid`/`Hint` 标签样式、指示器等可通过 TOML 调整。

`Primary` 是跨平台写法：macOS 为 `Command`，Windows 为 `Alt`，为保持键盘位置的一致性。 它可以在 `[key_aliases]` 中改成你习惯的实体按键。

## 性能

**按键处理路径平均 151 ns**，来自 **0.9.21 开发机测试**，即 **0.151 μs / 0.000151 ms**。让每次操作更加无感

## 两条路线，马上开始

| 先试什么 | 默认操作顺序 |
| --- | --- |
| 移动与点击 | `Primary+E` → `H/J/K/L` → `;` → `Esc` |
| 移动一个窗口 | `Primary+W` → 松开入口键 → `H/J/K/L` → `Q` |
| 排列多个窗口 | `Primary+W` → `E`（立即排列）→ `Z` 可撤销 |

`A` 快速分屏、`T` 组合标签、`R` 恢复预设。完整介绍见 [Window 操作指南](docs/modes/window.md)。

### Window：窗口听键盘指挥

`H/J/K/L` 移动，`S` 切换缩放，`C` 居中，`F` 循环最大化／最小化／恢复；`V` 组合键调音量，`X` 关闭当前窗口。

https://github.com/user-attachments/assets/a1f38691-f6cf-45bf-b264-03a1a70cd104

### Quick：按 A，快速分屏

从 Window 按 `A` 进入，方向键安排窗口，再按同方向切换比例；`split_ratios` 可自定义，`Q` 返回 Window。

https://github.com/user-attachments/assets/e9a278cd-a05a-4931-80cc-d2f832142296

### Editor：按 E，自动平铺再微调

`E` 立即平铺；输入 `1`、`2` 交换窗口，`Shift+方向` 切分区域，`Ctrl+方向` 移动分割线，方向键调整布局。

https://github.com/user-attachments/assets/e21b68e3-f970-4e47-b5a2-fec80720b43b

### Tabs：同名程序，自动收成一组

从 Window 按 `T` 首次进入，按同名程序自动分组。`Tab` / `Shift+Tab` 切换组内窗口，方向键调整标签组位置。

https://github.com/user-attachments/assets/2c1b894d-7402-4b57-9fd1-da03f5bd81a9

### 保存：布局与标签组，都能记下来

在 Editor 或 Tabs 中按 `Ctrl+S`，输入备注后按 `Enter` 保存；备注也可留空，自动生成名称。保存的是布局和分组安排，不是需要启动的应用名单。

https://github.com/user-attachments/assets/9965dbb6-ed80-4071-b636-563563b83b7d

### key_help：忘了快捷键？看一眼就好

提示面板展示当前可用的按键和动作。在相应模式的 bindings 中配置 `"?" = "key_help"`，即可用问号切换显示；绑定存在才启用，示例也可换成其他键。视频通过模拟器的提示预览按钮执行同一动作。

https://github.com/user-attachments/assets/9c28c826-d465-4d47-b671-7a275c282a76

### 配置与模拟器：看着完整键盘改键

托盘右键菜单 → **Configuration & Simulator...**，跳转浏览器打开（注意网络连接）。点击完整键盘中的按键，再选择动作；例如 `W → move_up`、`A → move_left`。入口菜单为操作示意，键位编辑使用实际项目模拟器；实际行为以程序为准。

https://github.com/user-attachments/assets/fbb5f750-fcda-4d3f-92da-1f7667387927

### Normal

键盘移动、速度修饰、滚动与点击。

https://github.com/user-attachments/assets/10c990c4-903c-49fb-b8d7-5441430d3496

### Grid

一级大标签、二级预览与二键快速定位。

https://github.com/user-attachments/assets/4ecb749e-d770-43c1-907a-e55a4144a9ca

### Recursive Grid

逐层细分、回退与精确定位。

https://github.com/user-attachments/assets/cb399755-5cde-40a0-ba64-d00c7e581cc6

### UI Hint

扫描界面元素、标签筛选与控件定位。

https://github.com/user-attachments/assets/71efcae3-eb11-46d0-aba4-0a5df5e9c80c

## 默认按键

先按 `Primary+E` 进入 `Normal`。默认配置中的 `Primary` 为：macOS `Command`、Windows 左 `Alt`；可在 `[key_aliases]` 改成自己的习惯。

| 按键 | 作用 |
| --- | --- |
| `h j k l` | 左、下、上、右移动鼠标 |
| `Caps Lock` / `Left Shift` / `v` 或 `b` | 精确 / 慢速 / 快速移动 |
| `m` / `,` | 向下 / 向上滚动 |
| `;` / `'` / `Right Shift` | 左 / 右 / 中键点击 |
| `n` | Toggle 鼠标按住状态，用于拖拽 |
| `g` / `f` / `Primary+F` | `Grid` / `Recursive Grid` / `UI Hint` |
| `Primary+S` | 切换到下一块显示器 |
| `q` 或 `Esc` | 返回 Idle |

## 配置

无需配置文件即可运行：内置 `Config::default()` 与发布的 [`keysteer.default.toml`](keysteer.default.toml) 一致。

程序优先选择数据目录中的 `keysteer.<名称>.toml` 用户配置（排除 `keysteer.default.toml`）；不存在用户配置时才读取默认 TOML，仍不存在则使用内置默认值。显式 `--config`/`-c` 始终优先。

```bash
# 校验仓库中的默认示例；带 ./ 表示当前目录的确切路径
cargo run -- --check -c ./keysteer.default.toml

# 输出当前生效的完整配置
cargo run -- --dump-config

# 检查权限、显示器、输入后端和前台应用
cargo run -- --doctor
```

详情请看[配置文件](docs/reference/configuration.md)与[模式和动作](docs/reference/modes-and-actions.md)。也可以通过[配置与模拟器](https://dccif.github.io/KeySteer/editor/)编辑键位和样式。

## 安装

请从 [GitHub Releases](https://github.com/dccif/KeySteer/releases/latest) 下载与系统和 CPU 架构对应的 ZIP。Windows 解压后运行 `KeySteer.exe`；macOS 解压后将 `KeySteer.app` 移入 `/Applications`。

如果从 GitHub Release 手动安装的 macOS 应用被 Gatekeeper 提示无法打开，请确认文件来自上述官方发布页，然后运行：

```bash
sudo xattr -cr /Applications/KeySteer.app
```
建议先授予“辅助功能”和“屏幕录制”权限后再打开

## 运行与打包

开发环境：Rust 版本以 `rust-toolchain.toml` 为准。文档站需要 Node 24+ 和 `package.json` 指定的 pnpm 版本。

```bash
cargo run
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings

pnpm install
pnpm docs:build
```

## 支持的平台

- Windows 10/11：x64、ARM64
- macOS 14+：Apple Silicon、Intel

## 许可证与版权

版权所有 © 2026 dccif。KeySteer 以 **GNU General Public License v3.0 或更高版本（GPL-3.0-or-later）** 发布，完整条款见 [LICENSE](LICENSE)。

你可以使用、研究、修改和再发布本项目；但任何**对外分发**的修改版或包含本项目的衍生作品，必须继续以 GPL 提供相应源码与相同的自由。不能把公开分发的衍生版本改成闭源专有软件。GPL 不限制仅供自己或组织内部使用、且不对外分发的私有改动。
