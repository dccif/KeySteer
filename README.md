# KeySteer

<p align="center">
  <img src="assets/brand/keysteer-wordmark.webp" alt="KeySteer" width="760">
</p>

<p align="center">
  <strong>把鼠标交给键盘：轻量、原生、可配置。</strong>
</p>

KeySteer 是 Windows 和 macOS 上的原生键盘鼠标工具。它用键盘驱动指针、滚轮和鼠标按钮，也提供 Grid、Recursive Grid 与 UI Hint 三种定位方式。输入、覆盖层、辅助功能扫描和显示帧同步都直接使用系统 API；不带 WebView，也不需要浏览器运行时。

完整使用文档在 [`docs/`](docs/index.md)，其中包含[快速上手](docs/guide/getting-started.md)、[配置参考](docs/reference/configuration.md)、[动作参考](docs/reference/modes-and-actions.md)和[开发文档](docs/development/architecture.md)。

## 功能

- **Normal**：`hjkl` 连续移动，支持平滑 S 曲线加速、精确/慢速/快速修饰、滚动、点击和拖拽。
- **立即点击，长按 Toggle**：普通点击在物理键按下沿立即完成；按住可将左/中/右键转为 Toggle 保持按下，适合拖拽。
- **Grid**：快速定位二键组合。
- **Recursive Grid**：区域持续递归细分。
- **UI Hint**：为按钮、链接、菜单和输入框显示可键入标签；macOS 支持 Accessibility Tree、Vision 和 Hybrid，Windows 使用 UI Automation。
- **多显示器**：`Primary+S` 切换到下一块显示器；Grid 与 Recursive Grid 可保留当前定位路径。
- **原生帧同步**：连续移动以实际显示帧经过时间积分；Windows 等待目标输出的 DXGI 垂直同步，macOS 使用 DisplayLink，不依赖固定刷新率或轮询。
- **外观与配置**：深浅主题、Grid/Hint 标签样式、光标旁模式和点击指示器均可通过 TOML 调整。

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

`pointer.smooth_acceleration = true` 是默认值；它在加速起止使用柔和的 S 曲线，松开最后一个方向键仍立即停止。普通点击的按键在仍被物理按住时会在光标底部显示对应颜色，不会为了提示而延迟或拆分点击。

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

详情请看[配置文件](docs/reference/configuration.md)与[模式和动作](docs/reference/modes-and-actions.md)。也可以通过 VitePress 文档站的配置与模拟器编辑键位和样式。

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
