# 配置、按键和持久化

## 配置模型

根类型是 `src/config/mod.rs::Config`。所有主要 section 都有默认值，因此空配置或没有
配置合法。根类型和大多数结构使用 `deny_unknown_fields`，拼错字段应在加载时明确失败，
不能静默忽略。

主要 section：

- `general`、`debug`、`platform`
- `theme.dark` / `theme.light`
- `hotkeys`
- `normal`、`grid`、`recursive_grid`、`ui_hint`
- `pointer`、`scroll`、`mode_indicator`
- `plugin_modes`、根级和 Mode 级 `app_configs`

完整字段和注释以 `keysteer.default.toml` 为准；它必须与 `Config::default()` 的发布默认
行为保持一致，并由集成测试逐字段锁定。不要在 AI 文档复制一份会过期的完整配置。

`pointer.smooth_acceleration` 默认启用 Normal 的 smootherstep S 曲线；关闭时使用线性
加速。两种模式都按真实时间积分并在松开最后一个方向键时立即停止。

`normal.long_press_toggle_ms` 默认 500；0 禁用。它为直接绑定的 click/double-click 和
单独按住的无参数 `toggle` 激活键建立 deadline。前者达到阈值后把对应鼠标按钮交给现有
latched Toggle 状态机；后者把激活键自身锁定为 `Down`。普通点击仍在按下沿立即完成，
配置允许范围为 `0..=60000`。

`mode_indicator.cursor.{left,middle,right}_pressed_color` 控制合成鼠标按钮被 `press` 或
`toggle` 锁定时，以及普通 click 的物理触发键仍按住时的光标圆形颜色；每个 Mode 的
cursor override 可单独覆盖这些值。普通 click 的提示由按键释放事件清除，不计时。

## 加载与发现

`Config::discover()` 在应用数据目录中优先查找按名称排序的非 default
`keysteer.<任意名称>.toml`；`keysteer.default.toml`（大小写不敏感）只在没有用户 profile
时作为文件 fallback。显式 `--config` 不参与该排序，仍要求这个文件名格式，但可以接受完整路径：

- 纯文件名在应用数据目录中解析，适合 portable 包。
- 带目录的相对路径从当前工作目录解析。
- 绝对路径保持不变。

显式加载的路径也会原样交给 `ConfigStore`，因此运行时配置变更会写回同一个文件。

启动时没有使用 `--config` 时，runtime 会保留应用数据目录，并在每次 Reload 时重新执行
`Config::discover_in()`；因此新增、删除、重命名或排序更靠前的配置都能切换当前来源。使用
`--config` 时来源固定，Reload 只重读该路径。重新发现失败或文件无效时必须保留最后一份
有效配置；目录中没有配置时则恢复内置默认值，并继续把 `keysteer.user.toml` 作为写入路径。

数据目录由 `src/app/paths.rs` 决定：

- 打包的 macOS `.app`：`~/Library/Application Support/KeySteer/`。
- Windows portable、裸 macOS 二进制和其他裸二进制：可执行文件所在目录。
- 配置和 `keysteer.log` 使用同一策略；不依赖 process current directory。

自动发现到的文件无效时，启动会记录错误并使用内置默认值；显式 `--config` 无效则返回
错误。CLI `--check`、`--dump-config`、`--doctor` 用于诊断。

## 按键标准化

`src/api/input.rs` 负责 `Key` 和 `KeyChord`：

- 名称统一小写 canonical 形式。
- `primary` 在 macOS 解析为 Command，在 Windows/Linux 解析为 Ctrl。
- 泛型 modifier 可匹配左右两侧，`left_`/`right_` 只匹配指定侧。
- `[key_aliases]` 先应用，再由 `[key_aliases.windows|macos|linux]` 覆盖当前平台。
- `"v b" = "fast"` 是多个单键共享动作，加载时展开成 `v` 和 `b` 两条普通绑定；它不是
  按键序列。Chord 内部继续使用 `+`。

平台风险（macOS Option+字母、系统保留键、终端快捷键、Apple 不存在的 F21-F24）是
warning，不是解析错误。

## Binding 语法

`src/api/binding.rs::Binding` 同时是配置动作、内部动作和插件动作。解析顺序大致为：

1. `none`/`__disabled__`。
2. 显式形式：`call`、`send`、`exec`、`move_mouse`、`set_config`、`press`、`release`、
   `toggle`、`wait`。
3. 已知 verb，如 `move_left`、`left_click`、`fast`、`finish`。
4. 带参数的插件 verb。
5. `+` chord 或已知裸键，解析为 `Send`。
6. 内置 Mode id 或带 namespace 的 plugin Mode id。

TOML 右值可以是字符串，也可以是字符串数组；数组解析为有序 `Binding::Sequence`。
canonical 输出必须能重新 parse。`press/release/toggle` 是合成输入状态管理；
`precision/slow/fast` 才是移动速度修饰符。

无参数 `toggle` 对所有键使用相同规则。伙伴键在激活键之前或之后按下都可被锁定；伙伴
绑定为 click/double-click 时锁定其鼠标按钮，否则锁定解析出的键盘目标。单独短按激活键
释放全部现有 latch，单独长按达到 `long_press_toggle_ms` 才锁定激活键自身。Windows 与
macOS 原生后端把重复的同按钮 `Press` 视为幂等操作，不得注入第二个 mouse-down。

## 继承和优先级

Engine 为每个 Mode 编译有效 keymap：

1. 当前 Mode 本地绑定优先。
2. 按 `inherits` 给出的顺序查找父 Mode。
3. `none` 显式屏蔽继承项。
4. 当前应用匹配的 `app_configs` 覆盖合并结果。
5. 插件建议的默认 chord 只填补 `normal` 中仍为空的位置，不覆盖用户配置。

`temporary_mode`/`temporary_mode_keys` 允许 targeting mode 暂时使用另一个 Mode（默认为
Normal）的绑定，而不销毁当前选择路径。

## targeting 生命周期配置

每个 targeting mode 有：

```toml
[<mode>.lifecycle]
after_finish = "..."
after_click = "..."
```

- `after_finish`：`keep`、`restart`、`return`、Mode/plugin Mode、四种 click。
- `after_click`：`keep`、`finish`、`restart`、`return`、Mode/plugin Mode；禁止 click 防递归。
- `keep` 不调用 activate，不重建状态。
- `restart` 给当前实例发送 `Restarted`。
- `return` 返回该 session 进入时记录的 Mode。

当前发布默认：UI Hint `normal/normal`，Grid `normal/finish`，Recursive Grid `keep/keep`。

## 写入和重新加载

`src/config/store.rs::ConfigStore` 使用 `toml_edit` 保留注释：

- 在 clone 文档上应用 dotted-path 修改。
- 重新 parse + validate 成功后才替换当前文档。
- 使用同目录临时文件、flush/sync 和原子 replace/rename 写入。
- 无效修改不会破坏最后一个有效配置。

Engine 的 `ReloadConfig` 重新读取、校验、更新 palette/keymap，并向 Mode 广播
`ConfigReloaded`。新增可缓存配置时，Mode 必须在这个事件中刷新自己的副本。
