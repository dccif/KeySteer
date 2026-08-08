# KeySteer

<p align="center">
  <a href="https://github.com/dccif/KeySteer/actions/workflows/pages.yml"><img alt="Page status" src="https://img.shields.io/github/actions/workflow/status/dccif/KeySteer/pages.yml?branch=main&amp;label=Page&amp;style=flat-square&amp;logo=githubactions&amp;logoColor=white"></a>
  <a href="https://github.com/dccif/KeySteer/actions/workflows/build.yml"><img alt="Build status" src="https://img.shields.io/github/actions/workflow/status/dccif/KeySteer/build.yml?branch=main&amp;label=Build&amp;style=flat-square&amp;logo=githubactions&amp;logoColor=white"></a>
  <a href="https://github.com/dccif/KeySteer/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/dccif/KeySteer?display_name=tag&amp;sort=semver&amp;label=Release&amp;style=flat-square"></a>
  <a href="rust-toolchain.toml"><img alt="Rust 1.97" src="https://img.shields.io/badge/Rust-1.97-dea584?style=flat-square&amp;logo=rust&amp;logoColor=white"></a>
  <img alt="Windows 10 and 11" src="https://img.shields.io/badge/Windows-10%2F11-0078D4?style=flat-square&amp;logo=windows11&amp;logoColor=white">
  <img alt="macOS 14 or later" src="https://img.shields.io/badge/macOS-14%2B-000000?style=flat-square&amp;logo=apple&amp;logoColor=white">
  <a href="LICENSE"><img alt="License: GPL-3.0-or-later" src="https://img.shields.io/github/license/dccif/KeySteer?label=License&amp;style=flat-square"></a>
</p>

<p align="center">
  <img src="assets/brand/keysteer-wordmark.webp" alt="KeySteer" width="760">
</p>

<p align="center">
  <strong>把鼠标交给键盘：轻量、原生、可配置。</strong>
</p>

KeySteer 是 Windows 和 macOS 上的使用键盘操控鼠标工具。 

[在线文档](https://dccif.github.io/KeySteer/)

## 功能

- **Normal**：`hjkl` vim风格移动鼠标。
- **点击，长按 Toggle**：普通点击在物理键按下沿立即完成；按住可将左/中/右键转为 Toggle 保持按下，适合拖拽。
- **Grid**：快速定位二键组合。
- **Recursive Grid**：区域持续递归细分。
- **UI Hint**：为按钮、链接、菜单和输入框显示可键入标签；macOS 支持 Accessibility Tree、Vision 和 Hybrid，Windows 使用 UI Automation。
- **多显示器**：`Primary+S` 切换到下一块显示器；Grid 与 Recursive Grid 可保留当前定位路径。
- **外观与配置**：Grid/Hint 标签样式、光标旁模式和点击指示器等可通过 TOML 调整。

## 视频演示

### Normal

键盘移动、速度修饰、滚动与点击。

https://github.com/user-attachments/assets/255ba0b5-57c6-4a2b-ae68-ffc1bf06ad6c

### Grid

一级大标签、二级预览与二键快速定位。

https://github.com/user-attachments/assets/98e63544-89d2-464d-823b-3a0d5712b49a

### Recursive Grid

逐层细分、回退与精确定位。

https://github.com/user-attachments/assets/874ed096-1ab0-4228-879e-e53efb0b1a55

### UI Hint

扫描界面元素、标签筛选与控件定位。

https://github.com/user-attachments/assets/11aff61b-acd3-4e2b-bf27-f089b79f430b

## 默认按键

先按 `Primary+E` 进入 Normal。默认配置中的 `Primary` 为：macOS Command、Windows 左 Alt、Linux Ctrl；可在 `[key_aliases]` 改成自己的习惯。

| 按键 | 作用 |
| --- | --- |
| `h j k l` | 左、下、上、右移动鼠标 |
| `Caps Lock` / `Left Shift` / `v` 或 `b` | 精确 / 慢速 / 快速移动 |
| `m` / `,` | 向下 / 向上滚动 |
| `;` / `'` / `Right Shift` | 左 / 右 / 中键点击 |
| `n` | Toggle 鼠标按住状态，用于拖拽 |
| `g` / `f` / `Primary+F` | Grid / Recursive Grid / UI Hint |
| `Primary+S` | 切换到下一块显示器 |
| `q` 或 `Esc` | 返回 Idle |

## 配置

无需配置文件即可运行：内置 `Config::default()` 与发布的 [`keysteer.default.toml`](keysteer.default.toml) 完全一致，后者只是带注释的可复制示例。

自动发现配置时，程序优先选择数据目录中按文件名排序的 `keysteer.<名称>.toml` 用户配置（排除 `keysteer.default.toml`）；不存在用户配置时才读取默认 TOML，仍不存在则使用内置默认值。显式 `--config`/`-c` 始终优先。

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

该命令会递归清除应用包的扩展属性；不要对来源不明的应用使用。随后重新打开 KeySteer，并按系统提示授予“辅助功能”和“屏幕录制”权限。

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
- Linux：暂未支持

## 许可证与版权

版权所有 © 2026 dccif。KeySteer 以 **GNU General Public License v3.0 或更高版本（GPL-3.0-or-later）** 发布，完整条款见 [LICENSE](LICENSE)。

你可以使用、研究、修改和再发布本项目；但任何**对外分发**的修改版或包含本项目的衍生作品，必须继续以 GPL 提供相应源码与相同的自由。不能把公开分发的衍生版本改成闭源专有软件。GPL 不限制仅供自己或组织内部使用、且不对外分发的私有改动。
