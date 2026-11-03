# 配置与持久化

## 查找入口

- `src/config/`：文档模型、默认值、别名、校验和保留注释的写回。
- `src/config/macros.rs`：`config_struct!` 生成字段和默认值，`mode_config!` 复用路由字段，`window_config!` 处理窗口公共结构与平铺解析。
- `src/config/compatibility.rs`、`compatibility.json`、`build/compatibility.rs`：配置入口统一规范化旧字段、绑定和表格式；原生与网页共用规则，Rust 在编译时生成静态规则。
- `src/app/configuration.rs`：配置编译边界；`src/app/mode_catalog.rs`：模式 Settings 与路由装配。
- `src/api/input.rs`、`src/api/binding.rs`：键与动作语法。
- `src/app/paths.rs`、`src/app/restart.rs`、`src/app/preset_store/mod.rs`：路径、重载进程、工作区存储。

## 新增模式的维护入口

1. 在 `api/input.rs` 的 `builtin_mode_ids!` 添加内置身份；枚举、解析、构造函数、字符串转换与 `ModeId::BUILT_IN` 自动同步。已有条目的顺序不要调整，它决定 `ModeId` 的排序。
2. 声明模式配置并接入 `ConfigFile` 的字段和默认值，再在 `config/macros.rs` 的 `builtin_mode_fields!` 添加配置节。公共路由与临时键遍历不再逐处加名单；专属字段仍需要相应的校验和别名处理。
3. 在 `app/mode_catalog.rs` 编译模式 Settings，并在 `built_in_factories` 的共享数组中添加一项 `ModeFactory`，一起声明启用条件、实例构造函数和路由编译函数。普通路由使用 `compile_route`，带标准应用覆盖的使用 `compile_app_route`；UIHint 的独立覆盖类型由 `compile_ui_hint_route` 处理。`built_in` 与 `built_in_specs` 自动共用该项，无需分别添加模式；只有后者编译路由。两种历史顺序由固定前后项与共享数组拼接，新增普通模式不需要修改顺序分支。窗口模式另接入 `WindowKind` 与 `window_common` 的穷尽匹配；窗口路由按 ID 查找配置，不依赖两个数组的位置一致。
4. 配置测试对照序列化的模式节、ID 清单、路由／可变键清单，并逐模式检查普通／应用覆盖绑定的别名和非法键；catalog 测试检查完整身份集合、注册顺序和每个启用开关。漏接任一入口应在测试中失败。模式特有生命周期和发布配置／用户文档仍按改动范围维护。

## Bisect 配置入口

`[bisect]` 独立配置二分上限、最小宽高、光标跟随、生命周期、继承、临时层与应用覆盖；`[bisect.bindings]` 默认 h/j/k/l 对应四个二分动作，支持普通绑定的别名、多键与禁用规则。稀疏表补齐默认方向和导航动作，`none` 可禁用。没有 `keys`、行列或 `layers`；`[bisect.ui]` 仅提供无字母区域适用的线宽、线色、高亮色。默认 Normal 入口 `primary+f`；从临时层沿用这个完整入口须在来源模式 bindings 显式配置，临时目标及继承不重新使用已消耗的激活键；网页配置器同步模型、键位及预览。

## 保留的语义

三种网格默认将 `u`／`i` 分别绑定到 `area_hint`／`area_text`。加载稀疏旧配置时，若按键已用于当前网格的标签（含递归层）或显式继承的 Normal／hotkeys 绑定（含应用覆盖），且该网格没有显式绑定该键，则不补入对应动作；显式本地绑定仍优先。网页配置解析保持相同规则。

配置宏只生成普通结构体和默认值，不增加运行时配置层。普通设置使用 `config_struct!`；`field: Type = expression` 指定默认值，省略表达式使用字段类型的 `Default`。字段注释、serde 属性和自定义解析器原样保留；结构体可追加 `#[derive(Eq)]` 等属性。已有 `derive(Default)` 且无需自定义默认值的结构体不必改用宏。根配置的默认值还需要编译别名，继续使用普通 Rust 实现。

