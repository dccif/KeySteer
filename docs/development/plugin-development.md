# 插件开发

KeySteer 暂时没有一套"专属插件 API"：**一个插件就是一个 `Mode`，外加一份说明自己的 `Manifest`**。插件与内置模式共享完全相同的词汇——同样的 `Command`（你能让宿主做什么）、同样的 `ModeEvent`（宿主会告诉你什么）、同样的 `HostContext`（只读的运行现场）。

这意味着内置的 `Grid` 或 `UI Hint` 能做到的任何事——全屏覆盖层、自定义网格、指针移动、按键注入插件都能做到。

## 核心概念

### 插件就是 Mode

插件需要实现 `api::Mode` trait：

```rust
pub trait Mode: Send {
    fn id(&self) -> ModeId;            // 稳定标识，用于配置与唤醒
    fn display_name(&self) -> String { /* 默认: id 中的下划线替换为空格 */ }
    fn handle(&mut self, event: &ModeEvent, ctx: &HostContext<'_>) -> CommandBatch;
    fn captures_keyboard(&self) -> bool { true }              // 是否独占键盘
    fn indicator_color(&self, _palette: &Palette) -> Option<Color> { None }
}
```

关键在 `handle`：每次收到事件就返回一批 `Command`。模式是纯状态机，不直接碰任何平台 API——这正是插件与内置模式可以互换的原因。

### Manifest：告诉宿主你是谁

```rust
pub struct Manifest {
    pub id: String,            // reverse-DNS 风格唯一 id，如 "com.example.zoom"
    pub name: String,          // 人类可读名称
    pub description: String,   // 一句话说明
    pub api_version: u32,      // 必须等于 API_VERSION（当前 8）
    pub default_chords: Vec<KeyChord>,     // 直接激活插件模式的遗留组合键
    pub verbs: Vec<String>,                // 插件导出的带参动词
    pub default_bindings: Vec<(KeyChord, Binding)>, // 建议绑定（见下）
}
```

Manifest 提供链式构造方法：`Manifest::new(id, name).with_description(...).with_verb(...).with_chord(...).with_default_binding(chord, binding)`。

**校验规则**（`Manifest::validate`，注册时宿主会校验）：

- `api_version` 必须等于 `API_VERSION`（当前 `8`），否则整个插件被拒绝

### API v8 overlay migration

`OverlayLabel.text` is now `OverlayText` and `style` is `SharedLabelStyle`.
Their serialized shapes remain a TOML/JSON string and a style object. Existing
`OverlayLabel::new(text, rect, LabelStyle)` calls still compile; struct literals
must use `.into()`, and direct style mutation becomes
`label.style_mut().font_size = ...`. Short text stays inline and cloned labels
share style through copy-on-write semantics.
- `id` 只能包含 ASCII 字母、数字以及 `.`、`_`、`-`，不能为空
- `verbs` 只能包含小写 ASCII 字母、数字和 `_`，不能为空

### 动词（verb）：让用户在配置里调用你的插件

插件可以导出**带参动词**。用户写 `screen next`、`screen 2`、`call screen` 这样的绑定，宿主会解析为 `ModeEvent::Invoked { verb, args }` 交给插件。

绑定右值的解析顺序（`Binding` 词汇与配置文件、内部动作、插件动作共用）：

1. `none` / `__disabled__`：屏蔽
2. 显式形式：`call`、`send`、`exec`、`move_mouse`、`set_config`、`press`、`release`、`toggle`、`wait`
3. 已知内置动词：`move_left`、`left_click`、`fast`、`finish` 等
4. **带参数的插件动词**
5. 组合键或已知裸键：发送给当前应用
6. 内置 Mode id 或带命名空间的插件 Mode id

所以插件动词只要注册进 `Manifest.verbs`，用户在配置里直接用 `screen next` 这种写法即可，无需特殊前缀。

### 你能让宿主做什么（Command 精选）

| Command | 作用 |
| --- | --- |
| `MovePointer { dx, dy }` / `WarpPointer { x, y }` | 移动 / 跳转鼠标 |
| `MouseButton { button, action }` | 按下、松开或点击鼠标键 |
| `Scroll { dx, dy }` | 滚动 |
| `Command::show_overlay(OverlayScene)` / `HideOverlay` | 全屏覆盖层（网格、标签都靠它） |
| `SetFrameClock(true)` | 用系统帧时钟驱动连续运动（不是普通定时器） |
| `SendKey` / `SendChord` | 向聚焦应用注入按键 |
| `Command::scan_ui(UiScanRequest)` | 扫描可访问性树，结果以 `ModeEvent::UiScanned` 返回 |
| `SwitchMode(ModeId)` / `PushMode` / `PopMode` | 切换模式 / 模态压栈 / 弹出 |
| `RetargetScreen { index, preserve }` | 把会话迁到指定显示器 |
| `SetTimer` / `CancelTimer` | 自定义延时任务 |
| `SetConfigValue` / `ReloadConfig` | 改配置 / 重读配置 |
| `Exec { program, args }` | 后台运行外部命令 |
| `Quit` | 退出程序 |

内置模式也只被限制在 `Command` 这一层，所以插件能表达的一切与内置模式完全相同。

### 宿主会告诉你什么（ModeEvent 精选）

