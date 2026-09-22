# 构建、打包、文档站与测试

窗口事务优化的功能回归位于 `window_session/async_tests.rs`：覆盖分批准备、取消前无写入、晚发现非法布局、提交次序回滚和历史重试。`window_tabs/tests.rs` 验证独立组不受阻挡、相关结构事件保序和队列上限；presentation 测试验证几何变化复用文本及标题／组／编号失效；logging 测试验证稳定错误合并时跳过上下文格式化。Windows 的显式 `native_async_group_maximize_layout_and_cancel` 使用自建临时窗口，包含 Undo／Redo／ResetInitial 原生验收；它不是性能基准，不给生产路径增加插桩。

中英文 `releases/index.md` 使用 `releaseHistory: true`，由 Markdown 插件在渲染阶段将最新版本之后的内容包入原生 `details/summary`；最新版本展开，旧版本独立折叠，关闭冗长的右侧目录。
保留源文件中的 `## 版本号` 和全部正文，确保 `tools/compose-release-notes.sh` 不会提取到折叠标记；新增版本无需手动调整旧版本。`release-history.test.ts` 检查中英文内容／锚点不变，以及代码块和未启用页面不受影响。

配置模拟器语言默认跟随 VitePress 页面语言，顶部选择器可在同一实例切换中文／英文。
`config-studio/i18n.ts` 使用 provide/inject 保存实例级语言，避免 SSR 请求间共享状态；切换时释放预览按键，但不重建配置文档、工作区或编辑页面。
`messages.ts` 集中维护显示文案；模式、命令、TOML 路径、配置值和用户备注保持原值，旧模拟器事件只在显示边界匹配完整模板。
设置搜索同时索引中英文名称和原始路径；动作分类使用稳定的原始值，不能把翻译后的名称写入选项 value。
`studio-i18n.test.ts` 检查全部字段元数据翻译、双语搜索和插值内容保留；浏览器验收语言切换前后按键绑定不变，以及英文窄屏换行。

Windows 前台锁回归：`foreground_unlock_*` 检查屏蔽批次、自身注入标记、保留已按住 Alt、所有部分发送前缀的清理；ignored `native_explicit_selection_recovers_foreground_lock` 只操作临时自有窗口，先用 LockSetForegroundWindow 验证原始调用被拒绝，再验证共享 select 恢复前后台切换，并恢复原前台。需交互桌面；沙箱 SendInput 拒绝访问不代表产品路径失败。

Window 固定帮助预编译：`window_help_is_precompiled_before_entry_and_reused_across_move_resize_and_reentry` 检查入口前准备、跨会话 Arc 复用与 Resize 隐藏 Other Actions；`precompiled_window_help_matches_dynamic_rendering` 对比单/双栏、Move/Resize 和 1/1.5/2 DPI 的完整场景。分配探针 `precompiled_window_help_allocation_comparison` 必须单线程运行 ignored 测试。2026-09-16 本机 debug 下，1920 宽/1 倍 DPI，当前动态回退构建与预编译消费分别为 Move 817→106、Resize 767→96 次分配；仅比较一次面板重建，不代表整程序延迟或旧版本基准。

macOS `mapped_keyboard_events_*` 回归覆盖左右四类修饰键的全部非空组合、重复发送、
目标左右修饰键的释放顺序、保留修饰键和逐事件 Caps Lock/NumericPad 属性。
这些测试在 macOS target 下编译和执行；Windows 的交叉检查仅确认可编译，不能替代原生运行。
原生验收需确认 Primary+H/J/K/L 只移动一个方向、长按重复、源键松开后无残留，
目标显式配置 Command/Shift 时才执行对应组合；普通 C+H/J/K/L 与未映射 Command 快捷键保持有效。

映射发送性能探针 `mapped_chord_preparation_performance` 用 release 模式单线程 ignored 测试
测量 Engine 准备阶段（不注入系统输入），覆盖无修饰键、Alt、目标 Ctrl+Shift 和多源修饰键。
也覆盖目标 Ctrl 已由右 Ctrl 按住时的动态筛选。每组预热后循环 200,000 次并检查零分配；
测试使用计数系统分配器，输出为每批 1,000 次的平均耗时分位数，
不是端到端按键延迟。`mapped_chord_native_preparation_does_not_allocate` 检查常见 Windows
原生批次准备无分配。超出内联容量的任意长配置仍允许堆回退。

2026-09-14 本机局部对照（上述计数分配器、release）：预编译前 Alt → Ctrl+Shift+方向键
每次准备 12 次分配，批均值中位数约 573–575ns；预编译后为 0 次、约 42ns。
无源修饰键、Alt 和四个源修饰键的纯方向目标分别约 18/33/68ns；这些数值仅用于局部回归，
不包含 Hook、原生队列、SendInput 或目标应用处理，不能作为端到端延迟承诺。

键盘映射原生重复回归：`keyboard_chord_mappings_repeat_with_native_events_and_stop_without_prefix`
覆盖 C+H/J/K/L 连续输入、同输出裸键不接管与无新增 deadline；
`direct_keyboard_mappings_repeat_and_release_without_an_extra_tap` 覆盖单键、带修饰键发送和释放后停止。

无独立绑定的组合前缀回归位于 runtime 的 `window_mover.rs` 测试：覆盖任意前缀名称、三键组合、
单键松开补发、无关输入排序、新前缀接续、修饰键释放顺序、原始字母模式、禁用候选、重复键与捕获丢失。
`unavailable_unbound_prefixes_do_not_allocate_or_delay_ordinary_keys` 为 ignored 分配探针，需用
`cargo test --lib unavailable_unbound_prefixes_do_not_allocate_or_delay_ordinary_keys -- --ignored --test-threads=1`
单独执行；索引在配置编译时建立，测试确认普通键/缺失修饰键路径无分配，前缀不会新增 scheduler deadline。