搜索编辑键按动作保存的单个快捷键原位处理别名，不拆成临时字符串数组再拼接；含 Unicode 空白的分组继续沿原有分割、别名解析与单空格合并规则处理，空分组仍禁用动作。

内置模式及插件使用 `mode_config!`，在需要的位置放置 `..routing {}`，原位生成 `inherits`、`temporary_mode`、`temporary_mode_keys`、`temporary_mode_passthrough_keys`。默认继承 `["hotkeys", "normal"]`、临时目标为 `Some("normal")`，两种按键列表为空；这与 Hint 和网格模式共用的默认值一致。组内按上述字段顺序写 `字段名: Rust 表达式,` 即可覆盖默认值，所有字段均可省略。例如新增同类模式：

```rust
mode_config! {
    pub struct Example {
        enabled: bool = true,
        ..routing {}
        speed: f64 = 300.0,
        bindings: Bindings,
        app_configs: Vec<AppOverride>,
    }
}
```

无继承或临时目标的模式显式写 `inherits: Vec::new(), temporary_mode: None,`。仅影响导出的 `omit_empty: all,` 或 `omit_empty: temporary,` 放在组内最前面，分别省略四个空字段或仅三个空临时字段，缺省保留空字段。声明处不使用内部 `@` 分支。宏只拆分一次路由组前后的字段，不递归扫描任意字段组；特殊默认表达式、独立字段和解析函数仍由模式定义。 `mode_config!` 同时生成 `routing()` 借用视图，窗口从公共结构获得同一视图；catalog 和继承校验直接借用四个字段，不复制配置或改变平铺格式。`builtin_mode_fields!` 是内置配置节的公共遍历清单，供路由、绑定别名规范化与校验、临时触发键校验及可变键列表借用共用；窗口项指向 `common`。`ConfigFile::mode_routing` 从该清单生成借用并追加插件，保留原有临时目标报错顺序；继承图与遍历集合只保存借用。遍历中的少量特殊前后项直接列出，避免为保留报错顺序排序；其余模式从公共清单自动遍历。绑定诊断保留 Quick Switch、多选表和窗口应用覆盖的既有顺序。

窗口使用 `window_config!(pub struct Name { common: expression, ... })`，只声明共享配置的默认值及额外字段；`window_config!(common pub struct WindowModeConfig;)` 生成共享结构。窗口公共字段定义只有一份，路由字段继续复用 `mode_config!`。窗口保留 `common` 与 `Deref`，TOML 仍为平铺字段，其稀疏文档和生命周期使用各模式自己的默认值（包括 Restore 的完成目标）。共享结构直接反序列化时继续允许未知字段，各窗口模式的平铺文档则严格拒绝未知字段。普通配置也严格拒绝未知字段；显式绑定表仍替换默认表，不做通用递归合并。

所有可配置模式（含 Normal、Quick Switch、文字与窗口模式、插件）共用继承、临时目标、触发键与穿透键的编译和路由。内置 `temporary_mode_keys` 全部默认为空；用户配置省略该字段（包括整段省略）或设置 `[]` 均禁用临时层。发布 TOML 保留已有显式触发键，其行为不变；临时目标设为 `""`（或纯空白字符串）也禁用该功能；借用路由视图统一将其视作无目标，校验与编译共用此语义。只有非空触发键且有临时目标时启用。`Config::default()` 与 `Config::parse("")` 的临时层状态一致，无需加载后补偿清空。catalog 直接编译临时触发键为 KeyChord，对关闭的层移除目标与匹配数据；没有有效触发组合时一并清除目标、触发键与穿透键。首次建表时将新路由中的触发键移入模式槽位，后续建表复用原缓冲区；注册和配置替换不再单独转移临时键。建表根据关闭状态或新触发键更新槽位，缺失路由也会清除旧键；重新注册同一模式保留已安装的触发键。禁用层不生成反向继承的备用顺序；输入读取槽位中的 `None` 后直接走普通路由，不匹配触发器、不构造剥离按键，也不重新解析配置或判断字符串／列表是否为空。UIHint 与 Text Select 的重叠换层冲突由共用路由编译函数预先排除；通用建表不再识别这两个模式。重叠键在 EngineSettings 及 UIHint／Text Select 的模式 Settings 中只保存已解析的 KeyChord；校验保证它是有效修饰键，模式不保留原始字符串或可选解析缓存，焦点变化及键表重建不重新解析。启用层仍需在输入时匹配当前按键状态。网页稀疏文档合并同样不从默认配置补回触发键。

