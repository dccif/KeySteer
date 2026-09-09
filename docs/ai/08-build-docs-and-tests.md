# 构建、打包、文档站与测试

Rust 与 TypeScript 使用同一 `tests/fixtures/window-layouts-v1.kslayout` 做逐字节往返，覆盖二进制布局文件互导；v2 handoff 测试同时校验按键 source、布局 bytes 和 URL 清理，runtime 测试验证重新打开 R 后读到外部替换的文件。

布局收藏的 API 测试覆盖 9/4 区域与窗口数量不匹配、空区域及排序、备注/隐私字段；`app/layout_store` 测试覆盖二进制往返、逐字节截断、版本、持久化及写入失败不覆盖。runtime 测试覆盖 Ctrl+S、原生输入放行、R→编号恢复和会话取消后的迟到结果。Windows ignored `native_note_dialog_preserves_unicode_and_cancels_owned_windows` 在交互桌面创建并清理自有对话框，验证 Unicode 保存和取消；macOS 编译检查不能替代实机 IME 验证。网页 `window-presets.test.ts` 覆盖独立浏览器布局库与恢复撤销。

## 优化构建档位（2026-08）

- 通用发布保留目标默认 CPU baseline；版本变更应在提交前同步 Cargo.lock 中的 `keysteer` 根包版本。为避免只修改 Cargo.toml 导致正式打包失败，workflow 在每个原生 matrix runner 上先读取 manifest 版本，并用 `cargo update --package keysteer --precise <version>` 定向同步根包条目，不主动升级第三方依赖；后续打包仍使用 `--locked`。打包从 commit 生成 `SOURCE_DATE_EPOCH`，Windows 发布入口同时传递 `/Brepro`。
- `tools/build-native.ps1` / `tools/build-native.sh` 仅构建 host architecture，使用独立 `target-native/` 和 `-C target-cpu=native`。
- `perf-probe` 是 opt-in 诊断 feature；正式通用发布和性能验收均不启用。启用时热路径只向固定有界队列
  非阻塞写入，文件 I/O 由诊断线程执行。它用于关联生命周期事件，不代表未插桩 release 延迟。mimalloc 在 Windows x64 A/B 中未通过启动 p99
  门禁，未保留依赖或 feature。
- `.github/workflows/build.yml` 只承载手动打包和发布，可选择 Windows、macOS 或全部平台，并可选择只保留 artifact、正式发布或预发布；格式化、测试、Clippy 和性能验收在提交前本地运行，不消耗发布 Actions 时长。开启发布时无论平台下拉值为何都构建全部目标，防止产生不完整 Release。
- PGO 不在缺少代表性整进程训练语料时启用；必须先由对应架构原生 runner 产出稳定训练集，并通过同一 p99/内存门禁。

性能变更使用独立 target/worktree A/B：关键 p99 回退不得超过 2%；目标延迟改善至少 3%或内存下降至少 5%才保留。`cargo bench --features benchmark-hooks --bench core_hot_paths` 使用 release profile 和系统分配器；`benchmark-hooks` 只改变内部构造器的可见性，不启用探针、替代算法或运行参数。Normal 与每个 Hint 规模固定使用 20k 样本，精准候选验收还需进行多轮交替 A/B。`tools/benchmark-windows-dist.ps1` 记录进程与资源样本；`-UsePerfProbe` 产生的生命周期 JSONL 仅用于诊断 ready 顺序，并明确标记为 instrumented。普通发行包的 `--check` 结果只标记为 config-check，两者都不能冒充未插桩的真实 ready 延迟。

更新检查使用系统 `native-tls`。TLS 只在手动检查更新时创建，不进入输入、overlay 或 Idle 热路径。

## Cargo 结构

Windows 和 macOS 字符观察的局部基准均使用
`cargo bench --features benchmark-hooks --bench core_hot_paths -- --character-capture`。
它以 release profile、系统分配器和 20k 样本交替测量未过滤转换、无字符绑定，以及绑定 `?`
时普通 H 键的候选排除路径，不安装 Hook、不注入按键。未过滤参照也使用当前优化过的
状态构造器，因此它是字符观察成本的保守对比，不代表完整按键到指针的响应延迟。
macOS 的同名入口使用本地构造、从不投递的 CGEvent，对比原始字段读取/解码、需求关闭、
绑定 `?` 后普通 H 的共享 ASCII 过滤。它不调用 TIS、不查询布局、不注入输入。
macOS 目前只有交叉编译验证，尚无原生延迟数据；不能把跳过解码等同于整体更快。
Windows 显式原生正确性检查为
`cargo test --lib native_character_layout_probe -- --ignored --nocapture`，要求已加载美式布局；
测试只读取布局，不加载或切换用户的键盘布局。

