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

macOS 更新器测试覆盖握手、签名、交换与回滚、退出通知及启动状态不确定时保留两份应用包。登录桌面上显式运行 `cargo test --lib launch_services_preserves_arguments_and_handshake_and_stops_failed_startup -- --ignored --test-threads=1`，以临时无界面应用验证旧实例存活时交换应用包、新旧版本的真实 LaunchServices 启动、参数和确认环境传递、失败退出与回滚启动入口，不启动生产应用或请求权限。`cargo test --test macos_capture_permissions` 编译实际截图桥接，仅替换显示器与 ScreenCaptureKit 边界，覆盖预检查失败但实际截图成功、枚举／截图拒绝、普通错误、取消及超时后的恢复；不截图用户桌面。权限跨版本保留仍需同签名正式包的实机升级验收。

跨平台按键测试通过配置别名及 `KeyChord` 解析实际修饰键，再转换为物理按键；内置 `primary` 不一定存在于用户别名表。布局断言沿用后端坐标约定：Windows 检查 DPI 缩放后的矩形，macOS 检查 AppKit 逻辑点，不能把 Retina backing scale 再套入 Windows 几何函数。保留包含、重叠、字号及边距断言，不通过跳过平台或放宽容差修复测试。

覆盖层回归包括位置快路径后返回上次完整场景坐标，以及 macOS 相同矩形的显示刷新、保留当前位置和拔屏。Windows 固定坐标像素回归需关闭系统全屏放大镜，并确认测试图层确实出现在捕获结果中；游标使用与底色不同的不透明色，指示器另行验证可见、移动与恢复，不能以两次相同的桌面背景或只观察一个图层充当渲染一致性证明。macOS overlay 单元测试跳过原生窗口创建，只验证状态转换；交叉 `cargo check --tests` 也不执行这些测试，默认不编译 Objective-C 桥接。AppKit/屏幕取色验收仍需在 Mac 检查混合 Retina 显示、改变主屏／分辨率、静止与连续移动时热插拔、首次搜索背景复用，以及取色失败后的恢复；原生像素和耗时结论不能由其他平台测试替代。

公共 `overlay_geometry` 回归覆盖 Retina 倍率、Windows 100%／125%／150%／200% 预缩放样式、负坐标和连续移动中的固定像素尺寸，以及双行徽标共同右边界；两端另验证原生局部坐标转换。`cargo test --lib platform::macos::display::overlay::tests -- --include-ignored --test-threads=1` 执行离屏 CoreText／Core Animation 检查：字号度量、缩放和尺寸变化时重绘、单纯移动时复用位图。Windows 的 `directcomposition_smoke` ignored 测试还比较小数锚点在同一像素内移动、完整帧及位置快路径的实际画面，并确认不重画表面。交叉编译不能替代这些原生测试或内外屏实际清晰度验收。

正式构建使用 `cargo build --locked --release --bin keysteer --no-default-features`。macOS 上优先通过 rustup 代理或 `rustup run <toolchain> cargo ...` 启动构建；直接调用工具链目录里的 Cargo／rustc 时，部分工具链可能因缺少动态库搜索路径而让 `rust-objcopy` 无法加载 `libLLVM.dylib`，导致去符号失败。不要通过关闭 `strip` 或写死本机路径来隐藏警告。单元测试由 `cfg(test)` 隔离，测试依赖放在 dev-dependencies。主包不暴露基准接口或对应 feature，`--all-features` 不依赖本地性能目录。性能测试、适配代码、独立锁文件及原始结果只保留本地，不纳入提交；用户要求发布的已核验汇总可用于性能页面。需要访问内部实现时在隔离副本内适配，不能为基准扩大生产 API。

## 性能与原生验证

以下 `tools/` 命令是本地工具的使用示例，需先在隔离副本中准备相应适配；主包不提供供这些工具调用的公开测试接口。A/B 两边使用同一工作量、工具链、锁定依赖和构建参数。