| ModeEvent | 时机 |
| --- | --- |
| `Activated { previous }` / `Deactivated` | 激活 / 销毁，在这里绘制第一层覆盖层 |
| `Pushed { previous }` / `Suspended` / `Resumed` | 模态压栈 / 被遮挡 / 恢复重绘 |
| `Restarted` / `FinishRequested { cause }` | 会话重置 / 要求进入完成态 |
| `Clicked { button, action }` | KeySteer 执行的一次语义点击成功 |
| `Key { key, state, repeat }` | 原始按键（网格标签、搜索文本用它） |
| `Binding { binding, state, key }` | 配置绑定被触发（宿主已查表，插件不用自己解析按键） |
| `Invoked { verb, args }` | **插件动词被调用** |
| `Frame { elapsed }` | 每帧显示刷新（驱动连续运动） |
| `FocusChanged` / `ScreensChanged` / `ScreenRetargeted` | 应用 / 显示器拓扑变化 |
| `UiScanned(UiScanResult)` | 一次 UI 扫描的结果 |
| `Timer { id, elapsed }` | 自定义定时器到点 |
| `ConfigReloaded` | 配置重载，需要刷新缓存的字段 |

### HostContext：只读的运行现场

每次 `handle` 都会拿到一份 `HostContext`，包含当前屏幕列表、光标位置、聚焦应用、主题调色板，以及类型擦除的宿主设置（`config`）。内置模式可以直接 `downcast_ref::<Config>()` 读取自己的配置；bundled plugin 同样可以通过 `downcast_ref` 读取 `[plugin_modes."plugin:<id>".settings]`。

## 配置集成：`[plugin_modes]`

插件模式用 `plugin:<id>` 作为模式名，在配置里与内置模式享受完全相同的待遇：

```toml
[plugin_modes."plugin:screen-selector".settings]
preserve = true

[plugin_modes."plugin:screen-selector".bindings]
"1" = "left_click"
esc = "escape"

[plugin_modes."plugin:screen-selector"]
inherits = ["hotkeys", "normal"]
temporary_mode = "normal"
temporary_mode_keys = ["primary"]
```

- `settings`：插件自定义设置，代码里用 `config.plugin_setting_bool("<mode_id>", "<key>")`（或等价的字符串/整数读取）获取
- `bindings`：插件模式自己的绑定表，与 `[grid.bindings]` 一样参与继承与覆盖
- `inherits` / `temporary_mode` / `temporary_mode_keys` / `app_configs`：与内置模式语义相同

`PluginModeConfig` 没有 `[plugin_modes."plugin:<id>".lifecycle]` 字段。插件若需要完成或点击后的行为，应在自己的 `ModeEvent::FinishRequested`、`Clicked` 或 `Binding` 处理中维护状态并返回相应 `Command`。

## 注册插件

插件在程序内部编译并注册（当前没有外部动态加载）：

```rust
// src/plugins/mod.rs —— 内置插件统一在这里实例化。
// 别名解析失败会作为启动错误返回。
pub fn bundled(config: &Config) -> Result<Vec<Box<dyn Plugin>>, String> {
    Ok(vec![Box::new(ScreenSelector::with_key_aliases(
        config.resolved_key_aliases(),
    )?)])
}

// src/app/bootstrap.rs —— 启动时取得并注册每个 bundled plugin。
for plugin in plugins::bundled(&config)? {
    engine.register_plugin_dyn(plugin)?;
}
```

`Engine::register_plugin_dyn(Box<dyn Plugin>)` 会校验 Manifest、注册动词和 Mode，并合并默认绑定：**插件的 `default_chords` 与 `default_bindings` 都只填补 `normal` 中仍为空的位置，绝不覆盖用户已经配置的键位**。注册失败不是可静默忽略的配置问题：它会中止启动，使版本或 Manifest 错误能被立刻发现。

## 内置示例：Screen Selector

项目自带的 `src/plugins/builtin/screen_selector.rs` 是只用公共 API 实现的完整插件，值得照抄：

- 模式 id：`plugin:screen-selector`（配置段 `[plugin_modes."plugin:screen-selector"]`）
- 动词：`screen`（用法：`screen next`、`screen previous`、`screen 2`、`call screen` 打开编号选择器）
- Manifest 构造：

```rust
let manifest = Manifest::new("com.keysteer.screen-selector", "Screen Selector")
    .with_description("Switch displays directly or choose one from a numbered overlay")
    .with_verb(VERB);
// primary+s 建议绑定为 "screen next"，不覆盖用户已有配置
let manifest = match KeyChord::parse_with_aliases("primary+s", aliases) {
    Ok(chord) => manifest.with_default_binding(chord, Binding::Invoke {
        verb: VERB.into(),
        args: vec!["next".into()],
    }),
    Err(_) => manifest,
};
```

- 读取插件设置：

```rust
fn preserve(ctx: &HostContext<'_>) -> bool {
    ctx.config
        .downcast_ref::<Config>()
        .and_then(|config| config.plugin_setting_bool(MODE_ID, "preserve"))
        .unwrap_or(true)
}
```

它用自己的 `OverlayScene` 画出显示器编号、处理按键筛选，并最终发出 `WarpPointer` 与 `RetargetScreen`。默认 `preserve = true` 会让 `Grid` / `Recursive Grid` 的当前选择路径在切屏后保留。

## 规范与建议

- **只依赖公共 API**：插件不要向下依赖 `Engine` 或平台模块。需要新能力时，优先扩充公共 API，而不是给插件开后门——这样内置模式与第三方插件始终保持互换性
- **`api_version` 是硬约束**：宿主与插件版本不匹配会直接拒绝加载
- **共享词汇**：优先复用内置动词（`move_left`、`left_click`、`finish` 等），不要发明重复的词汇；`ModeEvent::Binding` 已经替插件查好了绑定表
- **默认绑定保持克制**：`default_chords` 和 `default_bindings` 都只是“建议”，用户配置永远优先
- **配置结构是公开边界的一部分**：新增插件配置字段时先扩展 `PluginModeConfig`；不要在 TOML 中记录运行时自己处理但配置类型并不存在的 section
