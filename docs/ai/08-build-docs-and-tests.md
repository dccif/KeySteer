# 构建与验证

工具链、依赖和脚本以 `rust-toolchain.toml`、`Cargo.toml`、`package.json` 为准。

原生 macOS 打包使用 macOS 26 runner 的 AppKit SDK，使标准菜单和 About 能采用新系统外观；最低部署版本仍由 `packaging/macos/package.sh` 定义为 14.0。旧 SDK 编译的本地包不保证启用新系统设计；外观仍遵循用户辅助功能设置。

## Rust

按改动先跑相关测试；运行时或跨层改动使用：

```sh
cargo fmt --check
cargo test --all-features --lib --tests -- --test-threads=1
cargo clippy --all-targets --all-features -- -D warnings
```

架构、日志和 unsafe 护栏分别在 `tests/architecture_dependencies.rs`、`tests/logging_policy.rs`、`tests/safety_budget.rs`。不要为通过检查而直接放宽预算。`safety_budget` 另编译实际 macOS CF owner 源码，用检查空指针的模拟 release 验证空 AX 输出、单次释放与所有权转移；可在非 macOS 宿主运行，但不代替原生实机验证。

正式构建使用 `cargo build --locked --release --bin keysteer --no-default-features`。单元测试由 `cfg(test)` 隔离，测试依赖放在 dev-dependencies；基准与原生探针位于独立包 `tools/perf/`，不作为主包构建目标。`benchmark-hooks` 默认关闭，仅性能包显式启用并加载 `tools/perf/support/benchmark.rs`。该入口必须纳入版本控制，否则干净检出的 `--all-features` 检查会缺失模块；忽略本地性能产物时不能一并排除它。

## 性能与原生验证

- `src/tests/performance.rs` 和模块内 ignored 测试覆盖分配与原生探针；先读测试的环境要求，分配计数串行运行。
- `cargo bench --manifest-path tools/perf/Cargo.toml --bench core_hot_paths`：核心 CPU 路径；`--bench notification_queue`：通知队列。
- `cargo bench --manifest-path tools/perf/Cargo.toml --bench scan_stream -- --fusion-only` 比较就绪数据的融合 CPU 成本；`-- --async` 使用实际融合 worker 比较首份额、提交耗时、大来源期间的小来源延迟与总耗时。`-- --async-hint --common` 补测 200／300／400 标签的实际异步融合与共享 Hint 场景生成，分别记录首次场景、全量完成及后到小来源响应。基准不包含原生识别或系统合成器，不能冒充输入到像素响应。
- `tools/` 提供 A/B、整进程和原生测量入口。正式性能比较不启用 `perf-probe`，基线与候选用独立 target 目录。

`tools/benchmark-core-ab.ps1 -Mode Gate` 交替运行 Hint 扩展场景、持续移动帧、小规模标签和引擎按键派发，保存实际 EXE 的 SHA256 与每轮原始输出。`compare-core-benchmarks.py` 汇总各轮分位数的中位数，默认 p50 或 p95 同时超过 3% 和 2 ns 就返回非零；p99 保留为尾延迟诊断。阈值用于发现需要调查的变化，不代表阈值以内没有退化。正式比较至少三轮，异常时增加轮次并保留原始失败记录；计时期间不运行编译、测试或其他基准。CPU 门禁不能替代原生输入到像素验证。

基准子进程默认使用 `Normal` 优先级；`-BenchmarkPriority AboveNormal` 可用于排查调度干扰，须单独保存结果，不能替代默认优先级的响应测试。环境记录包含优先级和计时器分辨率；比较器同时检查成对轮次与样本批次，避免工作量不同的结果混比。
- 原生变更检查对应 target，并在对应 OS 验证输入、权限、窗口、显示与清理。交叉编译只证明编译兼容。`native_macos_window_parity` 还直接检查鼠标窗口枚举回退，并确认恢复的 AX 窗口与编号枚举保留同一身份。
- macOS 取色回归夹具：`clang -std=c11 -Wall -Wextra -Werror tests/fixtures/macos-point-capture.c -framework CoreGraphics -o /tmp/keysteer-point-capture && /tmp/keysteer-point-capture`；验证 Retina／副屏坐标、固定像素不混入变化的邻域颜色，以及错误尺寸拒绝。非 macOS 可省略 framework，仅运行坐标测试。
- 失败先区分本轮回退和已有基线问题；忽略测试不等于验收通过。

## 网页与发布

网页代码位于 `docs/.vitepress/`；变更时按范围运行 `pnpm docs:test`、`pnpm docs:check`、`pnpm docs:build`，视觉交互另做浏览器验证。