- 区域提前交付：`scan_progress`、`scan_accumulator` 和 `scan_fusion` 的模块测试覆盖来源未声明、区域重叠／预览重复占用、OCR halo 与接缝扩展、元数据保留、完成标记 FIFO 及旧代次隔离；`window_visibility` 同时检查 macOS Quartz／AX 匹配范围。Windows 实机 A/B 仍须核对各来源完整目标、最终目标及实际提交过又被撤销的轮廓，不能仅统计终态耗时。
- `src/tests/performance.rs` 和模块内 ignored 测试覆盖分配与原生探针；先读测试的环境要求，分配计数串行运行。
- `cargo bench --manifest-path tools/perf/Cargo.toml --bench core_hot_paths`：核心 CPU 路径；`--bench notification_queue`：通知队列。
- `cargo bench --manifest-path tools/perf/Cargo.toml --bench scan_stream -- --fusion-only` 比较就绪数据的融合 CPU 成本；`-- --async` 使用实际融合 worker 比较首份额、提交耗时、大来源期间的小来源延迟与总耗时。`-- --async-hint --common` 补测 200／300／400 标签的实际异步融合与共享 Hint 场景生成，分别记录首次场景、全量完成及后到小来源响应。基准不包含原生识别或系统合成器，不能冒充输入到像素响应。
- `tools/uihint-replay/` 是独立的流式识别回放包：`cargo bench --manifest-path tools/uihint-replay/Cargo.toml --bench scan_replay -- --samples 32 --output replay.json` 默认测试后到 OCR 的元数据更新，并在每次消费后派发一次生产预热计时器，最后排空剩余预热；`--no-maintenance` 可单独诊断就绪数据处理；`--first-scene-only` 固定首份额 16 项、即时消费邮箱，并在首条 `ShowOverlay` 时停止计时，用于快速单独核对首批响应（正确性仍回放完整扫描）；A/B 脚本对应开关为 `-FirstSceneOnly`；`--fixtures <目录>` 可加入包含 bounds／contours／ocr／ax／visible 的本地冻结结果，`--verify` 增加逐项交付，`--features allocations` 单独测分配。程序遍历来源顺序、批次及邮箱合并，核对参考交付顺序的完整最终目标，并记录仍存在区域的标签变化、预热前首份场景、含后续维护的总耗时及最慢份额。私有界面数据留在本地，不写入仓库夹具。异步 worker／邮箱／Hint／最终场景的确定性组合回归在 `src/modes/hint/tests/scan_replay.rs`，另覆盖纯视觉、缩放及旧请求隔离。
- 回放 A/B 使用相同依赖与工作量、独立 target 目录，编译和检查全部结束后交替运行至少三轮，保存为 baseline-N.json／candidate-N.json；`tools/benchmark-uihint-ab.ps1` 以相同亲和性和 Normal 优先级交替运行并记录环境。`python tools/compare-uihint-replays.py <目录>` 先核对原始输入的逐位坐标／内容指纹，核对最终结果与标签变化，再比较各场景各分位数的轮次中位数；默认同时超过 3% 和 2 微秒视为需调查的 CPU 变化，分配不使用时间容差。明确增加目标的修复可用 `--changed-fixture` 标记预期的识别／编号差异，性能与分配门槛不变；其余界面仍严格核对完整结果。保留失败记录并排查，不用调整门槛掩盖回退。这些指标只包括已就绪数据的融合和共享展示，不包含截图、OCR／UIA／AX 遍历或系统上屏。
- `tools/` 提供 A/B、整进程和原生测量入口。正式性能比较不启用 `perf-probe`，基线与候选用独立 target 目录。

多核比较先读取系统 CPU 拓扑，记录亲和掩码对应的逻辑处理器及物理核心；两个置位不一定是两个独立核心，同一核心的 SMT 线程必须单列。分词尾延迟用真实后端的请求提交、工作线程开始、原生取词、结果投递及场景提交分别定位，再关闭探针做同负载 A/B；仅接后台分词器的无界面驱动不能代替原生消息循环、输入线程优先级和覆盖层调度。场景提交不是屏幕像素显示，固定 OCR 输入与内存复制替身的范围也须标明。

墙钟尖峰应与线程 CPU 周期或运行时间对照，不能直接归因于原生分词的计算量；区分抢占与原生等待需系统调度证据，采集权限不足时保留未确认结论。异常先做同一二进制 A/A，并核对完整结果、各轮原始样本和提交节拍；增加样本或换成真实后端后未复现，不能宣称原有尖峰已修复。微基准收益必须再通过端到端比较，不能据此引入缓存、并发或替换原生遍历。