平台警告是独立于配置有效性的保守提示：默认文字选择保留 `Alt+C` 细分，在 macOS 仍报告对应的 Option 字符输入提示，不因此改键或拒绝配置。默认警告回归只允许这一条已知提示，其他平台保持无默认警告。

- 发布配置除显式启用临时层外与内置默认值一致；未知字段、非法组合和冲突应明确报错。
- 可选颜色校验只在报错时格式化完整字段路径；有效或缺省颜色不创建临时路径字符串，嵌套字段的错误上下文仍保留。
- 模式只接收编译后的 Settings，不读取 TOML；别名、继承、应用覆盖和临时模式共用解析规则。绑定左侧以空格分隔任选的单键或组合键，例如 `"mod+c enter"`；启动时展开、解析别名并拒绝重复，不在输入时解析。
- 按键规范化借用未变化的别名名称，保留无需展开的绑定表；混合表直接移动原有键和值，仅多键展开时复制动作。控制键补齐原位修改所属绑定表，配置解析与程序化配置编译共用同一规则，保留显式 `none`、冲突顺序及错误路径。各模式在自己的配置定义旁提供兼容控制键，完整默认表复用同一份控制键并追加普通快捷键；公共补齐函数只接收控制键表、错误标签和已验证别名，不检查模式名或动作类型。一处模式到控制键工厂的映射供配置解析和运行时编译共用；不为补齐窗口控制键构造完整模式或插件配置。规范化后的默认控制键已按原始键名齐全时直接返回，不再解析这些键或创建占用集合。
- 配置发现按入口文件名选择普通文件和指向文件的软链接；用户 profile 按名称排序并优先于 default。保留入口路径用于重载，保存前解析真实目标并在其目录原子替换；断链、循环、非文件目标或只读目标明确失败，不替换链接，首次创建普通配置仍受支持。
- `app/cli.rs` 将启动选项和执行命令保存在同一 `CliOptions`，默认 `CliCommand::Run`；Windows 只为输出命令和解析错误准备控制台，不另维护参数白名单。执行命令（包括帮助／版本）以最后一个参数为准，不依赖枚举顺序；`--config` 只修改启动选项。内部 helper 在普通 CLI 解析前分派，重载沿用默认启动命令。
- UIHint 的标签字符在 catalog 规范化后直接交给模式；共享 Hint Settings 不保存原始字符集，Text Select 也不携带无用的 UIHint 字符集。Text Select 在编译绑定时排除占用的单字符键，Settings 只保存最终字符数组；禁用绑定和组合键不排除标签字母，原顺序保持不变。Quick Switch 的模式标签与捕获键分别在配置编译时保存为字符和 Key，运行时不重新解析；两种表示各供标签生成和键盘捕获使用。
- 字符输入与物理键身份分开；保持修饰键匹配、长按重复和 Down/Up 配对。
- 五个鼠标键 `mouse_left`／`mouse_right`／`mouse_middle`／`mouse_x1`／`mouse_x2` 均可作绑定触发或点击动作；前三项动作兼容原有 `left_click`／`right_click`／`middle_click`。Windows／macOS Hook 将物理按键交给普通绑定管线，未绑定或禁用时成对透传，不参与模式的键盘捕获或字符输入；回放保留鼠标目标身份。
- 生产 Reload 先验证并准备新进程，失败保留旧实例；旧实例有序退出后交接已验证配置。进程内 `set_config` 仍原子替换完整计划。
- 配置与工作区 I/O 不阻塞输入循环；写入失败不覆盖有效数据，格式变更需考虑兼容性。
- macOS 打包应用的数据在 `~/Library/Application Support/KeySteer/`；portable 使用可执行文件目录。数据目录不能取进程工作目录；显式相对配置路径另按 CLI 规则解析。