项目是一个 crate，同时提供：

- library：`src/lib.rs`，crate 名 `keysteer`。
- binary：`src/main.rs`，程序名 `keysteer`。

平台依赖写在 Cargo target-specific dependency 中，`cargo build --target ...` 自动选择。
release profile：`opt-level=3`、fat LTO、`codegen-units=1`、abort panic、strip symbols、
关闭 incremental/debug/overflow checks，目标是发布体积和运行性能。

`Cargo.toml` 的 `rust-version` 是最低 Rust 版本；`rust-toolchain.toml` 选择 rustup 当前
`stable`。发布工作流在每个原生 job 显式安装 stable，保证同一次工作流的所有步骤使用同一
工具链；MSRV 验证需要另行显式选择 `1.98`。

## build.rs

- Windows host 构建 Windows target 时，将 `assets/icons/keysteer.ico` 和版本资源嵌入 exe。
- macOS host 构建 macOS target 时，只编译原生能力所需的 `vision_bridge.m` 和
  `autostart_bridge.m`，最低 macOS 14，并链接
  AppKit/Foundation/CoreGraphics/ScreenCaptureKit/ServiceManagement/Vision。
- 非原生 host 做 cross-check 时跳过必须依赖目标 SDK/resource compiler 的步骤，保证
  Rust 代码仍可检查。

## 官方打包

不要发布裸 `target/release`。

完整平台发布时，workflow 从 `Cargo.toml` 读取版本，并由
`tools/compose-release-notes.sh` 精确提取 `docs/releases/index.md` 中同名的
`## <version>` 条目。该双语内容和 `.github/release-notes.md` 的固定安装提示会置于
GitHub 自动生成的 commit/PR notes 之前；版本条目缺失、重复或为空时禁止创建 Release。
Release 汇总 job 会运行同一提取器，确保版本条目缺失时不发布。

### Windows

`packaging/windows/package.ps1 [target]`：

1. `cargo build --locked --release --target`。
2. 使用 `KEYSTEER_SIGNING_PFX` + `KEYSTEER_SIGNING_PASSWORD` 或
   `KEYSTEER_SIGNING_THUMBPRINT` 对 EXE 做 SHA-256 Authenticode 签名和 RFC 3161 时间戳；
   SignTool 返回失败会立即终止，`-RequireSigning` 禁止产生不可自动安装的正式包。打包阶段不把
   自签证书加入 runner 的 Root；安装更新时由 KeySteer 校验 Authenticode 摘要并比较当前程序与
   候选程序的叶证书指纹，因此最终用户也不需要安装 CER。
3. 复制图标已嵌入、GUI-subsystem 的 `KeySteer.exe` 与
   `keysteer.default.toml` 到 `dist/<target>/KeySteer/`。
4. 生成包含该目录的 `dist/<target>/KeySteer-v<version>-<target>.zip`；自动更新器下载同一个
   便携 ZIP，严格提取其中已签名的 `KeySteer.exe` 后再验证和安装。
5. 不生成独立 EXE 或 checksum 附件；Windows 每个架构只发布一个 ZIP。

支持 `x86_64-pc-windows-msvc`、`aarch64-pc-windows-msvc`。
GitHub Windows packaging job 从 `WINDOWS_SIGNING_PFX_BASE64` 与
`WINDOWS_SIGNING_PASSWORD` secrets 恢复短生命周期 PFX 文件并强制签名；可用
`WINDOWS_TIMESTAMP_URL` repository variable 覆盖默认时间戳服务。两种架构必须使用同一证书，
否则跨架构包身份会分裂。workflow 允许自签 PFX，不修改 runner 的证书信任库。