移动／滚动冷启动同时记录后端构造、构造后的首个原生显示帧等待及两者合计；标明 CPU 亲和性与额外负载。固定单核即使没有压测线程，GPU／COM 等并行预热也会竞争该 CPU，不能把它称为普通多核场景。分段诊断区分资源准备、工作线程被唤醒后的等待、原生等帧及投递；原生等待的墙钟时间也可能包含调度或驱动资源争用，不能直接当成显示刷新周期。比较总等待与构造后等待，不能只用更早返回构造或某一轮的显示相位说明优化有效；原生时钟首帧不等于按键到真实移动／滚动或屏幕像素的延迟。

用户首次使用的验证以配置加载／编译、后端及 Hook 启动、引擎初始状态和 Idle 激活完成后进入第一次 poll 为就绪边界；后端构造返回和 backend started 日志均不能替代它。分别测就绪后立即使用、明确空闲间隔后的首次使用及同进程再次使用，区分按下立即执行的短步和首个连续显示帧，并记录每次操作时 GPU 设备、显示时钟和滚动线程是否已就绪；引擎就绪不等于全部后台预热完成。移动、滚动分别作为独立首次动作，组合动作再检查逐个释放不影响剩余动作。测试驱动的空闲等待、输入投递与最终原生输入替代必须明确标注，不扩大生产 API，也不能把到后端执行入口的时间称为系统输入或屏幕像素延迟。

GPU 设备就绪、首次场景提交完成和输入执行分别记时，不能将设备构造结束称为任意场景资源已全部预热。测试驱动的计划空闲时长与实际恢复时刻也须分别核对；线程恢复晚于请求期限时，保留长尾与完整时间线，不把后续手势当成严格固定间隔的数据，也不凭时间上的重合认定具体原生调用是原因。

`tools/benchmark-core-ab.ps1 -Mode Gate` 交替运行 Hint 扩展场景、持续移动帧、小规模标签和引擎按键派发，保存实际 EXE 的 SHA256 与每轮原始输出。`compare-core-benchmarks.py` 汇总各轮分位数的中位数，默认 p50 或 p95 同时超过 3% 和 2 ns 就返回非零；p99 保留为尾延迟诊断。阈值用于发现需要调查的变化，不代表阈值以内没有退化。正式比较至少三轮，异常时增加轮次并保留原始失败记录；计时期间不运行编译、测试或其他基准。CPU 门禁不能替代原生输入到像素验证。连续无节拍的 DirectComposition 提交基准不代表真实输入频率；尾延迟异常先复跑同一二进制 A/A，再以计时区间外等帧的对照检查提交节拍影响，保留所有轮次，不把单批 p99 差值直接归因于代码。

