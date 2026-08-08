# 架构

KeySteer 是一个单 crate Rust 桌面程序。你可以把它理解为一条清晰的流水线：配置决定“按什么键”，Mode 决定“当前怎么处理”，Engine 负责调度，Backend 负责调用 Windows/macOS 原生能力。这样功能可以复用、测试和扩展。

这页给准备读源码或贡献代码的人；只想改快捷键，请先看[配置文件](/reference/configuration)。

## 一句话架构

```text
TOML
  -> Config
  -> Engine
  -> 编译后的 Keymap
  -> ModeEvent
  -> Mode / Plugin
  -> Command
  -> Backend
  -> Windows / macOS
```

平台事件沿相反方向返回：

```text
原生 Hook / UIA / AX / Vision / 状态栏
  -> BackendEvent
  -> Engine
  -> 当前 Mode
```

## 源码分层

| 目录 | 负责什么 | 从哪里开始读 |
| --- | --- | --- |
| `src/api/` | 跨平台公共协议：按键、绑定、命令、事件、覆盖层、插件和后端 trait | `api/mod.rs`、`api/binding.rs`、`api/command.rs` |
| `src/config/` | TOML 反序列化、默认值、校验、主题、继承和原子写入 | `config/mod.rs`、`config/store.rs` |
| `src/app/` | CLI、路径、日志、启动组装和运行时编排 | `app/bootstrap.rs`、`app/runtime/mod.rs` |
| `src/modes/` | `idle`、`normal`、`grid`、`recursive_grid`、`ui_hint` 状态机 | 对应的 `.rs` 文件 |
| `src/domain/hints/` | UI 标签分配、匹配和网格算法 | `labels.rs`、`matcher.rs` |
| `src/plugins/` | 内置插件；也是第三方插件的参考实现 | `builtin/screen_selector.rs` |
| `src/platform/windows/` | Win32 Hook、SendInput、UIA、覆盖层、帧时钟和托盘 | `mod.rs` |
| `src/platform/macos/` | CGEventTap、Core Graphics、AX、Vision、AppKit 和菜单栏 | `mod.rs` |

`src/lib.rs` 暴露公共 API；`src/main.rs` 只负责进入 CLI 和启动流程。平台后端由 `cfg(target_os)` 在编译期选择，不需要 feature 开关。

## 启动流程

`main.rs` 进入 `app::run_cli()` 后，程序依次：

1. 初始化日志和 panic hook。
2. 解析 `--config`、`--check`、`--doctor` 等参数。
3. 显式 `--config` 时加载该文件；否则在数据目录按文件名选择第一个 `keysteer.<名称>.toml` 用户配置（排除 `keysteer.default.toml`）。只有没有用户配置时才使用该默认文件；仍不存在时使用 `Config::default()`。
4. 创建目标平台的 `Backend` 和 `Engine`。
5. 注册内置 Mode 与 bundled plugin。
6. 启动事件循环，默认激活 `idle`。

## Engine 是边界和调度中心

`Engine` 不实现具体的网格或 UI 扫描算法，而是维护运行时状态：

- 当前 Mode、临时 Mode 和 modal stack。
- 每个 Mode 编译后的按键表及应用覆盖。
- 按键消费/延后决定/透传决定、held gesture、长按点击 Toggle 和合成输入 latch。
- 非阻塞动作序列、timer、UI scan owner 和 frame clock owner。
- 屏幕、光标、前台应用、主题和覆盖层 scene。

它把 Mode 返回的 `Command` 翻译成 Backend 调用，或再发回一个 ModeEvent。这样模式不需要知道窗口句柄、线程、权限或原生 API。

## Mode 与 Plugin 的统一契约

Mode 是一个平台无关状态机：收到 `ModeEvent` 和只读的 `HostContext`，返回 `Vec<Command>`。

```rust
impl Mode for MyMode {
    fn id(&self) -> ModeId { ModeId::new("my_mode").unwrap() }

    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> Vec<Command> {
        // 更新状态，返回宿主可以执行的 Command
        Vec::new()
    }
}
```

Mode 可以请求：移动鼠标、发送按键、显示覆盖层、扫描 UI、切换或压栈模式、设置 timer、执行命令和重载配置。它不能直接调用 `Backend`、Win32、AppKit、UIA 或 AX。

这也是插件边界：插件实现同一个 `Mode` trait，并通过 `Manifest` 声明 id、动词和建议绑定。当前插件是在程序内编译注册的，并不是运行时加载的动态库。

## 输入与动作流

键盘 Hook 必须在原生回调返回前知道按键是否透传：

```text
native callback
  -> BackendEvent::Input
  -> Engine::handle_key()
  -> Backend::dispose_key(Consume | Defer | Forward)
```

`Defer` 仅用于必须等到修饰键组合是否成立才能决定的原生路径（Windows 的 Alt 处理是典型例子）；后端随后会补发或消费该键。Engine 会记录每个物理键的 down 决定，使对应的 up 使用同一归属。持续绑定在释放时仍交给按下时的 owner，避免模式切换造成状态泄漏。

绑定动作有两条路径：

- `move_left`、`left_click`、`exec`、`send`、`set_config` 等宿主动作由 Engine 执行。
- `move`、`scroll`、`fast` 等依赖模式状态的动作通过 `ModeEvent::Binding` 交给 owner Mode。
- Grid 标签、UI Hint 标签和搜索文本等原始字符通过 `ModeEvent::Key` 交给 Mode。

动作数组是非阻塞的。`wait` 会把剩余动作交给 Engine 的 pending sequence，不会阻塞事件线程。

## 异步工作

UIA、AX、Vision 扫描在 worker 中执行，以 `BackendEvent::UiScanned` 分批返回；Engine 用 scan id 和 owner 丢弃过期结果。连续鼠标移动使用原生显示帧时钟，而不是固定 16ms timer：Windows 等待当前目标输出的 DXGI 垂直同步，macOS 使用 `CVDisplayLink`。每帧携带实际经过时间，Normal Mode 以该时间积分速度曲线；切换显示器会重绑帧源，因此不会把“假定 60 Hz”的距离带到新屏幕。

Windows 的帧等待是专用 worker 的可取消阻塞：它不查询刷新率、不在输入 hook 中等待，也不占用 Engine/UI 线程。停止帧时钟、切换输出和应用退出都会发出唤醒/停止信号并等待 worker 收尾；`INFINITE` 只表示该 worker 在没有下一帧或停止请求前无需轮询，并不让运行时对象永久存活。

新增异步工作时必须有：

- 明确的 owner 或 generation，避免旧结果污染新会话。
- 超时、取消或过期检查。
- 有界队列和结果数量限制。
- 不阻塞 Engine/UI 线程。

## 修改时守住的边界

- 新能力先放进 `src/api/` 的平台无关类型，再由 Mode/Engine 使用。
- 不要让 Mode 依赖某个平台的具体类型。
- 修改 `Backend` 时同时检查 Windows、macOS 和 `unsupported` 实现。
- 改动绑定语法时同步更新 `keysteer.default.toml`、配置文档和测试。
- 改动 Finish、点击或 held input 时，重点检查失败清理和 key-up 路由。配置重载失败、模式切换、禁用和退出必须清除长按候选、普通点击视觉状态、计时器、帧时钟、scan owner、覆盖层与所有 latched 输入。

下一步可阅读[扩展 KeySteer](/development/extension-guide)、[开发流程与测试](/development/workflow)或[插件开发](/development/plugin-development)。
