# KeySteer

<p align="center">
  <a href="https://github.com/dccif/KeySteer/actions/workflows/pages.yml"><img alt="Page status" src="https://img.shields.io/github/actions/workflow/status/dccif/KeySteer/pages.yml?branch=main&amp;label=Page&amp;style=flat&amp;logo=github&amp;logoColor=white"></a>
  <a href="https://github.com/dccif/KeySteer/actions/workflows/build.yml"><img alt="Build status" src="https://img.shields.io/github/actions/workflow/status/dccif/KeySteer/build.yml?branch=main&amp;label=Build&amp;style=flat&amp;logo=github&amp;logoColor=white"></a>
  <a href="https://github.com/dccif/KeySteer/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/dccif/KeySteer?display_name=tag&amp;sort=semver&amp;label=Release&amp;style=flat"></a>
  <a href="https://github.com/dccif/KeySteer/blob/main/rust-toolchain.toml"><img alt="Rust 1.100" src="https://img.shields.io/badge/Rust-1.100-dea584?style=flat&amp;logo=rust&amp;logoColor=white"></a>
  <img alt="Windows 10 and 11" src="https://img.shields.io/badge/Windows-10%2F11-0078D4?style=flat&amp;logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCI%2BPHBhdGggZmlsbD0iI2ZmZiIgZD0iTTEgMy4yIDEwLjYgMS44djkuNEgxVjMuMlptMTEgNy45VjEuNkwyMyAwdjExLjFIMTJaTTEgMTIuOGg5LjZ2OS40TDEgMjAuOHYtOFptMTEgMGgxMXYxMS4yTDEyIDIyLjR2LTkuNloiLz48L3N2Zz4%3D">
  <img alt="macOS 14 or later" src="https://img.shields.io/badge/macOS-14%2B-000000?style=flat&amp;logo=apple&amp;logoColor=white">
  <a href="https://github.com/dccif/KeySteer/blob/main/LICENSE"><img alt="License: GPL-3.0-or-later" src="https://img.shields.io/github/license/dccif/KeySteer?label=License&amp;style=flat"></a>
</p>

<p align="center">
  <img src="assets/brand/keysteer-wordmark.webp" alt="KeySteer" width="760">
</p>

<p align="center">
  <strong>从点击到分屏，把整个工作区交给键盘。</strong>
</p>

<h3 align="center">
  <strong>简体中文</strong> · <a href="https://github.com/dccif/KeySteer/blob/main/README.en.md">English</a>
</h3>

KeySteer 是 Windows 和 macOS 上的原生键盘操控工具。用键盘移动鼠标、输入标签定位控件，也能分屏、平铺窗口并保存布局。