正式产物走 `packaging/` 脚本和 `.github/workflows/`，检查应用身份、资源和签名。纯文档修改验证链接、代码路径和 `git diff --check` 即可。

两个发布工作流都调用 `tools/compose-release-notes.sh`，从 `docs/releases/index.md` 提取当前版本，再追加 `.github/release-notes.md` 的通用 Windows 默认／兼容下载与自动选包说明、macOS 安装提醒；长期有效的安装／兼容说明维护在该模板中，每次发布自动附带。

`.github/workflows/cross-build.yml` 在 Linux 上交叉构建 Windows MSVC x64/ARM64 和 macOS Intel/Apple Silicon。手动运行提供平台选择、发布开关（默认开启）和 release/pre-release 选项；发布必须全部六包成功（Windows x64 兼容／AVX2／AVX-512、Windows ARM64、两个 macOS 架构）。推送或强制更新任意 tag 自动转发到默认分支（main）的 `workflow_dispatch`，构建并发布全部六包，删除 tag 不构建。非默认分支的手动运行也转发，保留平台和发布选项。所有实际构建在默认分支运行，使下载缓存归属并复用于 main；checkout 单独固定为启动时所选提交，不因 main 后续推进而发布错误源码。转发传递 `source_sha` 和 `release_tag`；默认分支手动填写 `release_tag` 时可省略 `source_sha`，直接解析该 tag。tag 必须与构建提交一致，且提交已包含在默认分支历史中。tag 发布沿用所选 tag，版本格式的预发布 tag（如 `v1.2.3-rc.1`）自动标记 pre-release；分支手动发布仍按 Cargo 版本生成 `v<version>` 或 `v<version>-pre`。发布先删除同名旧 Release 及全部附件，再删除远端旧 tag、在已解析的构建提交重建 tag，并创建全新 Release，保证同名 tag／相同正式或预发布版本以最新运行产物为准。正式版设为 Latest，预发布不占用 Latest。实际发布运行共用 concurrency 组，新运行取消旧运行，避免旧构建较晚完成后覆盖新发布；转发运行使用独立组，避免取消自己启动的构建。

构建成功后从默认分支 dispatch 独立的 `.github/workflows/pages.yml` 运行，同时保留 Pages 的手动入口。发布成功时传入本次 release tag，使下载入口对应本次产物（包括显式选择的预发布）；仅构建时使用 `latest`。任一构建或发布失败、取消时不部署。转发构建和 Pages 的 job 使用 `actions: write` 发起 dispatch；Pages 自身声明 `contents: read`、`pages: write`、`id-token: write`。tag 更新使用 `GITHUB_TOKEN`，不会递归触发 tag 构建。

`packaging/windows/package-cross.sh` 对已编译 EXE 使用现有 `WINDOWS_SIGNING_PFX_BASE64`、`WINDOWS_SIGNING_PASSWORD` 和可选 `WINDOWS_TIMESTAMP_URL` 完成 Authenticode 签名、RFC 3161 时间戳和签名校验；缺少证书或签名失败时不上传包。默认 AVX2 版产物名称与 `package.ps1` 一致：`KeySteer-v<version>-<target>.zip` 内含 `KeySteer/KeySteer.exe` 和 `KeySteer/keysteer.default.toml`；x64 兼容版／AVX-512 版在 target 后追加 `-compatible`／`-avx512`，上传 artifact 身份也独立，发布时校验六个精确文件名。更新器复用同一套完整 v3／v4 特性要求：编译期特性识别当前构建，运行时 CPU 和系统可用特性选择支持的最高等级（AVX-512 → AVX2 → 兼容），不受最初下载包影响。新版本正常升级；同一版本但构建等级不同也下载切换，已经匹配时不重复下载，不降级版本号。GitHub Release API 直连失败后，只通过 `gh-proxy.com/https://api.github.com/repos/dccif/KeySteer/releases/latest` 重试，不使用 jsDelivr。两路共用包身份、大小和摘要校验；镜像检查成功时下载优先走相同镜像，失败再试直连。旧版更新器仍请求无后缀文件，因此不支持 v3 的旧电脑首次迁移需手动下载兼容包。签名校验显式信任提供的证书链并验证叶证书指纹，不代表公共信任或 Windows 原生运行验证。原生构建保留在 `build.yml`；两种工作流使用相同的产物名称和发布说明来源，原生工作流仍拒绝重复 Release。

两个构建工作流复用 `.github/actions/setup-rust`，统一从 `rust-lang/rust` 官方 Release API 解析最新稳定版本，由 `dtolnay/rust-toolchain` 安装工具链与目标架构并设置构建日期。仅在 Cargo.toml 与 Cargo.lock 的项目版本不同步时调用 `cargo update`，避免每次初始化都更新索引。ZIP 上传关闭二次压缩；原生 Windows 签名证书在打包结束或失败后清理。