本地文档默认配置由 docs:sync 生成，ConfigStudio 必须通过 parseConfigDocument 加载完整 TOML；新增共享卡片字段需同步网页校验器。config-document.test.ts 覆盖完整默认文件加载，避免 UI 初始化失败后控件无效。快速切换预览复用既有 animation frame 检查长按，提供数字／点击选择与屏幕、窗口、鼠标定位；配置不写入浏览器草稿。

模式统计与快速切换回归：`preset_store/usage.rs` 覆盖阈值前不落盘、阈值 checkpoint、退出补存、旧 snapshot 不回滚计数／布局和替换失败保护；Rust/TS 共用 `tests/fixtures/workspace-usage.ksw` 覆盖 v2、u64 最大值和逐字节截断，原 `workspace.ksw` 保留 v1 兼容验收。runtime 覆盖 Idle 透传、短按 Q、立即 Q+数字、固定排序、黑名单只影响面板、输入捕获丢失及不重复计数。presentation 验证负坐标屏幕、窗口中心和鼠标边缘夹取。跨平台编译检查不等于系统关机、注销实机验收；不得为验证自动触发用户系统退出。

范围回归覆盖默认当前屏幕／非最小化、显式全部屏幕／包含最小化、跨模式配置、重入回收关闭和最小化窗口编号、持久组仍保留。多屏事务验证各屏布局不跨屏搬动、失败整体回滚及一次撤销；渲染测试检查实际帮助面板与重叠窗口卡片在 1/1.5/2 倍 DPI 下分离，且排序后的文字仍随背景移动。跨进程自有窗口探针另检查最小化后可枚举、UI Hint 仍排除它、屏幕归属及恢复；macOS 仍需实机 AX 验证。

组内优先循环回归覆盖活动成员与旧目标不同、正反循环及数字跳出组；编号回归覆盖同程序大小写归一批次和刷新稳定性；最小化回归覆盖全部成员、非成员不受影响、旧焦点不重开组和恢复单个活动成员。自有 Win32 窗口探针向活动窗口发送最小化，验证全部成员 IsIconic、隐藏成员的还原位置不变和可恢复；因 hook 跳过自身进程，测试在系统完成最小化后显式交付同一原生事件入口。

栏高回归覆盖 1/1.5/2 倍比例的逻辑外框、原生内容框、最小高度、选择后几何、模式外顶部移动和还原不重复扣减。`native_grouped_layout_accounts_for_header_and_dissolves_maximized` 使用自有窗口验证真实严格分屏、布局撤销／重做、最大化和解散归还空间；`native_maximized_tab_header_keeps_state_and_restore` 检查保留最大化状态及三态恢复。拖拽原生探针另验证纵横滚轮后的点击、标题更新保持滚动和自动显示活动项。Mode 测试验证 `move_*` 与全部成员卡片。

标签拖拽回归覆盖退出模式后跨组移动、整组合并、组内排序、剩余单成员位置、撤销／重做和失败回滚。Windows 显式 `native_tab_drag_drop_and_cancel` 向自有临时栏发送鼠标消息，验证单窗口／整组 drop、无效落点与 capture 取消；不移动用户指针。`native_cross_process_tabs_keep_composed_content` 启动自有子进程窗口，反复切换后核对 WS_VISIBLE、WM_SHOWWINDOW 计数与透明样式恢复，不能代替 Explorer/Zed 的实际画面验收。活动成员原生探针同时检查栏 owner、非置顶样式和箭头光标。

活动成员回归覆盖：退出模式后持续拖动时隐藏窗口零几何写入、选择时才对齐、再次进入并追加使用当前活动矩形、关闭标签栏后全体恢复可见、活动成员关闭后显示剩余窗口。Window Session 与 runtime 验证普通窗口和隐藏成员共享编号和前后轮换、可重绑、退出模式后不产生选窗请求。

Rust 与 TypeScript 使用同一 `tests/fixtures/workspace.ksw` 做逐字节往返，覆盖二进制布局文件互导；v2 handoff 测试同时校验按键 source、布局 bytes 和 URL 清理，runtime 测试验证重新打开 R 后读到外部替换的文件。

布局收藏的 API 测试覆盖 9/4 区域与窗口数量不匹配、空区域及排序、备注/隐私字段；`app/preset_store` 测试覆盖二进制往返、逐字节截断、版本、持久化及写入失败不覆盖。runtime 测试覆盖 Ctrl+S、原生输入放行、R→编号恢复和会话取消后的迟到结果。Windows ignored `native_note_dialog_preserves_unicode_and_cancels_owned_windows` 在交互桌面创建并清理自有对话框，验证 Unicode 保存和取消；macOS 编译检查不能替代实机 IME 验证。网页 `window-presets.test.ts` 覆盖独立浏览器布局库与恢复撤销。

## 优化构建档位（2026-08）

- 发布依赖缓存使用 `Swatinem/rust-cache@v2`：忽略 workspace 自身版本号，按依赖、实际 Rust 工具链、环境、host 和 matrix target 分区；包含 `target` 下 host/target 的依赖产物，排除 workspace 产物和 Cargo bin。仅默认分支保存，其他分支/标签只恢复可访问缓存，避免每个发布 ref 各存一份。`prefix-key: v1-release-deps` 标记缓存策略版本；脚本内 Windows `/Brepro` 和 macOS deployment target 也体现在自定义 key 中，修改这些编译条件时同步更新 key。首次切换需要预热，项目自身 release 编译和 LTO/链接仍正常执行；旧缓存不会由新工作流主动删除。

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