`keysteer.default.toml` 保留常用参数、完整默认键位和简短说明；高级扫描阈值、角色列表及 `[text_select.ui]` 省略后使用 Rust 默认值，不在入门文件中展开。文字选择键位按层级导航、选择与输入、退出与帮助分组；网页外观控件通过字段默认值显示省略的样式，仅编辑后写入覆盖。完整配置可用 `--dump-config` 查看，编辑键完整示例内嵌在中英文 Normal 文档的折叠代码块中，并由测试核对两份示例一致。修改用户配置语义时同步用户参考和网页编辑器适用部分。

只有 Window 的屏幕范围缺省为全部屏幕；Quick、Editor、Restore 和 Tabs 缺省为当前屏幕。发布配置、Rust 默认值及网页模拟器保持一致，网页保留屏幕范围选择并按模式说明默认值。各模式仍保留独立配置和入口目标语义。

网页按键编辑区共用同一绑定表，通过“动作／映射到按键”切换编辑原生动作或发送按键。文字选择的复制／清空输入使用同一按键映射区和动作列表，分别写回 `text_copy`／`text_clear_input`；不增加独立的快捷键控件或第二套配置。`config-studio/KeyMappingEditor.tsx` 提供常用键与点击录制：仅在录制按钮聚焦期间捕获按键，松开后应用，Esc／失焦／切换编辑对象取消；Esc 本身可通过常用键选择。`simulator/key-mapping.ts` 按物理键识别组合，录制实际 Ctrl／Alt／Shift／Win（macOS 为 Cmd），不擅自换成可被用户覆盖的 Primary。普通单键可导出 `t = "home"`，组合键和有歧义的名称使用显式 `send`；已有配置保留用户及平台别名。网页预览只显示发送结果，不向系统注入按键；动作序列不自动识别为单键映射。

网页 `config-studio/MouseDiagram.tsx` 提供五键鼠标图示，与键盘共用选中组合键、动作拖放、绑定继承及 TOML 写回入口。点击图示编辑该鼠标键的触发绑定；将图示按键拖到键盘，则把目标键绑定为对应的 `mouse_*` 点击动作，沿用当前组合键。图示只提交按键身份与指针事件，不维护第二套绑定或拖放状态。左键触发绑定只在实际设置、禁用或移除时弹出原生模态确认，确认前不改配置；选中查看、重复设置相同值及将左键点击动作拖给键盘均不弹出。检测沿用共享别名解析，也覆盖带修饰键的左键。鼠标区域使用容器查询适应编辑区宽度：宽布局直接在键盘右侧显示鼠标，不提供折叠箭头；窄布局默认折叠，通过键盘下方居中箭头展开到键盘下方。折叠时键盘占满可用宽度，完整键盘仍由 `FittedKeyboard` 缩放。

`scroll.speed` 是普通长按滚动的像素/秒，默认 500，独立于短按距离；接受有限非负小数，0 为仅短按。未发布的 `steps_per_second` 已移除，不提供兼容转换。模式装配时将短按距离和每秒速度编译为 `ScrollSettings`，半页／整页仅按次执行，其连续速度为 0。模式按键变化时缓存方向速度与持续手势状态，帧热路径只做时间积分，不读取配置或解析字段。网页 `docs/.vitepress/simulator/scroll.ts` 同样预编译速度，控件与移动速度一样直接编辑数值，不设额外速度档位，导出仅使用 `speed`，保留速度修饰键和 macOS 方向设置。

窗口卡片共用 `[window.card]`；选中底色、边框颜色与线宽可由 `selected_background_color`、`selected_border_color`、`selected_border_width` 覆盖。明暗主题薄荷绿及 1.5 线宽为内置默认值，默认配置和原生导出不写入这些默认字段；网页控件显示默认值，但只在编辑后生成对应覆盖，重置删除覆盖。颜色支持 `#RRGGBBAA` 和完整的浅深主题表，线宽为 0–20，0 隐藏卡片选中边框；勾选标记仍保留。原生在装配时预编译样式，网页编辑器、普通／选中对照与交互预览使用同一配置。

