# 改动导航与不变量检查

## 按任务找文件

| 需求 | 主修改点 | 通常还要检查 |
| --- | --- | --- |
| 新增/修改动作 verb | `src/api/binding.rs` | `command_executor.rs`、default TOML、配置文档、模拟器分类 |
| 修改按键匹配/别名 | `src/api/input.rs`, `src/config/mod.rs` | `input_router.rs`、integration tests、网页 `bindings.ts` |
| 修改模式切换 | `runtime/mod.rs` | `command_executor.rs`、所有 Mode 的 Activated/Deactivated |
| 修改 Finish/click 语义 | `src/modes/mod.rs` + targeting Mode | 生命周期验证、Engine semantic Clicked tests |
| 修改 Normal 移动 | `src/modes/normal.rs` | 两端 frame clock、pointer config、实机手感 |
| 修改 Grid 绘制 | `src/modes/grid.rs` | overlay API、两端 overlay、default style |
| 修改 Recursive Grid | `recursive_grid.rs` | layers/min-size/Backspace/keep tests、网页预览 |
| 修改 UI Hint 标签逻辑 | `modes/hint.rs`, `domain/hints/` | 两端扫描终态/Partial、retry tests |
| 修改 Windows 扫描 | `platform/windows/accessibility.rs` | COM thread、popup/Z-order、timeout、实机 UIA |
| 修改 macOS 扫描 | `platform/macos/ui_scan.rs` + AX/Vision | 权限、单 worker、Objective-C bridge、macOS 14 |
| 修改覆盖层性能 | 两端 `overlay.rs` | `OverlayScene` equality、dismiss 内存、DPI/Retina |
| 修改状态栏/开机启动 | 两端 `status_item.rs`/`autostart.rs` | `BackendEvent`、打包应用身份 |
| 修改配置路径 | `app/paths.rs`, `config::discover` | packaged app 与 portable tests、README |
| 修改打包 | `packaging/<os>/`、`build.rs` | CI + release matrix、图标/签名、仅发布平台 ZIP |
| 修改网页模拟器 | `docs/.vitepress/components/ConfigStudio.tsx` | style controls、Node tests、typecheck/build |

## 跨层改动顺序

新增能力时推荐：

1. 在 `api` 建立平台无关类型或 verb。
2. 写 parse/canonical/validation 测试。
3. 让 Mode 通过 Command 表达需求，或 Engine 执行 host-level 动作。
4. 扩展 `Backend` 时同时实现 Windows、macOS 和 unsupported。
5. 更新 default TOML、用户文档、网页模拟器需要显示的子集。
6. 加集成测试锁定默认体验。

不要先在一个 Backend 做专用入口再让核心知道其 concrete type。

## 高风险不变量

### 输入

- 每个吞掉的 key-down 必须吞掉对应 key-up。
- held binding 的 release 发给 press 时的 owner，不在 release 时重新解析。
- `press/release/toggle` 必须在退出、暂停、输入失败和 shutdown 时尽力释放。
- semantic `Clicked` 只在成功的 KeySteer click/double-click 后发一次。

### Mode

- Idle 永不捕获普通键盘。
- targeting `keep` 不调用 Activated/Restarted，不丢路径。
- Finish 幂等；`after_click` 禁止 click action 递归。
- modal plugin Pop 后下层收到 Resumed 并重画原状态。

### 扫描

- Engine thread 不等待 AX/UIA/Vision。
- 旧 scan id、旧 pid/window context 的结果必须丢弃。
- Partial 可直接使用；TimedOut 不等于清空已出现标签。
- worker queue/target count/native transaction 必须有边界。

### 绘制

- overlay 必须 topmost、click-through、no-activate。
- 静态 Grid 与 cursor/indicator 尽量分层或缓存。
- 大 buffer/image/font cache 在 dismiss 时释放。
- 多屏坐标使用 desktop absolute；原生 window 内再转换 local。

### 配置与发布

- `deny_unknown_fields` 保持 typo 可见。
- 一个 TOML 可跨平台解析；平台字段不能在另一目标意外生效。
- macOS `.app` 不写 bundle；portable 不写用户全局目录。
- 正式 artifacts 必须来自 packaging script，保持图标、应用身份和签名链。

## 验证强度

- 纯文档：链接、路径、`git diff --check`。
- API/config/mode：相关单测 + `cargo test` + clippy/fmt。
- Windows/macOS 后端：目标 `cargo check/clippy`，并在对应 OS 实机验证 Hook、权限、覆盖层。
- 打包：运行对应 script，检查 ZIP 内容、图标、无控制台、签名/Info.plist。
- 网页：`docs:test`、`docs:check`、`docs:build`；视觉布局变化再做浏览器实测。

## 文档同步规则

- 新目录/模块：更新 `01-project-map.md`。
- API/Engine 数据流：更新 `02-runtime-and-api.md`。
- 配置语义/默认生命周期：更新 `03` 和 `04`，必要时用户配置参考也更新。
- 扫描算法/线程：更新 `05`。
- 原生模块或最低系统：更新 `06`/`08`。
- 缓存、buffer、frame path：更新 `07`。
- 本索引只写稳定入口，不堆实现细节。