Grid／Recursive Grid 预览通过 `grid-region.ts` 按选键路径逐层细分逻辑画布；网格范围与模拟指针中心使用同一计算。Backspace 弹出路径后恢复父区域，重新进入模式清空路径。`grid-region.test.ts` 覆盖多层坐标、矩形网格、返回父级和不存在的格子；浏览器验收确认位置与尺寸同时改变，而非只改变高亮。

配置工作台按鼠标控制、目标定位、窗口管理、全局设置和配置文件分类；分类是展示元数据，不改变独立模式或 TOML 路径。`config-studio/navigation.ts` 维护页面、页签及字段归属，`fields.ts` 维护现有控件和中文搜索标签。搜索结果打开对应页面、页签和折叠区后定位控件；`settings-navigation.test.ts` 检查字段入口覆盖、共享样式归属及搜索。

`ConfigStudio` 持有唯一配置文档及示例工作区，编辑目标与模拟运行模式独立。分类切换只切预览模式，不重建文档；预览快捷键不切换表单。窗口卡片共用字段在 `window_card` 展示页编辑，Editor 位置覆盖仍属于自身外观。复用原有 TOML 读写和模拟逻辑，不新增 Rust runtime API。

工作区在 1200px 起为分类、设置、预览三栏；768–1199px 为两级选择与双栏；手机通过设置／预览切换保留状态。设置与预览分别滚动。预览使用固定逻辑画布和 ResizeObserver 缩放，指针坐标反算到同一逻辑空间。只有预览画布获得焦点时捕获按键，失焦、切页和关闭放大时释放按键；放大预览约束 Tab 焦点并在关闭后恢复原焦点。

桌面工作区可扩展至 2400px；`PaneDivider.tsx` 用 Pointer Capture 调整导航宽度及设置／预览比例，支持方向键、Home/End 和双击恢复。宽度只保存在页面展示状态，不写入 TOML；分栏收窄时设置行按容器宽度重排，手机隐藏分隔条。拖动开始释放模拟器按键状态，预览继续通过同一 ResizeObserver 缩放。

默认进入按键页。映射编辑器提供主键区／完整键盘、动作搜索与分类、当前绑定来源和折叠的高级命令。点击键帽再选动作、先选动作再点键帽、Pointer Capture 拖动动作到键帽都复用 `selectKey`/`setAction`，组合键和字符键保持原绑定语义。动作拖动达到阈值后才开始，松开在键帽外或捕获取消不写配置；浏览动作库时键盘在设置面板内吸顶。浏览器验收需检查点击、拖放、分组别名拆分、继承显示和导出绑定表，不能只检查视觉状态。

完整键盘默认显示，由 `FittedKeyboard` 的 ResizeObserver 根据可用宽度缩放；切换页签销毁观察器，再进入时重新测量。动作库随设置面板统一滚动，不再设置独立最大高度。导入导出、工作区文件和统计页使用同一 utility card 样式及 VitePress 主题颜色，禁止复用固定浅色文字／深色输入背景；文件选择按钮也需保持一致。统计页直接展示内容，无需再展开整页折叠区。

组合键工具栏可切换 Windows／macOS 预览，切换时释放按键状态但不写配置。Primary 标注必须通过现有 `shortcutCaption` 解析当前文档的平台别名（例如发布配置的 Windows Primary 为左 Alt），不能硬编码为 Ctrl／Command。物理键帽使用平台名称与符号，原始键名和 `primary+…` 绑定保持不变。

网页验收覆盖 1440×900、1280×720、1024×768、手机宽度、浅深主题、搜索高级项、焦点恢复、表单输入不触发模拟快捷键及滚动到设置末尾时预览可见。配置兼容性继续由局部 TOML、未知字段、绑定替换和工作区往返测试覆盖。

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

`scripts/sync-doc-assets.mjs` 只复制文档站需要的 default TOML/icon。VitePress 配置在每次
构建/开发服务器启动时查询 GitHub latest release API（构建 15 秒、开发 3 秒超时），从 `tag_name`、Release URL
和真实 `browser_download_url` 获取最新正式发布信息，通过 Vite define 同时注入 SSR 和浏览器包。
DownloadSection 首屏直接包含版本和下载地址，浏览器不再请求版本 API；缺失的平台资产链接到
本轮 Release 页面，不推测文件名或使用 Cargo 中尚未发布的版本。Pages 构建使用 github.token，
本地可设置 GITHUB_TOKEN；token 仅用于请求，不进入注入数据。配置工厂使用 Vite 传入的 `command` 区分 `serve` 和 `build`。开发模式遇到 API 错误、断网、超时或无效元数据时打印警告并继续启动，版本标记为本地预览不可用，下载入口指向 GitHub latest Release 页面，不推测版本或资产名。生产构建遇到上述错误仍失败，
不发布空版本或过期回退。Pages 仍按 workflow_dispatch 手动部署，每次运行自动解析当时最新正式版，
无需修改组件或 workflow 中的版本号；后续新 Release 需要重新运行 Pages 才更新已部署静态页面。

模拟器重点是键位和 Grid/Recursive Grid/UI Hint 样式可视化，不是完整 Rust runtime。它：

- 解析/输出 TOML，支持导入和下载。
- 可从 KeySteer 菜单接收 zlib + Base64URL fragment；页面立即清除 fragment，并只在浏览器本地解码。
- 模拟 Mode binding inheritance 和空格分组键。
- 展示键位动作分类、targeting overlay 和 key_help 面板外观；按键提示与其他模式共用 ModeStyleControls 和 ConfigStudio 预览，无独立高级配置模块。
- 不使用 Rust/WASM 校验器；复杂配置和最终校验交给程序/文档。

