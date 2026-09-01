# 架构边界与运行计划

KeySteer 保持单 crate，并采用混合模块布局：聚合目录使用 `mod.rs`，简单叶子使用
`foo.rs`。文件命名不是架构规则；依赖方向和状态所有权才是。`tests/architecture_dependencies.rs`
锁定稳定内层：`support` 不依赖业务层，`api` 是依赖底座，`config` 不访问 app/platform，
`runtime` 只消费 api/support，生产 Mode/Plugin 不读取配置或平台类型，平台层不访问业务层。

## 配置编译

`config::ConfigFile` 只代表 TOML 文档、默认值和校验。`app::configuration::compile` 是唯一编译
边界，产出与 TOML 无关的 `runtime::RuntimePlan`：Engine settings、明暗 palette、路由、应用
override，以及已经实例化的 Mode/Plugin。`app::mode_catalog` 是唯一内置 Mode 注册点，每个
构造函数只接收自己的强类型 `Settings`。

Engine 只持有 `runtime::ConfigurationRepository` trait object；TOML、配置发现、comment-preserving
store 和平台原子替换由 `app::configuration::ConfigRepository` 适配。库的常规公开面只保留
二进制启动入口；内部跨模块测试位于 `src/tests/`，独立 benchmark 只能通过 `perf-probe`
下的 doc-hidden hook 访问实现。

## Runtime 所有权

Engine 组合四类有状态协作者：`ModeRegistry`、`InputState`、`Scheduler` 和
`OverlayCoordinator`。Mode 通过 `claims_key` 与 `wants_pointer_events` 声明自己的输入兴趣；
Engine 不识别 Grid/UI Hint 字符表。

## 原子热重载

候选配置先完整 parse、validate、compile；失败时当前 Mode、timer、scan、overlay 和合成输入均
不改变。成功候选在当前命令批次边界执行受控重启：取消 scan/timer/sequence/modal/frame clock，
释放 KeySteer 拥有的合成输入，替换完整 Mode/Plugin 集和路由，再进入新计划的 Idle。物理按键
的 pressed/disposition 记录保留到真实 KeyUp，因此不会吞 Down 却放行 Up。Mode 不接收
`ConfigReloaded`，`HostContext` 也不暴露配置。