UIHint 的完整默认配置与部分 `search_input_ui`／`search_info_ui` 表共用面板自身的默认值函数，反序列化不构造整份 UiHint 来提取面板；信息面板不能退回输入框的通用宽度和屏幕锚点。补齐后沿用共享校验和启动样式编译，运行时不解析配置。

旧配置在 TOML 结构解析后、类型反序列化前统一处理；原生 `Config::parse`／带诊断的加载路径与网页 `parseConfigDocument` 各调用一次兼容模块。共享 `compatibility.json` 按路径声明 `ignore`（丢弃）、`reject`（提示替代项）、`copy`（按优先级补齐缺省字段）、`reject_values`（旧绑定名）和表格式转换；路径的 `*` 匹配一级表名或数组项，覆盖应用覆盖表、插件及动作序列。简单规则只需修改该表，复杂转换只放在兼容模块；当前字段的格式校验仍留在各自模型中。

旧滚动反转值在入口迁移为两个当前轴字段，保持显式值、新旧开关及缺省平台表的原有优先级；DTO、设置解析和导出不再保留旧字段或回退逻辑，导出直接序列化当前模型。旧搜索编辑“动作 = 按键”表在此转换，后续反序列化和网页预览只处理“按键 = 动作”；空键组仍可分别禁用动作。旧五项复制键在入口丢弃末尾已移除的标签项，校验与模式装配只接收四项。诊断随加载结果返回并沿用启动日志策略，不保存在 Config 或运行时 Settings 中。

`build.rs` 调用 `build/compatibility.rs`，在编译时解析并校验共享 JSON，将字段、分段路径和转换枚举生成为 `OUT_DIR/compatibility_rules.rs` 中的静态借用数组；原生通过 `include!` 使用，不在启动时解析 JSON 或构造规则。网页仍将同一 JSON 打入构建产物，两端均无需部署独立规则文件。兼容处理只在配置加载时应用一次，不在输入、帧处理、导出或网页预览合并时重新迁移。当前格式配置的原生兼容遍历有零堆分配测试；实际旧字段转换和诊断仅在加载时按需分配。`tests/fixtures/config-compatibility.json` 由两端共同验证原文规范化、优先级、诊断、幂等和导出回读。命名锚点、分数／小数等仍受支持且有独立语义的当前语法不属于旧字段迁移；保留其正常解析与数值编译。

两个搜索面板与窗口卡片共用四边百分比的校验、区域、中心点和边界限制计算；面板统一由 `CompiledSearchPanel::place` 定位，信息面板直接使用实际高度，不重复计算输入框布局。仅用 `position_mode` 与 `position` 定位，百分比限定为 0–100%；旧 `x_offset`／`y_offset` 兼容接受但忽略，类型模型和导出不保留。输入框缺省为当前屏幕高度的 95%；信息面板以输入框中心为基准，上／下半区决定上方／下方，两面板半高之和先分离边缘，再以区域中心距 50% 中线的比例乘以总高度计算间距。缺省 `44% / 56%` 在普通字号下约留 12 像素，四个 `50%` 紧贴上方。`window` 参照识别窗口，无窗口时退回当前屏幕。网页复用 `CardPositionEditor` 的点选、范围拖动、预设及四边滑块，修改直接更新右侧实时预览的 `SearchPanelsOverlay`；它与 `simulator/search-panel-position.ts` 共用定位计算，并通过 `config-studio/preview-geometry.ts` 与桌面背景共用屏幕／窗口矩形，设置区不另绘制布局预览。编辑写回各自 TOML 路径。`tests/fixtures/search-panel-layout.json` 同时验证 Rust／网页的窗口回退、负屏幕坐标、缩放、贴边、超大尺寸和跟随间距；运行时仅计算当前几何，定位与样式复用有零分配回归测试。