修改 Rust 默认绑定或 UI style 字段时，需要检查网页默认配置是否仍能正确解析、染色和显示。

## Window 验证

`window_family_shortcut_plan_survives_input_and_scene_refresh_until_routes_change` 覆盖 Window / A / E / R / T：逐键输入、编辑事务反馈和强制场景重绘均复用同一份 Arc 快捷键表，绑定表重建才替换；Editor 尚未就绪时也包含保存键。`window_normal_launcher_hint_stays_visible_across_unrelated_key_edges` 检查进入时启动键仍按住以及后续 HJKL / Shift / Ctrl 按下、松开均不会丢失 Normal 入口。

帮助面板的运行时测试覆盖 Window / Quick / Editor / Tabs / Restore 在 100%、150%、200% DPI 与两种字号下的边界、标签相交、键帽对齐、完整单行文字和底部模式顺序。`window_return_hint_uses_local_configured_destination_and_physical_alias` 验证返回目标改为 Normal、按键改成 F9 别名后，右上角提示与实际跳转同步。`export_grouped_window_help_visual_samples` 为显式运行的忽略测试，导出当前原生合成器几何到 `target/window-help-v14/`，供字形与版式检查；模拟器分组测试位于 `window-help.test.ts`。

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

Tab 回归另覆盖空闲组编号复用、重入不重复分组、失焦后标签栏仍可见、显式选择与普通 Window 切换在隐藏旧成员前转移焦点。Windows 原生探针直接向位置回调交付自有窗口的移动事件，在调用共享 pump 之前验证标签栏已到达新位置；查找/关闭仅使用测试 adapter 返回的自有栏句柄，不按全局类名查找用户窗口。消息唤醒测试验证在开始原生等待前排队的命令唤醒不会丢失。

`worker_exit_keeps_bar_tracking_without_moving_hidden_members` 经真实后台请求队列建立组合、取消模式会话，再连续注入外部移动，验证退出后标签栏继续定位且隐藏成员保持原位置；不能只用组合对象的 `reset()` 测试替代这一生命周期覆盖。

持久 Tab 组的测试分三层：`window_tab_model.rs` 验证成员转换与稳定编号；`window_tabs/tests.rs` 验证自动整理一次撤销、跨会话保留、共享几何/最小化、关闭清理、失败回滚与旧焦点事件不能覆盖点击；Mode/模拟器测试验证连续 `12t34t`、多位数字与空格、组前缀、单成员转移、模板恢复取消和混合文件编解码。混合 Layout/Tabs 的 workspace.ksw fixture 必须在 Rust 与浏览器逐字节往返。

Windows 显式 `native_active_only_tabs_move_switch_dissolve_and_close` 只创建自有临时顶层窗口，验证 40 次活动成员移动时隐藏窗口矩形不变、切换时对齐且可见移动计数不增加（包括隐藏的最大化成员）、重入追加采用最新矩形、原父级/样式保持、关闭独立标签栏恢复成员及活动窗口关闭后的单成员释放。前台权限由测试 adapter 隔离，实际几何/显隐/窗口关系使用 Win32；不能用它代替所有第三方应用焦点和动画验收。macOS 交叉编译检查覆盖恢复后的入口、AX observer 与 AppKit 标签栏；原生动画和应用兼容性仍需 Mac 实机验证。

`tests/architecture_dependencies.rs` 检查 presentation 只依赖 api，Mode/Plugin 不依赖具体 composer，
且不能直接构造 OverlayLabel、OverlayShape、LabelStyle 等绘制原语。Grid 的注入测试使用替代
Presenter 验证提交的是原始借用状态，并且最终场景由该端口控制。既有 Grid/Hint/Window/help
行为测试继续检查实际 compositor 输出；Hint 分层测试随实现归入 `presentation/hint/layers.rs`。
重构后仍需运行单线程分配预算、帮助缓存开关释放与位置更新探针，不能仅凭编译判断热路径不变。

Window 回归应覆盖：E/反引号入口自动布局、撤销入口不再自动重排、A 无双击分支、区域编号原生聚焦、空区定位、X 删除与稳定 ID、Ctrl 短按与长按像素移动及一次撤销、应用最小尺寸边界。Windows ignored `native_maximized_window_can_tile_and_undo` 使用自有临时窗口验证最大化恢复、严格分屏和撤销，不操作用户应用。

独立窗口模式验收还覆盖：Idle 直达五模式、路径无关 Q、自定义目的模式、显式方向／继承／应用覆盖、异步事务交接与备注取消、Restore 成功生命周期和失败重试、Delete 确认／取消／连续删除／稳定 ID／有效空库／外部修改／原子替换失败。网页模拟器移除派生 Normal 方向表，与默认配置同步；保留 KSWORKSP 格式互导测试。macOS 交叉检查只能证明编译，不能代替原生 UI 验证。

`native_window_three_state_cycle_and_undo` 是显式 Windows 自有临时窗口验收：连续两轮最大化→最小化→恢复，核对目标身份、原位置/尺寸、指针抑制及两步撤销；不得用用户窗口代替。

窗口历史测试覆盖逐步 Undo/Redo、新操作清空 redo、超过 32 组仍可恢复初始状态、跨窗口最大化/最小化恢复、未修改窗口不被重置、取消保留未处理历史，以及可重绑 Shift+Z/Shift+C、等待编辑完成后重置且不自动重排。显式 `native_window_initial_restore_and_redo` 只创建并操作自有临时 Win32 窗口，验证最小化→初始恢复→撤销→重做。网页测试同步覆盖 Quick/Editor 的入口撤销边界与原始状态恢复。