交叉工作流通过带 `GITHUB_TOKEN` 的官方 Release API 查询 LLVM 和 cargo-xwin：`llvm/llvm-project`、`rust-cross/cargo-xwin`。LLVM 下载 Linux X64 工具包（优先 zstd），按 Release API 的 SHA256 校验，支持 1 GiB zstd 解压窗口，解压后将 bin 目录置于 PATH 首位；不访问 `apt.llvm.org`。cargo-xwin 由 `taiki-e/install-action` 下载并校验上游预编译程序，禁用 fallback，避免从源码安装。系统库和签名工具使用 Ubuntu 软件源。

两个工作流通过共享 Rust 初始化缓存工具链及第三方依赖下载，不缓存工作区源码、`target` 或逐提交 EXE。下载缓存关闭 Swatinem 的自动环境哈希，键只包含宿主与 Cargo.lock 中外部依赖摘要，项目版本和 CPU 变化不产生新下载缓存。交叉工作流在同一 Linux 宿主安装四个 Rust 目标，共用工具链缓存。

`.github/actions/setup-llvm` 为两个平台共用官方 LLVM 安装缓存和 apt 下载缓存。LLVM 仅在未命中时下载、验 SHA256、解压，随后裁剪为两平台共用的编译工具、共享库及 Clang 资源目录；安装验证成功即保存。不缓存 LLVM 调试器、分析工具和开发静态库。系统包先检查 runner 已安装内容，只补缺失项，安装失败时才刷新 apt 索引；Windows 签名工具仅在 Windows 任务安装。apt 下载缓存按 runner 镜像版本和包列表复用。SDK/CRT 由 `cargo xwin cache xwin` 独立准备并立即保存。macOS SDK 和 rcodesign 同样在安装后保存。缓存恢复有传输/解压开销，apt 仍需安装；旧缓存需在 GitHub 管理中清理或等待淘汰。签名证书不进入缓存，项目与依赖每次重新编译。

macOS 默认从 `joseluisq/macosx-sdks` 社区清单自动选择最新 SDK 并校验摘要（可用仓库变量覆盖），配合官方 LLVM 的 Clang 与 Mach-O LLD，在 Linux 编译 Objective-C 桥接并打包。`packaging/macos/prepare-sdk.py` 校验 SDK 结构；`configure-cross.sh` 为目标配置编译器、归档器及链接器；`package-cross.py` 保持应用身份与 ZIP 布局，生成 PNG-backed ICNS，调用 rcodesign 签名并可选公证/装订。默认 ad-hoc 签名与原生工作流一致。配置变量、证书及工具限制见 [Linux macOS 打包](../../packaging/macos/README-cross.md)。

Windows x64 固定三个编译级别：兼容版 Rust `target-cpu=x86-64`、C `/clang:-march=x86-64`；AVX2 版两者使用 `x86-64-v3`；AVX-512 版两者使用 `x86-64-v4`。不使用 runner 的 `native`，兼容版遵循工具链的 Windows x64 基线且不要求 AVX／AVX2／AVX-512。Windows ARM64 和 macOS 保持各目标默认 CPU。release 的 O3、fat LTO 和单代码生成单元不变。

网页 `DownloadSection.tsx` 的 Windows x64 默认入口固定选择无后缀 AVX2 版，仅检测 OS／架构，不推断指令集。兼容／AVX-512 在选定 Release 存在对应附件时才显示为手动选项；选包提醒只说明一般用默认版、无法启动尝试兼容版；启动后任意包均可通过「检查更新」自动选择最高支持等级，同版本也可切换，无需手动核对指令集。`latest-release.ts` 按不同文件名尾部隔离六种附件，可选版不能覆盖默认 URL。

`KEYSTEER_CROSS_WINDOWS=1` 在非 Windows 宿主上启用 C 桥接和图标/manifest 编译，需要完整交叉工具链；未设置时保留不依赖 SDK 的跨平台类型检查。MSVC 交叉工作流显式设置 `RC_PATH=llvm-rc` 和空 `CROSS_COMPILE`，避免 winresource 0.1.31 初始化 GNU 前缀时误报 ARM64 未知目标。跨宿主的工具链、SDK、路径和签名时间戳可能改变哈希，不能据此断言行为不一致。

`KEYSTEER_CROSS_MACOS=1` 在非 Apple 宿主显式启用完整 Objective-C 桥接编译，需要 SDKROOT、Clang 与目标链接器；未设置时仍允许不安装 SDK 的跨平台类型检查。