基准子进程默认使用 `Normal` 优先级；`-BenchmarkPriority AboveNormal` 可用于排查调度干扰，须单独保存结果，不能替代默认优先级的响应测试。环境记录包含优先级和计时器分辨率；比较器同时检查成对轮次与样本批次，避免工作量不同的结果混比。
- 原生变更检查对应 target，并在对应 OS 验证输入、权限、窗口、显示与清理。交叉编译只证明编译兼容。`native_macos_window_parity` 还直接检查鼠标窗口枚举回退，并确认恢复的 AX 窗口与编号枚举保留同一身份。
- macOS AX 批量读取：`cargo test --test macos_ax_batch` 在各宿主执行实际回退策略，检查字段顺序、缺失属性、调用前／期间取消与临时值释放。`cargo test --lib native_batch_` 在 Mac 检查真实 CF／AXValue 类型、错误槽位、数组形状及引用回收，无需辅助功能权限。`cargo test --lib native_ax_batch_matches_individual_reads_and_cancels_after_first_target -- --ignored --test-threads=1` 需 Mac 辅助功能权限及稳定的前台窗口；按逐项／批量／逐项读取比较完整目标和顺序，确认逐节点交付及首项后取消。它不是双屏端到端性能测试；两屏最终标签补齐、真实 provider 超时和进程峰值内存仍需实机单独测量，不能从模拟调用次数或交叉编译推算提速比例。
- macOS 取色回归夹具：`clang -std=c11 -Wall -Wextra -Werror tests/fixtures/macos-point-capture.c -framework CoreGraphics -o /tmp/keysteer-point-capture && /tmp/keysteer-point-capture`；验证 Retina／副屏坐标、固定像素不混入变化的邻域颜色，以及错误尺寸拒绝。非 macOS 可省略 framework，仅运行坐标测试。
- macOS OCR 回归：`cargo test --test macos_ocr -- --nocapture` 在内存生成普通／Retina 网页小字，检查逐块识别、原生请求弱引用、带行填充的只读 BGRA／灰度回退及子图共享像素最终单次释放。夹具直接编译当前桥接；正常重新识别还通过 Cargo 构建的真实应用二进制及其平台入口执行，避免只测夹具、遗漏启动分派。另用同一私有工作入口提交 16 块永久挂起故障注入（GCD 实际调度数可能少于请求并发），验证强制结束／父管道 EOF 后子进程退出，并反复启动新进程识别中文及英文小字。纯 Rust worker 回归在各宿主启动真正不返回的子进程，覆盖取消、替换、超时和丢弃旧会话后同一执行槽恢复，不使用放行通道模拟旧 OCR 结束。回归打印真实取消、重新识别的首个非空结果及完成耗时；并发 A/B、双屏来源时间线和进程内存曲线通过隔离副本保留在本地。首次模型编译长尾与预热后数据分开记录，原始默认设备／CPU／GPU 对照不互相替代。这些是 OCR 诊断样本，未包含截图、AX、融合与系统上屏，不能代替实际交互和进程启动／传输成本测量。
- 常规回归将同一契约的配置变体合并到已有测试，各种取消路径各检查一次；大轮数重入、压力及性能 A/B 留在本地诊断，不扩大日常测试矩阵。失败先区分本轮回退和已有基线问题；忽略测试不等于验收通过。
- 扫描复用回归：公共 `scan_reuse` 测试覆盖期限刷新、容量收缩、到期唤醒、非 Send owner 的同线程销毁及关闭中的有界待办；原有 Windows provider 隔离测试通过同一任务票据验证恢复。Windows 可用 OCR 语言包时，运行 `cargo test --lib native_ocr_reuses_engine -- --ignored --nocapture --test-threads=1`，检查同一引擎重复识别内存文字、随后空白图没有旧结果、失败后丢弃资源；它不需要截图权限，打印的单图耗时不等于端到端提速或正式 A/B。

## 网页与发布

所有目标的 `build.rs` 都先调用 `build/compatibility.rs`，从 `src/config/compatibility.json` 生成静态 Rust 规则；共享 JSON 或生成器变化会触发重建，非法规则直接使构建失败。JSON 解析和规则分配仅发生在构建进程，生成文件留在 `OUT_DIR`，无需随程序发布；配置入口的运行时职责见 [配置](03-configuration.md)。

网页使用 Node.js 24，先用 `npm install --global pnpm@latest --allow-scripts=pnpm` 安装或升级 pnpm，再运行 `pnpm install`；npm 12 需要允许 pnpm 自身的安装脚本以放置原生可执行文件。本地通过 npm 管理 pnpm，不通过 `packageManager` 固定版本；Pages 每次从 npm 的 `latest` 标签解析最新正式版，并以解析出的确切版本缓存工具。`pnpm-workspace.yaml` 通过 `runtimeOnFail: ignore` 将语言工具链留给既有工具管理，避免仅安装文档依赖时下载 Rust。VitePress 2 当前固定使用 alpha 版本，采用 Vite 8 的 Rolldown／Oxc 构建链，不安装 esbuild；自定义 Markdown 测试须等待 `renderAsync` 完成异步语法高亮。

网页代码位于 `docs/.vitepress/`；变更时按范围运行 `pnpm docs:test`、`pnpm docs:check`、`pnpm docs:build`，视觉交互另做浏览器验证。

用户文档的中英文页面、章节、示例和图表成对维护；改写导读时保留原有标题锚点。开发导航通过 `developmentSidebar` 共用页面清单，新增译文同时加入语言切换脚本的 `englishPages`；正文目录自动收录二、三级标题。