编号保留回归覆盖同一会话反复最大化→最小化→还原、最小化期间新窗口出现、明确关闭释放和新会话重建。模拟器同时验证最小化编号保留但不进入可选数字索引。

恢复模式提示回归关闭可选 `?` 开关，验证进入按键松开即显示完整提示，强制后续刷新场景相同；自定义修饰键、普通键和链式别名的提示须与实际模式切换一致。网页额外覆盖 Windows/macOS 别名覆盖与配置修改后的重新解析。

区域编辑回归覆盖百次删除／分割复用空号、两轴及两侧选中区域增减、最近相邻区域最小尺寸受限时从其他祖先取空间、平铺面积完整及一次手势撤销。运行时以自定义别名改绑增宽动作，验证实际 ApplyLayout 与提示来自配置表。

Window 入口回归测试 acquire_activates_pointer_target_without_warp_or_geometry_change 验证激活、无鼠标移动及拒绝反馈；close_requests_only_the_target_and_preserves_it_until_application_closes 验证只关闭目标且不提前退役。runtime window_close_uses_default_or_configured_alias_and_editor_keeps_remove_region 覆盖默认 X、自定义 F9 别名、即时提示及 Editor X 隔离。

window_close_resolves_active_tab_and_dissolves_only_after_confirmed_closure 覆盖关闭组代表时实际关闭活动成员、3→2 保留组、2→1 解散/撤栏/显示剩余成员，以及保存取消前成员不变。native_close_request_only_closes_owned_target 已在 Windows 显式执行，只创建/关闭自有临时窗口并验证其他窗口与过期身份。

Quick 比例尺验收覆盖混合 1/2、1/3、0.3、0.45 和相近比例：原标识保留、按实际比例定位、宽高当前值、横竖屏比例、高 DPI 的文字边界。共享 workspace.ksw 测试样本应随测试源码保留。


音量回归覆盖三个模式、改绑、重复按键、松开前缀即停止、静音不重复、编辑与历史不变、取消与关闭身份。Windows 显式 native_application_audio_session_volume_and_mute 只创建本测试进程的无声会话，验证真实音量与静音往返，不触碰其他应用。


系统音频测试覆盖应用/系统路由隔离、Shift 松开后的重复抑制、设备循环与无设备边界。显式 native_audio_policy_routing_and_system_interfaces 对系统仅重设已有默认值、只读系统音量；应用策略只针对测试进程并恢复原偏好。


音频请求回归验证独立结果通道、无需 Window Acquire 的系统请求、按调用者取消队列、忽略取消后的迟到反馈，以及音频错误不生成窗口/布局请求。构建 macOS 应用需 macOS 14.2+ SDK，链接 CoreAudio、AudioToolbox、AVFoundation；运行时仍允许 macOS 14.0。

Windows 主机只能交叉检查 macOS Rust（`cargo check --target aarch64-apple-darwin --lib --bins --tests`，Intel 同理），不能验证 Objective-C 编译、权限与真实音频。Mac 原生验收应使用打包的 .app：拒绝/授予系统音频权限；应用及系统每步 1%、按住重复、松开 V/Shift 停止；独立应用与系统输出；多进程浏览器；拔插输出；休眠/唤醒；退出后声音恢复。原有 `examples/macos_native_probe.rs` 引用了私有 crate 模块，尚不属于上述交叉检查范围。


Tabs 优化验收：`cargo test --release tabs_geometry_baseline -- --ignored --nocapture`，可用 KEYSTEER_TABS_BENCH_OUTPUT 保存 CSV；每次涵盖 2/10/30 成员、各三轮 1000 次几何事件，记录 p50/p95/p99、快照读取和应用写入。它是模拟后端 CPU 基准，不是显示帧率或 compositor 延迟。回归覆盖单组更新、失败重试、手势结束校正，以及阻塞音频下的异步提交、队列上限和取消。Windows ignored 原生探针验证活动成员直接移动/缩放跟随、切换/解散、最大化恢复和标签拖放；实机跨屏 DPI 与高刷新率抖动仍需视觉验收。macOS 原生验收与性能入口见本专题末尾。


音频安全检查复用现有 unsafe 总预算（未增加），将 Toolhelp 的一个块从音频调用层迁入 native 所有权封装；portable 安全门禁覆盖 platform/common。回归验证真实只读进程快照包含自身，以及音频失败后 worker 可继续处理下一项。测试断言可继续使用 unwrap。


音频配置同步回归：Rust 验证 Config::to_toml 自定义系统静音绑定往返；网页 config-document.test.ts 验证十个音频默认绑定、重绑、原样比例标签与布局间距往返，window.test.ts 验证应用/系统静音独立，window-help.test.ts 验证 Shift 合并及自定义键提示。


关闭回收：X 的原生关闭请求仍为异步。完整库存确认被请求关闭的窗口不再存在于候选列表时，也回收隐藏到托盘但句柄仍存活的逻辑窗口；取消扫描和仍可见的保存对话框不构成关闭确认。closed 结果优先于旧快照，移除目标边框与编号。关闭后窗口编号按原相对次序压紧为 1..N，恢复出现的窗口追加新编号；普通最小化/屏幕过滤仍保留号码，标签组编号不变。Rust 回归覆盖托盘句柄、取消库存、拒绝关闭和旧快照，网页模拟器同步编号压紧。


Move 精度回归覆盖 60/75/120/144/165/240/360/500Hz 的等时位移与边界反向，以及细粒度移动无鼠标回写、未变化帧无原生写入。此为逻辑测试，不能替代高刷新率显示器的视觉流畅度验收。