`packaging/windows/new-development-certificate.ps1` 在 `CurrentUser\\My` 创建带 Code Signing
EKU 的可导出自签名 X.509 证书，并输出被 gitignore 的 PFX/CER。PFX 可导入 Kleopatra 做备份和
查看，也可直接交给 SignTool；Kleopatra/OpenPGP 不是 Authenticode 签名后端。只有显式传入
`-TrustOnThisMachine` 时脚本才把自签 CER 加入当前用户 Root，用于在本机查看系统信任效果；
自动更新本身不需要这个开关，最终用户也不需要安装 CER。不用本机信任后应删除该 Root 条目。
自签名证书不会让其他用户看到公开受信任的“已验证的发布者”；若需要该 Windows UI 身份，正式发布必须使用受信任 CA
颁发的代码签名证书并保护、轮换私钥。由于自动更新固定比较当前叶证书指纹，换证前必须规划
一次带过渡信任策略的版本，不能直接用新证书覆盖发布。
`packaging/windows/test-authenticode-update.ps1` 使用系统 Windows PowerShell 临时创建一天有效的
测试证书和随机密码 PFX，不加入用户信任库；它通过与 GitHub Secret 相同的 PFX 路径验证打包，
再签名两个临时 EXE 并运行 Rust `WinVerifyTrust`
同证书测试，并确认被篡改的已签 EXE 会拒绝。传入 `-VerifyPackaging` 时由 package script 仅在
工作区 `tmp/` 的隔离输出根完成构建、签名及 x64 ZIP 内更新候选 EXE 验证，不污染可发布的
`dist/`；`finally` 总会删除测试证书和所有临时文件。

`tools/benchmark-windows-dist.ps1 [target]` starts the unpacked
`dist/<target>/KeySteer/KeySteer.exe` and writes in-memory startup/resource
samples to JSON after the measured interval. With `-UsePerfProbe`, the binary
must be built with `--features perf-probe` and diagnostic samples are the emitted
instrumented `backend_started` elapsed time; they are not release performance gates. Without it, samples are explicitly named
`config_check_process_ms`. The script stops its launched process unless
`-KeepRunning` is selected. `-OutputPath` preserves separate A/B results; `-WarmupSeconds` excludes process warmup from resident samples, which include CPU-time deltas and actual sampling intervals. Binary/configuration hashes identify each run. `-Executable` and `-ConfigPath` allow equivalent
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

根目录 `.github/workflows/build.yml` 仅手动运行并只做发布所需工作：安装固定 Rust 工具链、同步
根包 lockfile 版本、安装目标、调用平台 package script 并上传各目标 artifact。它不再提供 `checks` 入口，也不在 runner 上运行
fmt、test、Clippy、性能或文档构建；这些检查必须在推送前本地完成。`publish=false` 时按
`platform` 构建 Windows、macOS 或全部目标并只保留 artifact；`publish=true` 时忽略平台筛选、
强制生成四个平台 ZIP，防止不完整发布。`release_type=release`
创建 `v<version>`，`pre-release` 创建带 run number 的独立 tag 并标记为 GitHub pre-release。
Windows x64/ARM64 与 macOS Apple Silicon/Intel 分别作为四个 matrix runner 并行编译；不同目标
仍生成独立机器码和包，但不在同一平台 job 内串行等待。matrix 会略增固定 setup 和总计费分钟，
主要优化的是发布墙钟时间。每个目标用 GitHub 官方 `actions/cache` 按 OS、target、固定 Rust
版本和 Cargo.lock 缓存 Cargo registry 与 release 构建产物；首次冷构建不受益，后续发布复用
依赖。package step 有 30 分钟硬上限，Windows 签名流程在签名、证书读取和 SignTool 验证边界
分别输出进度，信任组件异常时可定位且不会无限占用 runner。
Windows 自动更新复用便携 ZIP；运行时复用已有 `miniz_oxide` 做有界 DEFLATE 解压，不新增 ZIP
依赖。macOS 证书和
notarization 通过 secrets 注入。创建 Release 时把 `.github/release-notes.md` 的固定安装
提示置于 GitHub 自动生成的变更说明之前；该文件必须保留未 notarize macOS 下载包所需的
`sudo xattr -cr /Applications/KeySteer.app` 指引和来源安全提示。

每个 target 的 workflow artifact 也独立上传：Windows x64/ARM64、macOS Apple
Silicon/Intel 各一份；不会把不同架构放入同一个 ZIP 或 artifact。

`.github/workflows/pages.yml` 仅由 `workflow_dispatch` 手动触发；push 不会自动部署
GitHub Pages。需要更新线上文档时，在 Actions 页面运行 `Deploy documentation`。