`ui_hint.search_highlight` 只覆盖当前预览／已选单点的标签底色、文字、边框和线宽；缺省复用 Window 的选中绿色。可选主题颜色在 catalog 预编译，默认值不导出；网页外观页有相同校验、重置和 Tab 切换预览。绘制层从当前信息面板及 Point position 取标签身份，只改该标签样式并提升到标签顶层；普通 Hint 标签循环保持不变，集合中心不标记单个成员。

`ui_hint.search_edit_keys` 对外使用“按键 = 动作”表，加载时按动作归并并覆盖该动作的默认键；省略动作保留默认值。内部沿用动作到快捷键的模型和编译后的 Settings；与信息复制键一起校验冲突，并在启动阶段解析为 KeyChord。编辑键、复制键与 `ui_hint.search_bindings` 一起在启动时展开组合键内部的配置别名；`primary` 遵循平台别名覆盖（默认 macOS 为 Cmd、Windows 为左 Alt、Linux 为 Ctrl），也允许显式 cmd、ctrl。

搜索编辑键在左侧以空格分隔多个快捷键，也可多行绑定同一动作，启动时展开与校验，不在输入热路径解析。默认 `"enter / primary+q" = "accept"`。旧“动作 = 按键”表兼容读取，原生导出统一使用新格式并保留禁用动作；网页导入也转换为新格式，按动作合并稀疏覆盖并复用现有绑定控件。

颜色原文只保留在配置模型中，用于校验和序列化。网格、Hint、按键帮助与模式指示器在装配时转换为 `CompiledColor` 和 `style::compiled` 样式，运行时只选择浅／深色数值；未配置值及无效程序化颜色仍保留原有回退语义。搜索面板、窗口卡片和快速切换沿用已有整样式编译。透明度、对比度和场景默认色在绘制时派生，不因提前解析而冻结；主题变化选择另一组数值，配置重载重建整个计划。

Window 多选入口与清空沿用 `window.bindings` 的 `window_multi_select`／`window_multi_clear`；输入期间由 `window.multi_select.bindings` 覆盖确认及编辑键，默认 Ctrl／Enter 确认、primary+h/l 移动输入光标。编辑键显式写在该表，支持 arrow_left/right、home/end、backspace/delete 及 Shift 方向选区，不从 Normal 复制配置。空格仍表示多个快捷键别名，也不改变 `window.target`。

搜索点位操作使用 `ui_hint.search_bindings` 的“按键 = 动作”表（`point_toggle` / `point_next` / `color_next`；point_next 默认 Tab，搜索输入时循环结果预览、调整期间切换已选点），显式表替换默认映射；点位样式和格式顺序在 `ui_hint.search_point`，缺省字段不展开到导出配置。网页控件沿用此替换语义。四栏多点展示由 search_point.field_modes 固定长度数组配置，concat 拼接初始目标信息、switch 跟随当前检查点，默认 [concat, concat, switch, switch]；默认值不展开到导出。颜色与其他三栏一样可独立配置：concat 逐点异步采样并按选择顺序拼接，switch 跟随当前检查点。反序列化直接形成四项枚举，catalog 复制到 Settings，模式只接收编译后的按键、颜色、字段行为与格式枚举，运行时不解析字符串。

四个搜索信息复制键继续使用 `ui_hint.search_copy_keys` 独立数组配置，默认 Ctrl+1/2/3/4，两平台一致；显式 primary 仍按别名展开。`search_point.color_preview` 控制 Color 标题旁色块的开关、宽高、水平／垂直偏移与边框线宽；默认表及未改字段不导出。网页外观页提供同范围控件和预览，重置删除覆盖。

Point 输入框的 `search_point.input_background_color`／`input_border_color` 为可选主题颜色；缺省背景采用窗口多选卡片同款浅／深绿色，边框继承普通搜索框。两项在装配时预编译，默认值不写入配置；网页外观页同时编辑普通搜索框和 Point 覆盖并预览对照。