macOS 原生 Window 验收：`cargo test --lib native_macos_window_parity -- --ignored --nocapture --test-threads=1`。测试用 clang 编译 `tests/fixtures/macos-window-parity.m`，创建独立子进程的三个临时 AppKit 窗口，仅按子进程 PID 操作；退出时终止子进程并删除临时文件，子进程另有 180 秒 watchdog。覆盖完全重叠窗口枚举、移动和居中、实际左右分屏、Editor 完整库存和自动平铺事务、两轮 F 三态循环、按应用自动组合、含栏最大化／最小化／恢复、窗口和组内切换、解散及关闭；也验证无音频进程时的应用音量／静音／输出偏好。活跃音频 Tap 与多硬件输出仍需专门的音频验收。需要实际桌面、Accessibility 权限和 Command Line Tools；普通 CI 忽略此项。纯回归覆盖负坐标屏幕受限分屏位置、标签最小宽度、活动项滚入与滚动边界。第三方应用最小化动画、跨屏拖放及层级遮挡仍需交互验收。

首次原生验收可使用 `KEYSTEER_PROBE_REQUEST_ACCESSIBILITY=1 cargo test --lib native_macos_window_parity -- --ignored --nocapture --test-threads=1`，通过现有 AX 权限入口申请辅助功能权限并在同一测试进程中等待至多 120 秒；授权后自动继续。未设置该变量时缺少权限立即报错，不弹窗。已有权限直接运行；macOS 的授权对象／签名由系统决定，重建测试程序可能需要重新授权，不能保证一次授权永久有效。

为避免在系统设置中选择 Cargo 的裸测试文件，推荐使用 `python3 tools/test-macos-windows.py`。脚本按 Cargo JSON 输出定位库测试程序，生成固定路径 `target/native-window-tests/KeySteer Native Tests.app`，通过 LaunchServices 启动该应用，仅运行窗口原生验收，日志保存在相邻的 `result.log`。`--prepare-only` 只构建授权对象。应用的 bundle identifier 固定且使用本地 ad-hoc 签名；使用稳定的 Foundation 启动器执行 bundle 外的测试二进制；Rust 重建不改变启动器的签名。启动器自身变更时系统仍可能要求重新授权。授权由系统设置的正常辅助功能流程完成，不修改 TCC 数据库。

如果辅助功能开关已开而日志显示 `Failed to match existing code requirement`，旧条目的签名要求未更新，需要用户在系统设置中移除旧测试应用条目并重新添加当前 bundle；单纯关闭再打开可能无效。测试启动器与 Rust 测试二进制分离后，普通源码重编译不再改变负责授权的启动器。

原生探针的屏幕工作区由临时 AppKit 子进程主线程提供，避免 Rust 测试线程使用 Core Graphics fallback 时漏掉菜单栏／Dock 留白。失败日志保留请求矩形、实际 AX 几何和只读 Quartz 信息，用于区分约束、异步同步和测试环境错误。

macOS 原生性能：`python3 tools/test-macos-windows.py --performance --release`。复用已授权的稳定测试应用，1000 次先于等待的重复 mailbox 唤醒检查丢信号；临时 AppKit 窗口逐条接收变更，每次等 AX 通知确认后再发送下一条（2000 次，无移动定时器）；随后交错比较 20,000 对完整快照和几何读取，验证结果等价，写入 `target/native-window-tests/tabs-performance.csv` 的 p50/p95/p99。超时只检测测试故障，不驱动移动。

共享模拟基准也可在 macOS 运行：`KEYSTEER_TABS_BENCH_OUTPUT=target/native-window-tests/tabs-shared-macos.csv cargo test --release --lib tabs_geometry_baseline -- --ignored --nocapture --test-threads=1`。它覆盖 2/10/30 成员的协调层成本和外部窗口写入次数，不是 macOS 原生渲染或屏幕 FPS；原生 AX 耗时同样不等于 compositor 帧率，多屏混合刷新率的视觉效果需单独注明实测硬件。


卡片编辑区在 ModeStyleControls 中集中共享 card 与当前模式 ui 控件，CardPositionEditor 的拖动只更新 window.card.position，取消手势恢复先前值；positionFromPoints 测试覆盖反向拖动、点／线与百分比边界。CardStylePreview 随有效配置与主题响应式更新；颜色控件显示继承主题的真实默认颜色。
# 映射修饰键回归

`mapped_chords_*` 覆盖左右 Alt/Ctrl/Shift/Win 的全部非空组合、重复事件、释放无额外发送和
显式目标修饰键。Windows `mapped_chord_*` 检查原生批次中方向键期间无多余修饰键、批次结束
状态恢复、已松开的源键不恢复，以及常见组合的内联存储。Notepad4 原生验收应确认 Primary+J/K
只移动光标，长按重复，松开 Alt 不打开菜单，未映射 Alt 快捷键仍有效。

Window 资源回收回归：`cancellation_releases_inventory_before_worker_shutdown` 经真实 worker 队列验证取消后、线程仍活着时释放原生库存；`ended_ungrouped_session_releases_native_inventory_and_cached_snapshots` 验证没有分组历史的会话缓存容量释放；`ended_grouped_session_preserves_live_members_and_history` 与已有 worker_exit 测试验证持久组继续跟随。Mac 实机需用相同窗口集合重复进入／退出 Window 至少 30 次，同时记录 Activity Monitor footprint、resident 与 Instruments Allocations／Leaks；Windows 对应记录 private bytes、working set 和 handles。对比冷启动、首次进入、退出和重复周期平台值，并测重入／连续移动延迟；交叉编译和逻辑测试不证明实际内存降幅或帧延迟。

## 三项维护门禁的边界

API 默认 `send_chord_suspending` 自行汇总错误，恢复全部源修饰键后再返回失败，不依赖 support；`default_mapped_chord_restores_all_sources_after_suspension_failure` 验证暂停失败也会执行全部恢复。架构依赖规则保持不变。

