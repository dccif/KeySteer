# 扩展 KeySteer

先判断你要解决的问题，再选择扩展点。大多数需求不需要改 Rust：能用 TOML 配置完成，就不要把配置问题做成代码问题。

## 先选合适的方式

| 目标 | 推荐方式 | 主要入口 |
| --- | --- | --- |
| 改快捷键、速度、主题或某个应用的行为 | 配置 | `keysteer.<名称>.toml` |
| 把几个动作串起来 | 动作数组 | `[<mode>.bindings]` |
| 做一个新的键盘工作流或定位方式 | 新增 Mode | `src/modes/` + `api::Mode` |
| 为宿主增加一个可调用的功能 | 内置插件 | `src/plugins/` + `Manifest` |
| 增加跨平台能力 | 公共 API + Backend | `src/api/`、`src/platform/` |
| 增加一种配置动作 | Binding + Command | `src/api/binding.rs`、`src/api/command.rs` |

## 最短开发路径

```text
配置需求
  └─ TOML → --check → 文档/测试

Mode 或插件需求
  └─ ModeEvent → 状态 → Command → Engine → Backend

平台能力需求
  └─ api trait → Windows/macOS/unsupported 实现 → 实机测试
```

推荐先读[架构](/development/architecture)，再按改动类型阅读[开发流程与测试](/development/workflow)。

## 新增一个 Mode

Mode 是平台无关的状态机：它接收事件和只读运行现场，返回要宿主执行的命令。

1. 在 `src/api/` 确认已有的 `ModeEvent` 和 `Command` 是否够用。
2. 在 `src/modes/` 新建状态机，保存会话状态，不直接调用 Win32、AppKit 或 Backend。
3. 实现 `api::Mode` 的 `id`、`display_name` 和 `handle`。
4. 在 `src/modes/mod.rs` 的 `built_in()` 注册，并让配置决定是否启用。
5. 为按键、完成、取消、配置重载和失败清理补测试。
6. 同步更新默认 TOML、用户文档、侧边栏和 AI 项目手册。

一个 Mode 的核心形状如下：

```rust
impl Mode for MyMode {
    fn id(&self) -> ModeId {
        ModeId::new("my_mode").unwrap()
    }

    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> Vec<Command> {
        // 更新状态；通过 Command 请求宿主能力
        Vec::new()
    }
}
```

不要在 Mode 里直接操作鼠标、线程、窗口或平台权限。这样才能复用同一套状态机测试，也不会让 Windows 和 macOS 逻辑互相污染。

## 新增一个内置插件

当前插件是**编译进程序并在启动时注册的 bundled plugin**，不是从目录动态加载的 DLL、dylib 或脚本。完整接口见[插件开发](/development/plugin-development)。

基本步骤：

1. 在 `src/plugins/builtin/` 实现 `Mode`。
2. 用 `Manifest` 声明稳定 id、名称、描述、API 版本、动词和建议绑定。
3. 在 `src/plugins/mod.rs` 的 `bundled()` 返回它。
4. 在 `src/app/bootstrap.rs` 注册；注册时会校验 Manifest。
5. 在 `[plugin_modes."plugin:<id>"]` 提供继承、绑定和插件专属 `settings`；只使用 `PluginModeConfig` 已声明的字段，不要假设插件也有内置定位 Mode 的 `lifecycle` 段。
6. 给动词参数、默认绑定不覆盖用户配置、配置重载和模式退出写测试。

插件与内置 Mode 使用同一套公共 API：

```text
ModeEvent + HostContext → Plugin 状态机 → Vec<Command>
```

需要新能力时，优先扩充 `src/api/` 的公共类型，而不是让插件绕过 Engine 访问平台模块。

## 增加一个配置动作

例如新增 `notify <text>`，需要同时改动四层：

1. **解析**：在 `src/api/binding.rs` 添加语法、参数数量和错误信息。
2. **公共命令**：在 `src/api/command.rs` 添加平台无关的 `Command` 变体。
3. **执行**：在 `src/app/runtime/mod.rs` 的运行时命令执行路径处理该命令；涉及原生能力时调用 `Backend`。
4. **验证和文档**：增加单元/集成测试，更新 `keysteer.default.toml`、[模式与动作参考](/reference/modes-and-actions)、模拟器动作列表和 AI 手册。

绑定解析有意在加载阶段拒绝拼写错误。不要把未知输入静默当成“什么都不做”，否则用户只能在运行时猜原因。

## 增加平台能力

如果能力需要系统 API，请按以下顺序：

1. 在 `src/api/backend.rs` 定义 Windows/macOS 都能理解的接口。
2. 在 `src/platform/windows/` 和 `src/platform/macos/` 分别实现。
3. 同步检查 `unsupported` 实现，确保未支持平台仍能编译并返回清晰错误。
4. 让 Engine 负责调度、超时和失败清理；不要让 Mode 直接持有平台对象。
5. 在目标系统实测权限、Hook、输入注入、覆盖层和多显示器行为。

耗时的 UIA、AX 或 Vision 扫描应放到 worker，通过 `BackendEvent::UiScanned` 返回。结果必须带 owner、scan id 或 generation，避免旧会话的结果污染新会话。

## 提交前检查

```bash
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo run -- --check -c ./keysteer.default.toml
```

如果改了文档或配置模型，再运行：

```bash
pnpm docs:check
pnpm docs:build
```

最后检查三件事：配置示例能否复制、默认行为是否与源码一致、失败时是否释放按键/鼠标/覆盖层/timer。
