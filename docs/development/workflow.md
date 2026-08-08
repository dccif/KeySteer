# 开发流程与测试

## 环境

- Rust 版本以 `rust-toolchain.toml` 为准。
- 文档站需要 Node 24 及以上；pnpm 版本以根目录 `package.json` 的 `packageManager` 字段为准。
- macOS 原生功能需要 macOS 14；Windows 原生功能应在 Windows 实机验证。

## 编译和运行

在 `keysteer/` 目录执行：

```bash
cargo check
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
```

直接运行开发版本：

```bash
cargo run
cargo run -- --help
cargo run -- --doctor
cargo run -- --check -c ./keysteer.default.toml
cargo run -- --dump-config
```

`--doctor` 用于检查后端、键盘、显示器、前台应用和权限；`--check` 只校验配置；`--dump-config` 输出当前生效配置。`-c`/`--config` 使用给出的**确切路径**；因此在仓库根校验示例文件时要写 `./keysteer.default.toml`（Windows PowerShell 可写 `./` 或 `.\\`），而不是裸文件名。裸文件名会按应用的数据目录解析。

## 平台打包

不要直接把 `target/release` 当作最终用户包：

```bash
# Windows
powershell -ExecutionPolicy Bypass -File packaging/windows/package.ps1

# macOS
./packaging/macos/package.sh
```

打包脚本会处理 GUI subsystem、图标、`.app` 目录、签名和发布压缩包。macOS 用户应运行打包得到的 `KeySteer.app`，否则辅助功能和屏幕录制权限可能绑定到错误的宿主。

## 文档站

```bash
pnpm install
pnpm docs:dev
pnpm docs:test
pnpm docs:check
pnpm docs:build
```

- `docs:dev`：同步默认 TOML 和图标后启动 VitePress。
- `docs:test`：运行模拟器和配置模型的 Node 测试。
- `docs:check`：运行 Vue/TypeScript 类型检查。
- `docs:build`：生成静态文档站。

配置模拟器只负责浏览器端预览、编辑和轻量继承模拟，不是 Rust 校验器。最终合法性仍以程序的 `--check` 和 Rust 测试为准。

## 先判断改哪里

| 需求 | 优先改动 |
| --- | --- |
| 只是改按键、速度、主题或应用例外 | TOML，不改 Rust |
| 一个按键依次做几件事 | Binding 动作数组 |
| 新增一套会话状态和交互流程 | Mode |
| 给配置增加可复用的带参能力 | bundled plugin + Manifest verb |
| 调用操作系统能力 | `api::Backend` 与平台实现 |
| 新增配置动作语法 | `Binding`、`Command`、执行器、测试和文档一起改 |

详细步骤见[扩展 KeySteer](/development/extension-guide)。

## 改动建议

### 改绑定或动作

1. 在 `src/api/binding.rs` 修改解析、规范化和序列化。
2. 为合法输入、错误输入和数组顺序增加测试。
3. 更新 `keysteer.default.toml`、[模式与动作参考](/reference/modes-and-actions)和配置模拟器需要展示的动作分类。
4. 检查 `src/app/runtime/mod.rs` 中的命令执行分支、配置校验和集成测试。

### 改 Mode

1. 先确认 ModeEvent、Command 和生命周期语义。
2. 在 Mode 内保存状态，使用 `Command` 请求宿主能力。
3. 验证 `Activated`、`Deactivated`、`Restarted`、`FinishRequested` 和 `ConfigReloaded`。
4. 检查 overlay、输入释放和模式切换后的 owner。

### 改平台后端

1. 先在 `src/api/backend.rs` 定义平台无关契约。
2. 分别实现 Windows、macOS 和 `unsupported`。
3. 将耗时扫描放到 worker，不阻塞 Engine。
4. 在目标系统实机验证 Hook、权限、覆盖层、输入注入和多显示器。

## 测试分层

| 层级 | 覆盖内容 |
| --- | --- |
| Rust 单元测试 | Binding 解析、配置校验、Mode 状态、几何算法和 runtime 路由 |
| `tests/` 集成测试 | 发布默认配置、CLI 行为和跨平台不变量 |
| 文档 Node 测试 | 绑定继承、配置副本、模拟器状态 |
| 目标系统实测 | Hook、权限、UIA/AX/Vision、覆盖层、托盘和打包 |

只改 Markdown 时，至少运行：

```bash
git diff --check
pnpm docs:check
pnpm docs:build
```

## 文档维护规则

- 默认行为以 `Config::default()`、配置校验和 `keysteer.default.toml` 的一致性测试为准；默认 TOML 是人可读的发行副本，不是运行所必需的文件。
- 新模块更新[项目地图](/ai/01-project-map)和[架构](/development/architecture)。
- 改变运行时数据流更新架构文档；改变配置语法同时更新用户参考和 AI 手册。
- 面向用户的页面先给结论和可复制示例，再补充原理；不要让用户必须阅读源码才能完成常见操作。