模式演示共用 `components/ModeVideo.tsx` 的播放器、标题和说明样式。`file` 引用 `docs/public/video/` 下的本地文件；`src` 优先指定视频 URL，支持远程 MP4 或站内路径，并统一通过 `withBase` 处理部署前缀。URL 来源同时提供直接打开链接，便于在站外播放受限时前往视频页面。Bisect 模式页分别引用本地 `bisect-zh.mp4` 与 `bisect-en.mp4`，随站点同源发布。README 使用已上传视频的稳定 URL，不保存临时签名地址；生成工程与复制步骤不纳入仓库。

用户性能页面位于 `docs/performance.md` 与英文对应页；`components/Performance.tsx` 从 `.vitepress/data/performance.json` 读取已核验的公开汇总，复用同一组数值生成亮点、图表和表格，不添加图表依赖。Windows 内存主数据采用任务管理器默认口径的专用工作集，采样峰值与系统峰值工作集分开；响应与内存分批测量时保留各自来源。首页卡片、README 与更新日志使用相同测量口径。公开数据不包含具体硬件型号或本地路径；原始采样与诊断记录继续留在本地。

`core_cpu` 单独保存核心 CPU 微基准的日期、范围、样本批次和多轮分位数；批量均摊的 p95／p99 不能称为单次事件尾延迟。README 与页面亮点明确标注 CPU，系统坐标／截图观测与原生识别另列。截图读回的完成时刻包含观测工具开销，不能称为核心耗时或精确上屏时刻，也不能减去估算的截图耗时后充当新测量。

原生阶段可在隔离副本中用共享内存原子时间戳记录实际引擎收键、处理返回及渲染提交；公开汇总注明适配代码开销与起止边界，提交不能称为真实显示器上屏。双屏 UI Hint 分别记录两屏固定首行区域的标签场景提交与融合后的扫描终态，每轮核对两屏完整目标数量；识别需要的原生截图属于实际工作负载，外部验收截图不进入阶段计时。内存另用本轮无适配代码的发布构建重测，不沿用旧批次或把计时测试构建的内存混入主指标。

更新日志保留 Markdown 版本标题供发布脚本提取；`release-history.ts` 仅在渲染时折叠旧版本，主题以紧凑分隔行展示。中英文页面的 `outline: 2` 由标题自动生成目录；主题 Layout 在目录点击、带锚点进入和内容更新时展开对应版本，再交给 VitePress 定位。

Mermaid 图表直接使用官方 `mermaid` 包，不依赖 VitePress Mermaid 插件。`mermaid-markdown.ts` 将 `mermaid`／`mmd` 围栏转换为主题注册的 `MermaidDiagram`；组件仅在浏览器挂载后加载 Mermaid，串行初始化与渲染以隔离全局主题配置，切换主题时重绘，卸载后忽略旧结果，失败时显示错误和源码。隐藏测量容器使用与正文相同的 `.vp-doc .mermaid` 层级，使标签换行和字重参与尺寸计算；临时布局 DOM 必须清理。Vite 的 `optimizeDeps.include` 保留 `mermaid`，确保 ELK 等 CommonJS 依赖先转换。

`docs:dev`、`docs:check`、`docs:build` 通过 `scripts/docs.mjs` 在同一个 Node 进程中先同步资源，再启动对应 CLI，避免嵌套启动 pnpm 和额外的 Node／命令包装。CLI 选项原样透传；正式构建仍从 GitHub 获取并校验所选 Release。`latest-release.ts` 兼容 `dev`／`serve` 字符串及 VitePress 开发 CLI 转交的选项对象，网络失败可继续预览；`build` 和未知命令始终严格校验。

正式产物走 `packaging/` 脚本和 `.github/workflows/`，检查应用身份、资源和签名。纯文档修改验证链接、代码路径和 `git diff --check` 即可。

