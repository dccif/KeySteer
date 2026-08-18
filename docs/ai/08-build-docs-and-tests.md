# 构建、打包、文档站与测试

## 优化构建档位（2026-08）

- 通用发布保留目标默认 CPU baseline；workflow 先按 `Cargo.toml` 定向同步 Cargo.lock 中的 `keysteer` 根包版本，不更新第三方依赖，后续测试与打包始终使用 `--locked`。打包从 commit 生成 `SOURCE_DATE_EPOCH`，Windows 发布入口同时传递 `/Brepro`。
- `tools/build-native.ps1` / `tools/build-native.sh` 仅构建 host architecture，使用独立 `target-native/` 和 `-C target-cpu=native`。
- `perf-probe` 是 opt-in feature；正式通用发布默认不启用。启用时热路径只向固定有界队列
  非阻塞写入，文件 I/O 由测试线程执行。mimalloc 在 Windows x64 A/B 中未通过启动 p99
  门禁，未保留依赖或 feature。
- `.github/workflows/build.yml` 统一承载 CI 与发布且仅手动触发：可选择 Windows/macOS 打包发布或仅运行检查；Miri 也是手动选项。
- PGO 不在缺少代表性整进程训练语料时启用；必须先由对应架构原生 runner 产出稳定训练集，并通过同一 p99/内存门禁。

性能变更使用独立 target/worktree A/B：关键 p99 回退不得超过 2%；目标延迟改善至少 3%或内存下降至少 5%才保留。`tools/benchmark-windows-dist.ps1` 记录启动与 working set/private bytes/handles/threads；真实 ready 延迟要求被测二进制启用 `perf-probe`，并通过 `KEYSTEER_PERF_PROBE` 输出生命周期 JSONL marker。普通发行包的 `--check` 结果只标记为 config-check，不冒充 ready 延迟。

更新检查保留系统 `native-tls`：Windows x64 同一 release profile 的 A/B 中，Rustls+WebPKI
使 EXE 从 2,652,672 B 增至 3,573,248 B（+34.7%），同内容 ZIP 从 1,283,773 B 增至
1,758,220 B（+37.0%）。TLS 只在手动检查更新时创建，不进入输入、overlay 或 Idle 热路径。

## Cargo 结构

项目是一个 crate，同时提供：

- library：`src/lib.rs`，crate 名 `keysteer`。
- binary：`src/main.rs`，程序名 `keysteer`。

平台依赖写在 Cargo target-specific dependency 中，`cargo build --target ...` 自动选择。
release profile：`opt-level=3`、fat LTO、`codegen-units=1`、abort panic、strip symbols、
关闭 incremental/debug/overflow checks，目标是发布体积和运行性能。

`Cargo.toml` 的 `rust-version` 是最低 Rust 版本；`rust-toolchain.toml` 固定同一
`1.97` toolchain。发布工作流在每个原生 job 显式安装它，不能依赖 GitHub runner 预装
的 `stable` 版本。

## build.rs

- Windows host 构建 Windows target 时，将 `assets/icons/keysteer.ico` 和版本资源嵌入 exe。
- macOS host 构建 macOS target 时，编译 `vision_bridge.m` 和 `autostart_bridge.m`，最低
  macOS 14，并链接 Foundation/CoreGraphics/ScreenCaptureKit/ServiceManagement/Vision。
- 非原生 host 做 cross-check 时跳过必须依赖目标 SDK/resource compiler 的步骤，保证
  Rust 代码仍可检查。

## 官方打包

不要发布裸 `target/release`。

完整平台发布时，workflow 从 `Cargo.toml` 读取版本，并由
`tools/compose-release-notes.sh` 精确提取 `docs/releases/index.md` 中同名的
`## <version>` 条目。该双语内容和 `.github/release-notes.md` 的固定安装提示会置于
GitHub 自动生成的 commit/PR notes 之前；版本条目缺失、重复或为空时禁止创建 Release。
普通 CI 也会运行同一提取器，确保版本更新与文档条目在合并前保持同步。

### Windows

`packaging/windows/package.ps1 [target]`：

1. `cargo build --locked --release --target`。
2. 复制图标已嵌入、GUI-subsystem 的 `KeySteer.exe` 与
   `keysteer.default.toml` 到 `dist/<target>/KeySteer/`。
3. 生成包含该目录的 `dist/<target>/KeySteer-v<version>-<target>.zip`。
4. 不生成独立 checksum 附件；面向用户的发布资产只有平台 ZIP。

支持 `x86_64-pc-windows-msvc`、`aarch64-pc-windows-msvc`。

`tools/benchmark-windows-dist.ps1 [target]` starts the unpacked
`dist/<target>/KeySteer/KeySteer.exe` and writes in-memory startup/resource
samples to JSON after the measured interval. With `-UsePerfProbe`, the binary
must be built with `--features perf-probe` and startup samples are the emitted
`backend_started` elapsed time. Without it, samples are explicitly named
`config_check_process_ms`. The script stops its launched process unless
`-KeepRunning` is selected. `-Executable` and `-ConfigPath` allow equivalent
sampling directly from an isolated A/B target directory before packaging.

### macOS

`packaging/macos/package.sh [target]`：