`ui_hint.search_match_priority` 是固定三项枚举数组，label／text／pinyin 各一次，默认 pinyin／text／label（简拼、文字、标签）；只控制同等匹配程度下的结果预览类别顺序，不改变普通完整标签解析和显式 @label 语义。完整词／标签、前缀、包含匹配依次排列，同分保持扫描顺序、空格词项保持输入顺序。配置校验拒绝缺项、重复及未知值，catalog 将已解析枚举编译为 64 项匹配度／优先级查表，运行时不遍历配置；网页编辑器提供相同校验和重置。

快速切换通过 `quick_switch.hint_characters` 配置有序标签字符，默认 `wertyuiop`；名称与 UIHint 一致。启动时校验至少两个规范化后不同的字符且不含触发键，编译为字符数组；网页导入、控件与预览使用同一语义。可显式设为 `123456789` 保留数字选择偏好。

`quick_switch.bindings` 与其他模式一样整表替换默认绑定，默认 Esc／Q 为 `cancel_selection`，Backspace 为 `previous_level`；可仅保留其中一个退出键。校验阻止绑定占用标签字符。模式样式与候选设置在 catalog 装配，Engine 只保留入口手势与原生字符注册所需配置。

`text_select` 配置启用状态、独立 `scan_scope`、区域层标签字符、绑定及 `lifecycle.after_copy`；首层包围框、鼠标落点和细分网格样式集中在 `[text_select.ui]`，OCR 参数与面板外观复用 `ui_hint`，扫描范围独立，二者均默认 `all`；Normal 默认 `c` 进入。复制后生命周期缺省为 `normal`，原生与网页行为控件共用 keep／restart／return／模式名词汇，拒绝 finish 与点击动作。旧配置缺少 `[text_select]`／`[text_input]` 时沿用模式默认设置，并为旧 Normal 绑定表补上未被占用的 `c`／`\` 入口；同模式已有改键、明确 `none`、显式模式表及禁用设置优先。原生与网页解析保持一致。文字选择默认 `Alt+C` 细分（普通 `c` 是搜索文字）、Ctrl+Backspace 重置、Tab 循环翻页、Backspace 有输入时删除／空输入时清空勾选、Ctrl+A 有输入时全选输入／空输入时全选勾选、Alt+Backspace 返回上层、`"mod+c enter" = "text_copy"` 始终复制面板勾选文字、`ctrl+x = text_clear_input` 清空整个输入框（Windows/Linux Ctrl+C，macOS Command+C；`mod` 默认不受 Primary 别名影响），Space 在区域层分隔搜索项，在细分层确认编号／区间；`?` 显示实际绑定帮助。细分层固定使用十进制编号，网页 hint_characters 字段标为“区域层标签字符”，不影响数字选择。`text_select.ui.border_color` 可选主题颜色、`border_width` 为 0–20（0 隐藏）、`padding = [x, y]` 为两侧分别外扩的 0–100 整数，默认 `[2, 2]`；颜色在 catalog 编译，鼠标偏移采用 i16 二元数组，与 padding 独立。网页控件及导入使用相同范围，均位于“文字选择 → 外观”。`TextSelectionGeometryEditor` 在同一示意图显示原始区域、对称外扩包围框和鼠标热点，以拖动／滑块替代两项数组文本框，支持方向键与拖动取消；坐标换算使用原区域右下角，拖动期间固定视口，导入大偏移自动适配显示但不改配置值。`panel_width` 默认 800，范围 320–2400，catalog 仅覆盖文字选择自身的编译后信息面板宽度，不更改 UIHint。`word_min_width`／`char_min_width` 为 24–240 整数，默认 48／40，控制两级卡片的最小宽度；原生和网页均拒绝未发布的根级 `pointer_offset` 及 `selection_outline` 结构。预览和细分勾选语义及复制生命周期见[模式生命周期](04-modes-and-lifecycle.md)。

旧控制键通过各模式 `bindings` 配置，兼容补齐规则见上文；其他普通默认动作不会重新补入显式表。默认键及旧行为保持：网格 Space／Tab／Backspace／Enter／Esc，UIHint `/`／Enter／Esc／Backspace，以及仅在完成后生效的 Tab；窗口与屏幕选择也保留原键。网页按键面板暴露这些动作，默认值仍来自发布配置。
