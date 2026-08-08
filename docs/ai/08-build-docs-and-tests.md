# 构建、打包、文档站与测试

## Cargo 结构

项目是一个 crate，同时提供：

- library：`src/lib.rs`，crate 名 `keysteer`。
- binary：`src/main.rs`，程序名 `keysteer`。

平台依赖写在 Cargo target-specific dependency 中，`cargo build --target ...` 自动选择。
release profile：`opt-level=3`、fat LTO、`codegen-units=1`、abort panic、strip symbols、
关闭 incremental/debug/overflow checks，目标是发布体积和运行性能。

## build.rs

- Windows host 构建 Windows target 时，将 `assets/icons/keysteer.ico` 和版本资源嵌入 exe。
- macOS host 构建 macOS target 时，编译 `vision_bridge.m` 和 `autostart_bridge.m`，最低
  macOS 14，并链接 Foundation/CoreGraphics/ScreenCaptureKit/ServiceManagement/Vision。
- 非原生 host 做 cross-check 时跳过必须依赖目标 SDK/resource compiler 的步骤，保证
  Rust 代码仍可检查。

## 官方打包

不要发布裸 `target/release`。

### Windows

`packaging/windows/package.ps1 [target]`：

1. `cargo build --locked --release --target`。
2. 复制图标已嵌入、GUI-subsystem 的 `KeySteer.exe`。
3. 生成 `dist/<target>/KeySteer-<target>.zip`。
4. 生成 SHA-256 文件。

支持 `x86_64-pc-windows-msvc`、`aarch64-pc-windows-msvc`。

`tools/benchmark-windows-dist.ps1 [target]` starts the unpacked
`dist/<target>/KeySteer/KeySteer.exe`, then delegates its validated Alt+E/;
left-click scenario to `tools/windows-comparison-runner/`. `-SetupKey`,
`-Key`, and `-Observe` override the scenario for another configuration. It
writes the runner CSV and a JSON sidecar containing the fixed settle delay and
process-creation time. The latter is not application-ready latency. The script
stops its launched process unless `-KeepRunning` is selected.

### macOS

`packaging/macos/package.sh [target]`：

1. 设置最低 macOS 14 并构建 release binary。
2. 创建 `KeySteer.app/Contents/{MacOS,Resources}`。
3. 从 `Info.plist.in` 注入版本/最低系统，生成 `.icns` 并嵌入。
4. 默认 ad-hoc codesign；有 Developer ID 时正式签名。
5. 可通过 notary profile 或 Apple account notarize/staple。
6. `ditto` 生成 ZIP 和 SHA-256。

支持 Apple Silicon 和 Intel。正式签名身份必须稳定，否则 Accessibility/Screen Recording
授权可能无法跨升级延续。

## GitHub Actions

根目录 `.github/workflows/ci.yml`：

- 4 target 打包矩阵：macOS arm64/x64、Windows x64/arm64。
- 可执行 target 跑 tests，所有 target 跑 clippy。
- macOS host cross-check Windows backend。
- Linux job 跑 fmt 和 shipped-config integration test。
- docs job 使用 Node 24 + pnpm 10，跑 test/typecheck/build。

`.github/workflows/release.yml` 在 `v*` tag：复用同一平台打包脚本，上传四套 ZIP/checksum，
最终生成 GitHub Release。macOS 证书和 notarization 通过 secrets 注入。

## VitePress 文档与模拟器

工具链：VitePress 1.6、Vue 3 TSX、`smol-toml`、pnpm。主要脚本：

- `pnpm docs:dev`：同步 default TOML/icon 后启动开发服务器。
- `pnpm docs:test`：Node tests，覆盖浏览器端绑定继承、配置 clone 和模拟状态。
- `pnpm docs:check`：`vue-tsc --noEmit`。
- `pnpm docs:build`：同步静态资源并生产构建。

模拟器重点是键位和 Grid/Recursive Grid/UI Hint 样式可视化，不是完整 Rust runtime。它：

- 解析/输出 TOML，支持导入和下载。
- 模拟 Mode binding inheritance 和空格分组键。
- 展示键位动作分类和 targeting overlay 外观。
- 不使用 Rust/WASM 校验器；复杂配置和最终校验交给程序/文档。

修改 Rust 默认绑定或 UI style 字段时，需要检查网页默认配置是否仍能正确解析、染色和显示。

## 测试层次

- 源文件 `#[cfg(test)]`：API parse/canonical、geometry、Mode 状态、runtime 路由、平台纯逻辑。
- `tests/integration.rs`：发布配置可解析、默认快捷键跨平台、CLI/项目不变量。
- 文档站 Node tests：轻量模拟模型，不替代 Rust tests。
- 平台原生窗口/权限/Hook 仍需要对应 OS 的实机验证。
- `tools/windows-comparison-runner/` 是独立 Windows 黑盒性能 runner；它不属于主 crate 的
  `cargo test`，需要通过自己的 `--manifest-path` 构建、测试和 Clippy。

常用完整检查：

```text
cargo fmt --check
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
pnpm docs:test
pnpm docs:check
pnpm docs:build
```

只改文档 Markdown 时不必运行所有原生 target，但应检查链接/路径和 `git diff --check`。
