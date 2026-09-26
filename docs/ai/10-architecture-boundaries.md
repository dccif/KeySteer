# 架构边界与运行计划

面板几何请求只用 API 的请求编号、进程 id 和 Rect 跨层，借用现有窗口 worker，不新增 Engine 原生依赖、全局线程或定时器。原生确认的 deadline/poll 端口留在 `platform/common/window_session`，Windows 拥有 HWND 身份与实际确认，Mode 不参与原生焦点轮询。

Windows native 按资源所有权拆分，详见项目地图；`capture` 与 `dimensions` 编译期禁止 unsafe。线程限定的资源/工厂使用零尺寸标记保证 !Send/!Sync，不能为了排队方便增加 unsafe Send/Sync。代码移动仅重分配逐文件 unsafe 预算，总预算仍向下收紧。

KeySteer 保持单 crate，并采用混合模块布局：聚合目录使用 `mod.rs`，简单叶子使用
`foo.rs`。文件命名不是架构规则；依赖方向和状态所有权才是。`tests/architecture_dependencies.rs`
锁定稳定内层：`support` 不依赖业务层，`api` 是依赖底座，`config` 不访问 app/platform，
`runtime` 消费 api/support/presentation，`presentation` 只依赖 api；生产 Mode/Plugin 不读取配置、平台或具体 presentation 类型，也不构造场景原语，平台层不访问业务层。

`HostContext` 注入 API 的 `Presenter` 端口。借用视图在调用内同步消费，输出沿用 ShowOverlay；
Mode 的状态与 compositor 的布局算法分离。统一场景构建和扩展方式见 [覆盖层](07-rendering-and-performance.md#统一场景构建)。

## 配置编译

`config::ConfigFile` 只代表 TOML 文档、默认值和校验。`app::configuration::compile` 是唯一编译
边界，产出与 TOML 无关的 `app::runtime::RuntimePlan`：Engine settings、明暗 palette，以及
原子聚合的 `ModeSpec`。每个 spec 同时持有 Mode/Plugin 实例和完整 `ModeRoute`，不会出现实例、
路由或 ID 漏配。`app::mode_catalog` 是唯一内置 Mode 注册点，每个构造函数只接收自己的强类型
`Settings`，并在同一个 catalog 项编译 enable、继承、temporary keys 与 app override。

Engine 只持有 `app::runtime::ConfigurationRepository` trait object；TOML、配置发现、comment-preserving
store 和平台原子替换由 `app::configuration::ConfigRepository` 适配。库的常规公开面只保留
二进制启动入口；内部跨模块测试位于 `src/tests/`。独立 benchmark 通过 `benchmark-hooks`
只提升既有生产构造器的可见性，不启用诊断用 `perf-probe`、替代算法或运行参数。

## Runtime 所有权

布局库沿用依赖反转：Engine 只持有 `PresetRepository` trait object（列表、保存和 handoff 导出），`app/preset_store.rs` 实现二进制持久化并由 bootstrap 注入路径和平台 atomic_replace。Runtime 的 `PresetController` 只拥有原生备注请求、会话归属和完成路由，不导入 app/config 的具体 store 或文件替换类型。

Engine 组合四类有状态协作者：`ModeRegistry`、`InputState`、`Scheduler` 和
`OverlayCoordinator`。Mode 通过 `claims_key` 与 `wants_pointer_events` 声明自己的输入兴趣；
Engine 不识别 Grid/UI Hint 字符表。`ModeRegistry` 独占实例、活动 slot、路由、插件 manifest、
modal stack 和生命周期；其他三个协作者分别独占输入配对、deadline 与 overlay 呈现状态。

## 原子热重载

候选配置先完整 parse、validate、compile；失败时当前 Mode、timer、scan、overlay 和合成输入均
不改变。成功候选在当前命令批次边界执行受控重启：取消 scan/timer/sequence/modal/frame clock，
释放 KeySteer 拥有的合成输入，替换完整 Mode/Plugin 集和路由，再进入新计划的 Idle。物理按键
的 pressed/disposition 记录保留到真实 KeyUp，因此不会吞 Down 却放行 Up。Mode 不接收
`ConfigReloaded`，`HostContext` 也不暴露配置。


## 工作区与原生资源的所有权

PresetRepository 增加异步提交端口，具体仓库拥有磁盘 worker 和有界 mailbox；Runtime 只拥有请求的完成归属，不持有文件锁或工作线程。完成消息使用 api 类型并经 Backend 已有事件发送端唤醒 Engine。模式结束可以撤销完成路由，但不回滚已提交写入；持久化写操作仍串行，退出等待由仓库统一负责。

窗口会话的状态、排队策略、确认状态机、历史和相交缓存分模块高内聚，仍在同一 crate，不增加动态分发层或每操作分配的策略对象。原生 handle 的 owning wrapper 与 borrowing guard 放在 platform/windows/native，portable 层继续禁止 unsafe。

配置 I/O 的实现仍在应用层 repository，runtime 的配置 worker 只经 ConfigurationRepository 端口执行；ConfigurationReady 事件不携带应用层类型。窗口布局确认与失败回滚集中在公共 window_session，不让 Mode / Engine 获取原生句柄。