仓库根目录的 `Casks/keysteer.rb` 和 `bucket/keysteer.json` 分别作为自定义 Homebrew tap 与 Scoop bucket，安装时显式提供当前仓库 Git URL。`packaging/package-managers/generate.py` 从正式 Release 元数据生成四架构的下载 URL 与 SHA256；提供本地附件时重新计算并核对摘要，缺包、非法来源、草稿和预发布均拒绝写入。Scoop x64 固定选择无后缀的默认 AVX2 包，兼容版／AVX-512 附件不会覆盖默认选包。Scoop 升级／卸载前用 hooks 备份用户 TOML 和 `workspace.ksw`，安装前还原；避免原子保存替换文件后破坏持久化硬链接，默认配置和程序每次使用新包。macOS 沿用 Application Support 目录。

`build.yml`（Build）是唯一编译与发布入口；`pages.yml`（Pages）保留文档部署入口。Build 的 `build_mode` 默认 `cross`，可显式选 `native`；`platform` 支持 windows／macos／all。tag 推送自动用 cross 发布；手动支持平台选择、发布开关（默认开启）和 release/pre-release 类型。发布开启时始终编译两平台全部基础包，cross 另含兼容／AVX-512 两包；只构建时才按平台筛选。Cross 在 Ubuntu 编译，Native 使用 Windows 2025 与 macOS 26；构建方式统一显示在运行标题，任务名保留平台、架构与编译分类。未选模式的任务跳过，不启动 runner。

Build 的 Prepare 任务同时负责入口转发与源码校验：tag 推送和非默认分支手动运行只转发一次到默认分支的同一 `build.yml`，传递源码 SHA、tag、入口 run id 和入口 commit；默认分支运行直接构建，不再次转发。删除 tag 不运行。实际编译保持默认分支缓存作用域，checkout 固定为所选提交；显式 tag 必须指向该提交且已包含在默认分支历史中。完整 cross tag 发布共 10 个 runner job：入口、源码校验、四个 Windows／两个 macOS 构建、发布与清单同步、Pages。清单同步复用发布 runner，入口清理复用 Pages runner。

`.github/scripts/release.py` 校验附件并调用 `tools/compose-release-notes.sh`，从 `docs/releases/index.md` 提取当前版本，再追加 `.github/release-notes.md` 的通用安装／兼容说明。它校验远端 tag 的最终 commit；存在时保留 tag 对象及注释／签名，不一致则报错，缺失时才创建。cross 可重建同名 Release 和附件，但不删除 tag；native 拒绝重复 Release。预发布不占用 Latest。默认分支发布共用 concurrency 组，新运行取消旧运行，入口使用独立组以免与实际构建互相取消。

发布任务在 Release 成功后继续读取 latest 正式版；若它就是本轮发布，直接复用本轮安装包并核验摘要，否则仅下载该正式版的四个基础包。生成的 Homebrew／Scoop 清单提交到默认分支；预发布不会成为清单版本。清单生成器及测试从默认分支单独检出，构建旧 tag 时也使用当前发布工具。需要 `GITHUB_TOKEN` 的 `contents: write` 且默认分支允许 bot 提交，受保护分支需按仓库规则处理。包管理测试入口为 `python -m unittest discover -s packaging/package-managers -p 'test_*.py'`；PowerShell 可用时执行真实的配置升级 hooks。

Pages 通过 `workflow_call` 在 Build 内执行，同时保留独立手动入口；调用者 ref 保持默认分支以满足 github-pages 环境规则。仅在所选平台构建和发布／清单同步成功（或未选发布而跳过）后部署；发布使用本次 release tag，仅构建使用 `latest`。站点 base 与 `GITHUB_PAGES` 在构建步骤显式设置，不额外查询未使用的 Pages 元数据。Pages artifact 使用 1 级压缩，优先减少上传前的压缩耗时。官方 `deploy-pages` 使用 1 秒初始状态查询间隔并保留自动退避，减少快速部署完成后的确认等待；它不改变 GitHub 服务端的发布时间。部署实际成功后复用同一 runner：`.github/scripts/cleanup_caches.py` 读取仓库全部缓存分页，按最后访问时间删除闲置超过 3 天的缓存，仓库变量 `ACTIONS_CACHE_MAX_IDLE_DAYS` 原样传给脚本，缺失或为空时由脚本统一使用 3 天，可改为其他正整数天数；按缓存 ID 删除，不按 key 批量删除，近期使用的旧缓存也保留。缓存清理失败只记录警告，不影响部署和后续入口清理。最后调用 `.github/scripts/cleanup_entry.py` 检查所选模式的全部阶段结果，并删除对应且成功完成的入口运行记录。由于入口与实际构建共用 `build.yml`，入口清理还查询 jobs，确认只有 Prepare 成功、其余任务全部跳过；含实际构建／部署工作的运行不会被删除。失败／取消、来源不匹配或无入口 id 时保留入口记录。版本 tag、实际 Build 记录及无关运行始终保留。Prepare 和清理步骤需要 `actions: write`；Pages 使用 `contents: read`、`pages: write`、`id-token: write`。发布与清理测试入口是 `python -B -m unittest discover -s .github/scripts -p 'test_*.py'`。