Windows input 的 unsafe 文件预算为 9：新增的一处仅在 mapped-chord 测试中读取 `INPUT.Anonymous.ki`，读取前断言类型为 `INPUT_KEYBOARD`；生产 unsafe 数量不变；总预算对应从 368 增至 369，其他文件预算不变。

2000 目标 owned Hint 交付恢复最多 15 次分配、632,920 字节的原预算，普通 OverlayLabel 大小上限恢复 80 字节。窗口 placement／connector 仅在 WindowAnnotations 稀疏表中存储，不扩大普通标签。`window_annotations_are_sparse_shared_and_follow_sorted_merged_labels` 验证无窗口数据时不创建表、未启用引导线不存储样式、clone 共享与写时隔离、排序重映射、多屏合并及序列化往返。引导线避让回归同时覆盖启用和禁用，禁用的窗口背景不生成引导线，独立标注保留优化前的默认连接语义。全局分配统计仍需单独、单线程执行。

Window 卡片渲染回归额外从 TOML 分别编译 true／false／true，验证两主题的样式有无、实际场景共享样式和避让后的线条数量，覆盖开关重载。窗口背景使用编译样式，独立区域／组号的连接行为以优化前基准为准。

## 优化前行为基准

`window_scenes_match_pre_optimization_baseline` 对照提交 `c7bff963bef83a4a5a2ba67f1458e112e5ac6647` 实际生成的 1296 个 Windows 场景指纹，覆盖浅／深主题、引导线开关、1／5／16 窗口、双屏负坐标和混合 DPI、窗口底部／屏幕底部／屏幕中心位置、树布局及分组编号、重复避让与面板碰撞。比较全部绘制字段，只排除存储迁移的 placement／connector 元数据，浮点坐标以百万分之一像素规范化；基准 fixture 不能从待测实现自动更新。macOS 的紧凑标签物理几何不同，不运行 Windows fixture；跨平台逻辑测试验证任意角度起止点、静止线条不变和缓存一致性，macOS 仍需原生视觉验收。

`window_help_cache_restores_connectors_with_displaced_labels` 验证缓存与重建一致，源线条变化导致失效；`ended_dissolved_session_preserves_undo_and_redo` 验证退出后仍能撤销解散、重做分组。不能通过删除历史或放宽分配门禁换取优化。
# Window 上层定位回归

runtime 的 window_targeting 测试覆盖无修饰和临时 Normal 入口、冲突/none/改键、原 Grid/Recursive Grid 的 Tab/Space、跨屏路径保留、自然完成返回、同窗口会话和迟到结果不覆盖上层。共享后端 targeting_move 测试覆盖不同 DPI 的跨屏绝对定位、不回放鼠标和整轮一次撤销。原生窗口的视觉跟随仍需 Windows/macOS 桌面验收。


临时层回归覆盖完整层冲突、当前完整组合键优先于临时裸键、去激活键后的临时优先、`none`、穿透键和别名导出往返；网页模拟器同步同一顺序。Quick Switch 覆盖原动作即时执行、原始 Grid/Recursive Grid 选择、重复发送、松开无补发、即时进入 Idle 后长按面板、数字边沿配对与捕获丢失恢复。macOS Overlay 交叉编译不替代实机多屏验证：内屏/外屏分别为主屏，左右/上下/负坐标排列，混合 Retina 比例，`screens=current/all` 切换及逐屏编号、应用名、标题与原窗口对齐。

macOS overlay 回归 ll_displays_have_separate_local_origins_without_retina_coordinate_scaling、display_panels_follow_all_current_and_unplug_without_rebuilding_retained_content、clipped_scene_uses_only_each_display_intersection 验证逐屏原点、负坐标/上下屏、Retina 不影响逻辑坐标、all/current 切换、拔屏和图层缓存复用。这些测试在 macOS target 下执行；Windows 只交叉编译。实机需开启 Displays have separate Spaces，内外屏分别放置窗口，在两块屏幕分别进入 Window，核对所有编号/标题和窗口对应；切换 current/all，交换主屏、上下/左右排列及混合 Retina，再检查跨屏光标移动。


选窗状态保持：selecting_ungrouped_windows_across_screens_preserves_maximized_and_fullscreen_state 覆盖跨屏未分组窗口的直接选择、正反循环，断言最大化／全屏状态及还原矩形不变、无几何写入。Windows ignored native_selection_preserves_maximized_window_on_each_display 在各可用屏幕创建临时窗口，验证实际前台切换和最大化矩形保持，结束恢复原前台；不能替代第三方浏览器和 Mac 实机验收。

运行时 window_cross_screen_selection_warp_does_not_move_or_restore_windows 覆盖 screens=all、不同缩放的双屏、数字与 Tab 跨屏往返选择，消费真实 WindowResult 后验证鼠标到目标中心且不增加几何请求。该层回归覆盖原生激活测试未包含的 WarpPointer → PointerMoved 路由；window_targeting_* 继续验证 Grid/Recursive Grid 的显式窗口定位不受影响。


窗口枚举回归：window_visibility 测试跨屏窗口标题不同／缺失、完整重叠集合和其他进程／Space 歧义；all_screen_inventory_numbers_every_window_without_pointer_acquisition 验证 300 个跨屏窗口无需鼠标命中即可全部编号，current 范围仍正确过滤，且不改变焦点或几何。


## 异步响应与资源所有权回归

运行 `cargo test -- --test-threads=1`；分配预算测试使用进程级计数器，需要单线程测试。运行 `cargo clippy --all-targets -- -D warnings`，并交叉检查 macOS 的 aarch64／x86_64 目标。回归覆盖工作区磁盘锁阻塞下非阻塞提交和完成顺序、日志参数惰性求值、独立窗口及音频进度、请求屏障、取消／超时、部分写入撤销和标题更新后的恢复。