1. 设置最低 macOS 14 并构建 release binary。
2. 创建 `KeySteer.app/Contents/{MacOS,Resources}`。
3. 从 `Info.plist.in` 注入版本/最低系统，生成 `.icns` 并嵌入。
4. 默认 ad-hoc codesign；有 Developer ID 时正式签名。
5. 可通过 notary profile 或 Apple account notarize/staple。
6. 将 `.app` 和 `keysteer.default.toml` 放入 `KeySteer/`，再由 `ditto`
   生成带 Cargo 版本的 ZIP；不生成独立 checksum 附件。

支持 Apple Silicon 和 Intel。正式签名身份必须稳定，否则 Accessibility/Screen Recording
授权可能无法跨升级延续。

## GitHub Actions

根目录 `.github/workflows/build.yml` 选择 `checks` 时运行检查任务：

- 4 target 打包矩阵：macOS arm64/x64、Windows x64/arm64。
- 可执行 target 跑 tests，所有 target 跑 clippy。
- macOS host cross-check Windows backend。
- Linux job 跑 fmt 和 shipped-config integration test。
- docs job 使用 Node 24 + pnpm 10，跑 test/typecheck/build。

同一 workflow 手动运行时会构建所选平台；选择 `checks` 时仅运行上述检查，选择 `all` 时，待四个平台的
ZIP 都成功后，以 Cargo 版本创建 `v<version>` GitHub Release，并且只上传四个 ZIP。
只构建单个平台时保留 workflow artifact，但不创建不完整的 Release。macOS 证书和
notarization 通过 secrets 注入。创建 Release 时把 `.github/release-notes.md` 的固定安装
提示置于 GitHub 自动生成的变更说明之前；该文件必须保留未 notarize macOS 下载包所需的
`sudo xattr -cr /Applications/KeySteer.app` 指引和来源安全提示。

每个 target 的 workflow artifact 也独立上传：Windows x64/ARM64、macOS Apple
Silicon/Intel 各一份；不会把不同架构放入同一个 ZIP 或 artifact。

`.github/workflows/pages.yml` 仅由 `workflow_dispatch` 手动触发；push 不会自动部署
GitHub Pages。需要更新线上文档时，在 Actions 页面运行 `Deploy documentation`。

`.github/ISSUE_TEMPLATE/` 提供错误报告、功能建议和配置/按键问题三种 Issue Form；错误
报告收集平台、架构、版本、受影响功能、复现步骤和脱敏 TOML，避免把不完整的环境信息留给
维护者猜测。空白 Issue 仍允许创建。不要在模板中预设 labels，因为 GitHub 只会添加仓库中
已经存在的 labels。

## VitePress 文档与模拟器

用户文档以中文根路径和英文 `/en/` 路径并行发布：中文源文件保留在 `docs/`，英文源文件在
`docs/en/`，不要把 `docs/ai/` 的维护者资料复制到任一面向用户的侧栏。`docs/.vitepress/config.mts`
在 `locales` 与 `themeConfig.locales` 中维护两种语言的导航、侧栏、搜索、页内目录和上下页文案；
两套导航各有一个语言菜单。新增或移动用户页面时，必须同时新增/移动其 `docs/en/` 对应页面并更新两套链接，
避免语言切换后落到不存在的地址。

工具链：VitePress 1.6、Vue 3 TSX、`smol-toml`、pnpm。主要脚本：

- `pnpm docs:dev`：同步 default TOML/icon 和 Release 元数据后启动开发服务器。
- `pnpm docs:test`：Node tests，覆盖浏览器端绑定继承、配置 clone 和模拟状态。
- `pnpm docs:check`：先同步生成文件，再执行 `tsc --noEmit`。当前文档组件全部是 `.ts`/`.tsx`，不使用 Vue SFC；
  直接使用仓库固定的 TypeScript 可避免 `vue-tsc` 对编译器私有子路径的版本耦合。
- `pnpm docs:build`：同步静态资源和 Release 元数据后生产构建。

`scripts/sync-doc-assets.mjs` 以 `Cargo.toml` 的 `[package].version` 为唯一版本来源，生成
被忽略的 `docs/.vitepress/generated/release.ts`。下载组件用它构造当前 tag 和四个
`KeySteer-v<version>-<target>.zip` 直链，因此只要修改 Cargo 版本并运行文档构建，页面
就会自动指向同版本的 GitHub Release；不需要手改前端版本号。

模拟器重点是键位和 Grid/Recursive Grid/UI Hint 样式可视化，不是完整 Rust runtime。它：

- 解析/输出 TOML，支持导入和下载。
- 可从 KeySteer 菜单接收 zlib + Base64URL fragment；页面立即清除 fragment，并只在浏览器本地解码。
- 模拟 Mode binding inheritance 和空格分组键。
- 展示键位动作分类和 targeting overlay 外观。
- 不使用 Rust/WASM 校验器；复杂配置和最终校验交给程序/文档。

修改 Rust 默认绑定或 UI style 字段时，需要检查网页默认配置是否仍能正确解析、染色和显示。

## 测试层次

- 源文件 `#[cfg(test)]`：API parse/canonical、geometry、Mode 状态、runtime 路由、平台纯逻辑。
- `tests/integration.rs`：发布配置可解析、默认快捷键跨平台、CLI/项目不变量。
- 文档站 Node tests：轻量模拟模型，不替代 Rust tests。
- 平台原生窗口/权限/Hook 仍需要对应 OS 的实机验证。
- `tools/benchmark-windows-dist.ps1` 是 Windows 黑盒整进程采样入口；它不属于主 crate 的
  `cargo test`，需在对应架构真机上分别运行基线与候选包。

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