`.github/ISSUE_TEMPLATE/` 提供错误报告、功能建议和配置/按键问题三种 Issue Form；错误
报告以一个必填描述收集实际行为、预期行为和复现方式，并提供可选的版本环境、脱敏 TOML 与
诊断信息，降低提交负担。空白 Issue 仍允许创建。不要在模板中预设 labels，因为 GitHub 只会添加仓库中
已经存在的 labels。

## VitePress 文档与模拟器

用户文档以中文根路径和英文 `/en/` 路径并行发布：中文源文件保留在 `docs/`，英文源文件在
`docs/en/`，不要把 `docs/ai/` 的维护者资料复制到任一面向用户的侧栏。`docs/.vitepress/config.mts`
在 `locales` 与 `themeConfig.locales` 中维护两种语言的导航、侧栏、搜索、页内目录和上下页文案；
两套导航各有一个语言菜单。新增或移动用户页面时，必须同时新增/移动其 `docs/en/` 对应页面并更新两套链接，
避免语言切换后落到不存在的地址。内联脚本还会将语言选择记录在浏览器本地存储，并把 VitePress 内置语言菜单的
跳转改写为当前页面的另一语言路径；新增加英文用户页面时，也要把其无 `/en/` 前缀路径加入
`config.mts` 的 `englishPages`，否则不会自动恢复英文偏好。

工具链：VitePress 1.6、Vue 3 TSX、`smol-toml`、pnpm。主要脚本：

- `pnpm docs:dev`：同步 default TOML/icon 后启动开发服务器。
- `pnpm docs:test`：Node tests，覆盖浏览器端绑定继承、配置 clone 和模拟状态。
- `pnpm docs:check`：先同步生成文件，再执行 `tsc --noEmit`。当前文档组件全部是 `.ts`/`.tsx`，不使用 Vue SFC；
  直接使用仓库固定的 TypeScript 可避免 `vue-tsc` 对编译器私有子路径的版本耦合。
- `pnpm docs:build`：同步静态资源后生产构建。

`scripts/sync-doc-assets.mjs` 只复制文档站需要的 default TOML/icon。首页下载组件在浏览器
挂载后调用 GitHub 的 latest release API，从返回的 `tag_name`、Release URL 和真实
`browser_download_url` 解析版本及四个平台资产；Cargo 版本或 Release 变化不需要重新构建
GitHub Pages。API 暂时不可用、限流或某个平台资产缺失时，下载入口降级到 latest Release
页面，不保留构建时的旧 tag 直链。

模拟器重点是键位和 Grid/Recursive Grid/UI Hint 样式可视化，不是完整 Rust runtime。它：

- 解析/输出 TOML，支持导入和下载。
- 可从 KeySteer 菜单接收 zlib + Base64URL fragment；页面立即清除 fragment，并只在浏览器本地解码。
- 模拟 Mode binding inheritance 和空格分组键。
- 展示键位动作分类、targeting overlay 和 key_help 面板外观；按键提示与其他模式共用 ModeStyleControls 和 ConfigStudio 预览，无独立高级配置模块。
- 不使用 Rust/WASM 校验器；复杂配置和最终校验交给程序/文档。

修改 Rust 默认绑定或 UI style 字段时，需要检查网页默认配置是否仍能正确解析、染色和显示。

## Window 验证

`api/window_layout.rs` 验证比例、空间导入、空区域和最小尺寸；`modes/window/numbering.rs` 与运行时测试覆盖唯一编号立即执行、歧义计时、事务合并及临时模式。网页 `window.test.ts` 验证同一语言。

原生事务探针：`cargo test native_window_layout_transaction_probe -- --ignored --nocapture --test-threads=1`，默认只操作自有窗口；显式设置 `KEYSTEER_PROBE_HWND` 可纳入指定窗口并在断言前恢复。`native_tab_activation_visits_two_windows` 验证多窗口激活与中心位置。macOS 交叉编译不能替代 AX 原生实测。

`app/runtime/tests/window_mode.rs` 覆盖目标锁定、临时 Normal、自定义绑定、modal 返回、Tab 无确认切换/居中、取消和旧结果隔离。`common/window_geometry.rs`、`window_session.rs` 覆盖负坐标、逻辑 DPI、中心缩放、平铺、32 步撤销分组、部分失败及可取消 worker。

Windows 原生探针只创建并操作自有临时窗口，可显式运行：

```sh
cargo test --lib native_window_adjust_restore_and_closed_identity -- --ignored --nocapture --test-threads=1
```