`packaging/windows/package-cross.sh` 对已编译 EXE 使用现有 `WINDOWS_SIGNING_PFX_BASE64`、`WINDOWS_SIGNING_PASSWORD` 和可选 `WINDOWS_TIMESTAMP_URL` 完成 Authenticode 签名、RFC 3161 时间戳和签名校验；缺少证书或签名失败时不上传包。默认 AVX2 版产物名称与 `package.ps1` 一致：`KeySteer-v<version>-<target>.zip` 内含 `KeySteer/KeySteer.exe` 和 `KeySteer/keysteer.default.toml`；x64 兼容版／AVX-512 版在 target 后追加 `-compatible`／`-avx512`，上传 artifact 身份也独立，发布时校验六个精确文件名。更新器复用同一套完整 v3／v4 特性要求：编译期特性识别当前构建，运行时 CPU 和系统可用特性选择支持的最高等级（AVX-512 → AVX2 → 兼容），不受最初下载包影响。新版本正常升级；同一版本但构建等级不同也下载切换，已经匹配时不重复下载，不降级版本号。GitHub Release API 直连失败后，只通过 `gh-proxy.com/https://api.github.com/repos/dccif/KeySteer/releases/latest` 重试，不使用 jsDelivr。两路共用包身份、大小和摘要校验；镜像检查成功时下载优先走相同镜像，失败再试直连。旧版更新器仍请求无后缀文件，因此不支持 v3 的旧电脑首次迁移需手动下载兼容包。签名校验显式信任提供的证书链并验证叶证书指纹，不代表公共信任或 Windows 原生运行验证。原生构建通过 `build_mode=native` 选择；两种模式使用相同的产物名称和发布说明来源，原生模式仍拒绝重复 Release。

两种构建模式复用 `.github/actions/setup-rust`，统一从 `rust-lang/rust` 官方 Release API 解析最新稳定版本，由 `dtolnay/rust-toolchain` 安装工具链与目标架构并设置构建日期。仅在 Cargo.toml 与 Cargo.lock 的项目版本不同步时调用 `cargo update`，避免每次初始化都更新索引。ZIP 上传关闭二次压缩；原生 Windows 签名证书在打包结束或失败后清理。

交叉工作流通过带 `GITHUB_TOKEN` 的官方 Release API 查询 LLVM 和 cargo-xwin：`llvm/llvm-project`、`rust-cross/cargo-xwin`。LLVM 下载 Linux X64 工具包（优先 zstd），按 Release API 的 SHA256 校验，支持 1 GiB zstd 解压窗口，解压后将 bin 目录置于 PATH 首位；不访问 `apt.llvm.org`。cargo-xwin 由 `taiki-e/install-action` 下载并校验上游预编译程序，禁用 fallback，避免从源码安装。系统库和签名工具使用 Ubuntu 软件源。

两种构建模式通过共享 Rust 初始化缓存工具链及第三方依赖下载，不缓存工作区源码、`target` 或逐提交 EXE。下载缓存关闭 Swatinem 的自动环境哈希，键只包含宿主与 Cargo.lock 中外部依赖摘要，项目版本和 CPU 变化不产生新下载缓存。交叉工作流在同一 Linux 宿主安装四个 Rust 目标，共用工具链缓存。