场景基准 `tests/fixtures/window-scenes-c7bff963.txt` 的 1,296 项摘要由历史提交 c7bff963 的渲染实现生成，不使用当前实现生成期望值。safety_budget 同时约束全局与各原生模块：本次改动前 HEAD 实际为 385 个 unsafe 表达式（旧门禁预算已漂移），收拢封装后为 383；这包含测试代码，不能解释为消除了所有原生 unsafe。

Windows 可显式运行 `cargo test --lib native_deferred_geometry_submission_and_readback -- --ignored --nocapture --test-threads=1`。该探针只操作自己创建的临时窗口，40 次采样打印包含准备读取的提交耗时和读回确认耗时；debug 构建、桌面调度和测试窗口消息泵会影响数值。它验证原生路径可用，不是优化前后基准或按键到像素测量。macOS 交叉编译不替代 macOS 实机验收。

本次 Windows 验证：全量测试 1,052 通过／83 ignored，Clippy all-targets 无告警；aarch64-apple-darwin 和 x86_64-apple-darwin 均通过 cargo check。显式运行普通几何提交、窗口还原与关闭身份、布局事务、最大化标签栏四项原生探针均通过。40 次 debug 几何探针的准备＋提交 p50／p95 为 5.01／5.37ms，确认 p50／p95 为 30.50／37.45ms；未进行相同环境下优化前后的端到端或 RSS 对比。


## 确认分配与 Normal 巡航优化验收

确认等待 10,000 次的 portable 回归要求零 allocation/reallocation 且不读取完整元数据；同步回退六个分支与直接同步执行结果一致且不增加快照读取。Windows `native_deferred_geometry_submission_and_readback` 额外要求 256 次原生读回零 Rust 分配、title/app 地址不变。Normal 编译前后配置结构同尺寸，多个速度／加速度／曲线及时间点与旧积分对照。

2026-09-22 三版本同场 release 基准，固定 affinity=4，每版交替 5 轮，20k 样本：Normal p50（135b5f4／修复前／修复后）18／19／15ns，p99 为 21／21／17ns；默认按键 p99 为 181／183／180ns，WASD 为 187／190／186ns。均为各轮分位数的中位数；Normal 样本是 1,000 次帧的批均值、按键是 100 次事件的批均值，不是输入到像素延迟。前轮 Normal 23→28ns 的差异没有稳定复现，不能把调度噪声直接判为逻辑回退。原始运行与说明保留在 target/perf-135b5f4/results-fixed.json、fixed-comparison.md。

修复后全量 1,055 passed／83 ignored，Clippy 无告警，macOS ARM／Intel 编译通过，4 项 Windows 自有窗口原生测试通过。macOS 编译不等价于 AX 实机性能验收。

## 异步确认与配置回归

`window_session/async_tests.rs` 覆盖布局在途上限、提交部分失败后的回滚、取消等待已提交写入、最大化入口恢复、EndEdit 取消、撤销提交边界，以及快速窗口提前确认但不提前提交历史。Grouped 测试覆盖非活动代表编号到活动成员的转换、标签栏占位、无元数据重读和确认回声过滤。`configuration_async.rs` 用受控阻塞 repository 验证 Engine 不等待磁盘工作、连续候选使用最新 repository、失败不覆盖有效配置。logging 测试通过注入时间验证错误合并、计数 flush、键与时间窗口隔离，不依赖真实 sleep。

执行 `cargo test --all-targets -- --test-threads=1`、`cargo clippy --all-targets -- -D warnings`，并交叉检查两种 macOS 架构。分配断言需串行执行；Windows 上的跨平台编译不替代 macOS 桌面验收。

`runtime/tests/text_input.rs` 覆盖文字透传、普通 Enter 返回与 Send+Mode 序列、返回键与严格修饰匹配、别名／组合键配置往返、移除 Enter 返回绑定及停止手势／释放 latch。输入法候选确认与真实应用提交仍需桌面验收。

临时文本输入验收：默认 Normal 标记下，`cargo test --all-targets -- --test-threads=1` 为 1,085 passed／84 ignored。保留本地已有标记 `N` 后，integration 为 24 passed／1 failed，失败仅为 shipped 与 embedded 的 Normal/N 默认值差异；新增 Text Input 配置一致性通过。Clippy 无告警，网页 121 项测试和 TypeScript 检查通过，macOS ARM／Intel 编译通过。原生输入法和应用提交未做实机验收。

配置写法统一后的回归：Text Input 专项 10 项通过（含直接读取注释示例的 36 个编辑组合及 Enter 序列）；网页 121 项和 TypeScript 检查通过，Clippy 无告警。全量执行跳过此前 Normal/N 默认值差异测试后为 1,086 passed／84 ignored／1 filtered out。

`text_input_temporary_normal_routes_plugin_screen_commands_to_normal` 用真实插件、双屏及物理／字符事件验证 S 正反切屏、基础模式不切换、释放 Primary 后 S 恢复透传；`text_input_default_indicator_is_disabled_in_code_and_shipped_config` 验证代码／发布默认均隐藏 Text Input 文字且保留 Normal 指示器。

临时切屏／指示器回归：库测试 1,075 passed／84 ignored，唯一失败为当前 TOML 的 Normal `.`／`/` 横向滚动绑定与内置默认差异；Text Input 双屏／文字隐藏及原 Window 临时切屏测试通过。Clippy lib/tests 和 docs:check 通过。all-targets 构建因运行中的 target/debug/keysteer.exe 无法覆盖而中断，未替换用户正在运行的程序。