它验证真实 Win32 移动、尺寸回读、最大化/还原、撤销恢复及关闭后身份失效。普通测试不会创建这些窗口。完整验收仍需分别在 Windows/macOS 检查真实应用尺寸限制、多屏 DPI、Tab 焦点切换、Spaces 和原生全屏；交叉编译不能替代原生验收。

网页 Window 的示例窗口、布局与平铺由 `docs/.vitepress/simulator/window.ts` 实现，对应 `window.test.ts`；物理别名和临时激活匹配由现有 `bindings.ts` 提供。配置导入的 `[window.bindings]` 与 Rust 一样整体替换，不能深合并回默认表。

## 测试层次

- 源文件 `#[cfg(test)]`：API parse/canonical、geometry、Mode 状态、runtime 路由、平台纯逻辑。
- `src/tests/integration.rs`：crate 内部的发布配置、默认快捷键、Mode/catalog 与跨平台项目不变量。
- `src/tests/performance.rs`：默认编译但 `#[ignore]` 的内部性能预算；只通过 crate 私有生产实现运行。
- `tests/architecture_dependencies.rs`、`tests/logging_policy.rs`：不导入库实现的源码边界护栏。
- 文档站 Node tests：轻量模拟模型，不替代 Rust tests。
- 平台原生窗口/权限/Hook 仍需要对应 OS 的实机验证。
- `tools/benchmark-windows-dist.ps1` 是 Windows 黑盒整进程采样入口；它不属于主 crate 的
  `cargo test`，需在对应架构真机上分别运行基线与候选包。

常用完整检查：

```text
cargo fmt --check
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test tests::performance::steady_normal_frames_do_not_allocate -- --ignored --exact --test-threads=1
cargo bench --features benchmark-hooks --bench core_hot_paths
pnpm docs:test
pnpm docs:check
pnpm docs:build
```

只改文档 Markdown 时不必运行所有原生 target，但应检查链接/路径和 `git diff --check`。
# Latency benchmark gates

For causal startup diagnosis only, build with `--features perf-probe` and use
`tools/benchmark-windows-hint-cold-start.ps1` for startup offsets
`0/10/50/100/500 ms`. The offsets and input scheduling live only in the
external harness; production startup remains event-driven parallel prewarming.
Samples with `probe_dropped > 0` are invalid. The primary metric is the causal
physical `hook_received` marker through the first subsequent
`native_presented`, not `backend_started`. Because this binary is instrumented,
these samples locate latency stages but do not pass or fail release performance gates.

Global `stats_alloc` regions are process-wide. Allocation assertions that need
exact counts are ignored in ordinary parallel CI and must run alone with
`--test-threads=1`; ordinary CI checks deterministic Arc/SmallVec invariants.

## 场景边界验证

`tests/architecture_dependencies.rs` 检查 presentation 只依赖 api，Mode/Plugin 不依赖具体 composer，
且不能直接构造 OverlayLabel、OverlayShape、LabelStyle 等绘制原语。Grid 的注入测试使用替代
Presenter 验证提交的是原始借用状态，并且最终场景由该端口控制。既有 Grid/Hint/Window/help
行为测试继续检查实际 compositor 输出；Hint 分层测试随实现归入 `presentation/hint/layers.rs`。
重构后仍需运行单线程分配预算、帮助缓存开关释放与位置更新探针，不能仅凭编译判断热路径不变。

Window 回归应覆盖：E/反引号入口自动布局、撤销入口不再自动重排、A 无双击分支、区域编号原生聚焦、空区定位、X 删除与稳定 ID、Ctrl 短按与长按像素移动及一次撤销、应用最小尺寸边界。Windows ignored `native_maximized_window_can_tile_and_undo` 使用自有临时窗口验证最大化恢复、严格分屏和撤销，不操作用户应用。

独立窗口模式验收还覆盖：Idle 直达五模式、路径无关 Q、自定义目的模式、显式方向／继承／应用覆盖、异步事务交接与备注取消、Restore 成功生命周期和失败重试、Delete 确认／取消／连续删除／稳定 ID／有效空库／外部修改／原子替换失败。网页模拟器移除派生 Normal 方向表，与默认配置同步；保留 KSLAYOUT 格式互导测试。macOS 交叉检查只能证明编译，不能代替原生 UI 验证。

`native_window_three_state_cycle_and_undo` 是显式 Windows 自有临时窗口验收：连续两轮最大化→最小化→恢复，核对目标身份、原位置/尺寸、指针抑制及两步撤销；不得用用户窗口代替。