`.github/actions/setup-llvm` 为两个平台共用官方 LLVM 安装缓存和 apt 下载缓存。LLVM 仅在未命中时下载、验 SHA256、解压，随后裁剪为两平台共用的编译工具、共享库及 Clang 资源目录；安装验证成功即保存。不缓存 LLVM 调试器、分析工具和开发静态库。系统包先检查 runner 已安装内容，只补缺失项，安装失败时才刷新 apt 索引；Windows 签名工具仅在 Windows 任务安装。apt 下载缓存按 runner 镜像版本和包列表复用。SDK/CRT 由 `cargo xwin cache xwin` 按 cargo-xwin 默认渠道独立准备，缓存按工具版本与目标架构复用，不额外追踪微软新的 VS 大版本。macOS SDK 和 rcodesign 同样在安装后保存。缓存恢复有传输/解压开销，apt 仍需安装；闲置缓存淘汰见上面的部署清理流程。签名证书不进入缓存，项目与依赖每次重新编译。

macOS 默认从 `joseluisq/macosx-sdks` 社区清单自动选择最新 SDK 并校验摘要（可用仓库变量覆盖），配合官方 LLVM 的 Clang 与 Mach-O LLD，在 Linux 编译 Objective-C 桥接并打包。`packaging/macos/prepare-sdk.py` 校验 SDK 结构；`configure-cross.sh` 为目标配置编译器、归档器及链接器；`package-cross.py` 保持应用身份与 ZIP 布局，生成 PNG-backed ICNS，调用 rcodesign 签名并可选公证/装订。两种构建模式共用 `signing.py`：从 P12 提取公开证书、校验源码中的固定 SHA256 指纹，再生成固定 bundle identifier 与叶证书哈希组成的签名规则；公开 DER 与编译规则只存在于临时目录，成功或失败后均清理，无需纳入 Git。CI 使用 `MACOS_SIGNING_P12_BASE64`／`MACOS_SIGNING_PASSWORD`，发布缺少身份时失败，仅非发布构建允许 ad-hoc。原生 GitHub runner 显式开启临时证书的系统信任以验证自签名，普通本地构建不修改信任；自签名不支持 Apple 公证。其他配置与工具限制见 [Linux macOS 打包](../../packaging/macos/README-cross.md)。

Windows x64 固定三个编译级别：兼容版 Rust `target-cpu=x86-64`、C `/clang:-march=x86-64`；AVX2 版两者使用 `x86-64-v3`；AVX-512 版两者使用 `x86-64-v4`。不使用 runner 的 `native`，兼容版遵循工具链的 Windows x64 基线且不要求 AVX／AVX2／AVX-512。Windows ARM64 和 macOS 保持各目标默认 CPU。release 的 O3、fat LTO 和单代码生成单元不变。

网页 `DownloadSection.tsx` 的 Windows x64 默认入口固定选择无后缀 AVX2 版，仅检测 OS／架构，不推断指令集。兼容／AVX-512 在选定 Release 存在对应附件时才显示为手动选项；选包提醒只说明一般用默认版、无法启动尝试兼容版；启动后任意包均可通过「检查更新」自动选择最高支持等级，同版本也可切换，无需手动核对指令集。`latest-release.ts` 按不同文件名尾部隔离六种附件，可选版不能覆盖默认 URL。

`KEYSTEER_CROSS_WINDOWS=1` 在非 Windows 宿主上启用 C 桥接和图标/manifest 编译，需要完整交叉工具链；未设置时保留不依赖 SDK 的跨平台类型检查。MSVC 交叉工作流显式设置 `RC_PATH=llvm-rc` 和空 `CROSS_COMPILE`，避免 winresource 0.1.31 初始化 GNU 前缀时误报 ARM64 未知目标。跨宿主的工具链、SDK、路径和签名时间戳可能改变哈希，不能据此断言行为不一致。

`KEYSTEER_CROSS_MACOS=1` 在非 Apple 宿主显式启用完整 Objective-C 桥接编译，需要 SDKROOT、Clang 与目标链接器；未设置时仍允许不安装 SDK 的跨平台类型检查。

- macOS 撤权回归：`cargo test --lib permission_ -- --test-threads=1` 覆盖无通知不检查、后台检查阻塞时真实处置邮箱仍能交付、拒绝后立即放行、检查期间新通知和退出后的迟到结果；原生循环测试要求终态回合返回前已完成 shutdown。真实系统设置关闭／删除权限仍需实机确认，模拟通知或能力拒绝不能声称验证了 TCC 的通知投递。