[下载体验](https://github.com/dccif/KeySteer/releases/latest) · [在线文档](https://dccif.github.io/KeySteer/guide/getting-started) · [在线模拟器](https://dccif.github.io/KeySteer/editor/)

**目录** · [功能](#功能) · [演示](#演示) · [安装](#安装) · [上手](#快速上手) · [配置](#配置) · [性能](#性能) · [开发](#运行与打包)

## 功能

- **UI Hint 界面识别**：跨屏、跨程序显示控件标签，支持文字／简拼搜索、复制信息与取色。
- **Normal 键盘鼠标**：Vim 风格移动、点击、滚动与拖拽，支持速度调节和跨屏操作。
- **Grid、Recursive Grid 与 Bisect**：两键快速定位、逐层细分到小目标，或用 `H/J/K/L` 逐次二分。
- **Window、Quick 与 Editor**：移动、缩放、分屏和平铺窗口，也能调节音量。
- **Tabs 与工作区**：把窗口收成标签组，保存布局，下次继续使用。
- **外观与配置**：自由调整按键、速度与样式，在网页模拟器中预览。

## 演示

<details>
<summary>鼠标与界面定位：Normal、Grid、Recursive Grid、Bisect、UI Hint</summary>

### Normal

https://github.com/user-attachments/assets/10c990c4-903c-49fb-b8d7-5441430d3496

### Grid

https://github.com/user-attachments/assets/4ecb749e-d770-43c1-907a-e55a4144a9ca

### Recursive Grid

https://github.com/user-attachments/assets/cb399755-5cde-40a0-ba64-d00c7e581cc6

### Bisect：每次缩小一半

https://github.com/user-attachments/assets/630c7e04-c49b-42fc-aaf5-3fbd55b3cb8e

### UI Hint

https://github.com/user-attachments/assets/71efcae3-eb11-46d0-aba4-0a5df5e9c80c

### UI Hint 搜索：简拼、复制与取色

https://github.com/user-attachments/assets/84d0872d-81f6-4e42-aa7f-767061cef646

</details>

<details>
<summary>窗口管理：移动、分屏、平铺、标签与保存</summary>

### Window：窗口听键盘指挥

https://github.com/user-attachments/assets/a1f38691-f6cf-45bf-b264-03a1a70cd104

### Quick：按 A，快速分屏

https://github.com/user-attachments/assets/e9a278cd-a05a-4931-80cc-d2f832142296

### Editor：按 E，自动平铺再微调

https://github.com/user-attachments/assets/e21b68e3-f970-4e47-b5a2-fec80720b43b

### Tabs：同名程序，自动收成一组

https://github.com/user-attachments/assets/2c1b894d-7402-4b57-9fd1-da03f5bd81a9

### 保存：布局与标签组，都能记下来

https://github.com/user-attachments/assets/9965dbb6-ed80-4071-b636-563563b83b7d

</details>

<details>
<summary>按键提示与图形化配置</summary>

### key_help：忘了快捷键？看一眼就好

https://github.com/user-attachments/assets/9c28c826-d465-4d47-b671-7a275c282a76

### 配置与模拟器：看着完整键盘改键

https://github.com/user-attachments/assets/fbb5f750-fcda-4d3f-92da-1f7667387927

</details>

## 安装

支持 **Windows 10/11（x64、ARM64）** 和 **macOS 14+（Apple Silicon、Intel）**。

从 [GitHub Releases](https://github.com/dccif/KeySteer/releases/latest) 下载：Windows 解压后运行 `KeySteer.exe`；macOS 将 `KeySteer.app` 移入 `/Applications`，并按[授权指南](https://dccif.github.io/KeySteer/guide/macos)完成设置。Windows x64 默认使用 AVX2，旧 CPU 可选择兼容版。

**Windows（Scoop）**

```powershell
scoop bucket add keysteer https://github.com/dccif/KeySteer.git
scoop install keysteer/keysteer
```

**macOS（Homebrew）**

```bash
brew tap dccif/keysteer https://github.com/dccif/KeySteer.git
brew install --cask dccif/keysteer/keysteer
```

**macOS 首次运行**：在「系统设置 → 隐私与安全」中授予 KeySteer 控制权限（「设备控制和数据访问」或「辅助功能」）；UI Hint 视觉识别还需录屏权限。授权后重启应用。

若从官方 Releases 下载后提示无法打开，运行：

```bash
sudo xattr -cr /Applications/KeySteer.app
```

## 快速上手

无需配置文件即可使用。操作入门与键位见[快速上手](https://dccif.github.io/KeySteer/guide/getting-started)，窗口布局与标签分组见[窗口管理指南](https://dccif.github.io/KeySteer/window-management/)。

## 配置

用[网页模拟器](https://dccif.github.io/KeySteer/editor/)调整键位和外观，或直接编辑 TOML；保存与加载方式见[配置文档](https://dccif.github.io/KeySteer/reference/configuration)。

## 性能

**纳秒级核心处理，轻量常驻。**

![核心按键派发 157 ns；移动帧计算 15 ns；启动待机内存约 12 MiB](assets/performance/windows-0.12.3-zh.svg)

（0.12.3 / Windows；核心 CPU 数据按事件／帧批量计时后均摊，不含系统输入、识别与显示。[完整数据与测量方法](https://dccif.github.io/KeySteer/performance)）

## 运行与打包

源码运行、测试与打包见[开发流程](https://dccif.github.io/KeySteer/development/workflow)，测量方法与约束见[安全与性能](https://dccif.github.io/KeySteer/development/safety-and-performance)。

## 许可证与版权

版权所有 © 2026 dccif。KeySteer 以 **GNU General Public License v3.0 或更高版本（GPL-3.0-or-later）** 发布，完整条款见 [LICENSE](https://github.com/dccif/KeySteer/blob/main/LICENSE)。
