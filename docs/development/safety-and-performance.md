# 安全与性能约束

KeySteer 的高优先级路径是“原生键盘回调必须立即决定这个物理键怎样处理”。性能优化不能改变这一点，也不能把平台资源、按住状态或过期异步结果遗留到下一次会话。

## 关键不变量

- 输入 Hook 不做 UI 扫描、配置 I/O、外部进程等待或固定间隔 sleep。它只把输入交给 Engine，并得到 `Consume` 或 `Forward` 的处置结果；`Defer` 只为 API 兼容保留，内置运行时按 `Forward` 处理。
- 每个物理键的 key-down 与 key-up 必须保持同一归属；Mode 切换后，持续移动、滚动、速度修饰和临时模式键仍由最初 owner 收尾。
- 鼠标按住、长按点击 Toggle、普通点击指示颜色、定时序列、扫描 owner、覆盖层和帧时钟都有明确的清理点：模式结束/切换、配置重载失败、禁用与进程退出。
- Mode 和 plugin 不直接调用 Win32、AppKit、UIA 或 AX。它们只产生公共 `Command`，由 Engine 和 Backend 处理平台细节。
- 便携层（`api`、`app`、`config`、`domain`、`modes`、`plugins`）保持 safe Rust；原生 FFI 集中在平台层，并由局部封装维持所有权、线程和指针不变量。

## 连续移动和显示帧

连续鼠标移动不是用“每 16ms 加一次坐标”的软件计时器实现。启动移动后，Normal Mode 请求帧时钟，收到每帧的实际 `elapsed` 后积分速度；因此 60 Hz、144 Hz 以及帧偶发延迟的总位移都以墙钟时间为准。

- Windows 11：帧 worker 阻塞在带 stop event 的 DirectComposition compositor clock，原生适配混合刷新率；旧系统回退为当前输出的 DXGI VBlank，再回退 `DwmFlush`。路径都不查询或轮询刷新率；长帧跨过加速终点时，速度曲线段与匀速段分别积分，避免丢失距离。
- macOS：使用绑定到覆盖层 View 的 `CADisplayLink`，由 AppKit 随 View 所在显示器切换节拍。
- Windows 11 的无限期等待同时等待 compositor frame 和 stop event，不阻塞输入 Hook、Engine 或覆盖层线程；停止帧时钟和退出会设置 event，随后 join 并释放 worker。无限等待避免空转轮询，不表示内存或线程无法释放。

Normal 的平滑加速使用 `smootherstep` 曲线并解析积分；关闭 `pointer.smooth_acceleration` 后回退为线性加速。两种模式都在释放最后一个方向键时立即停止，不实现惯性滑行。亚像素余数、对角线归一化和速度修饰必须继续在实际帧间隔上工作。

## 覆盖层与异步任务

Windows 覆盖层提交使用 latest-frame 单槽和 `OverlayScene` 共享快照，提交方不等待绘制完成。场景中的静态 shapes/labels 是写时复制数组；cursor/indicator 更新不得复制、排序或逐项比较整张 Grid。Windows 高 DPI 渲染只在静态标签或目标 scale 改变时重建缩放缓存，并在空场景、100% scale、Hide/Shutdown 时释放。macOS 覆盖层由自己的 `NSPanel`、View 和 Core Animation 图层管理，所有 UI 更新必须回到正确线程；Hide 会断开 content view/layer tree，不把大场景作为池容量保留。

UIA、AX 与 Vision 扫描放在 worker；结果带 scan id 与 owner，Engine 丢弃旧 Mode、旧屏幕或取消后返回的结果。UI Hint 活动期间复用扫描数组，离开大型扫描后释放其 backing allocation。新增异步操作至少要定义：owner/generation、取消路径、队列或结果上限，以及不阻塞 Engine 的交付方式。

## 变更检查清单

改输入或平台代码时，除单元测试外至少人工检查：

- key-down/key-up、自动重复、模式切换和退出后没有按键或鼠标卡住；
- 低刷新率、高刷新率、切屏和短暂掉帧下连续移动没有按帧数计算的距离误差；
- 配置或输入错误恢复后，不残留覆盖层、普通点击颜色、timer、扫描或帧 worker；
- Windows 和 macOS 都有对应 Backend 行为，未支持平台返回清晰错误。
- Windows `SendInput` 失败日志必须保留实际返回数量和即时 `GetLastError`，并只在失败路径
  查询当前/前台进程的 session、integrity、elevation 与 UIAccess。UIPI 可能不设置错误码，
  因此日志报告证据而不把所有零返回都断言为权限问题。
- Windows Engine 完成键盘 disposition 后，不得立即与尚未返回的低级 Hook 回调竞争
  `SendInput`。鼠标按钮、滚轮和键盘注入先等待 Hook 线程处理一个原生消息屏障；该屏障
  由事件唤醒且只在注入前执行，不得改成 sleep、轮询或连续移动热路径上的固定等待。

发布前运行：

```bash
cargo fmt --check
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --no-deps
pnpm docs:check
pnpm docs:build
```
