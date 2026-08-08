# 安全与性能约束

KeySteer 的高优先级路径是“原生键盘回调必须立即决定这个物理键怎样处理”。性能优化不能改变这一点，也不能把平台资源、按住状态或过期异步结果遗留到下一次会话。

## 关键不变量

- 输入 Hook 不做 UI 扫描、配置 I/O、外部进程等待或固定间隔 sleep。它只把输入交给 Engine，并得到 `Consume`、`Defer` 或 `Forward` 的处置结果。
- 每个物理键的 key-down 与 key-up 必须保持同一归属；Mode 切换后，持续移动、滚动、速度修饰和临时模式键仍由最初 owner 收尾。
- 鼠标按住、长按点击 Toggle、普通点击指示颜色、定时序列、扫描 owner、覆盖层和帧时钟都有明确的清理点：模式结束/切换、配置重载失败、禁用与进程退出。
- Mode 和 plugin 不直接调用 Win32、AppKit、UIA 或 AX。它们只产生公共 `Command`，由 Engine 和 Backend 处理平台细节。
- 便携层（`api`、`app`、`config`、`domain`、`modes`、`plugins`）保持 safe Rust；原生 FFI 集中在平台层，并由局部封装维持所有权、线程和指针不变量。

## 连续移动和显示帧

连续鼠标移动不是用“每 16ms 加一次坐标”的软件计时器实现。启动移动后，Normal Mode 请求帧时钟，收到每帧的实际 `elapsed` 后积分速度；因此 60 Hz、144 Hz 以及帧偶发延迟的总位移都以墙钟时间为准。

- Windows：帧 worker 等待当前输出的 DXGI 垂直同步，不轮询刷新率。切屏时重绑目标输出；长帧跨过加速终点时，速度曲线段与匀速段分别积分，避免丢失距离。
- macOS：使用 Core Video 的 `CVDisplayLink` 提供显示帧回调。
- Windows 的 `WaitForVBlank(INFINITE)` 仅发生在可取消的帧 worker。它不阻塞输入 Hook、Engine 或覆盖层线程；停止帧时钟、输出切换和退出都会唤醒并收尾 worker。`INFINITE` 避免空转轮询，不表示内存或线程无法释放。

Normal 的平滑加速使用 `smootherstep` 曲线并解析积分；关闭 `pointer.smooth_acceleration` 后回退为线性加速。两种模式都在释放最后一个方向键时立即停止，不实现惯性滑行。亚像素余数、对角线归一化和速度修饰必须继续在实际帧间隔上工作。

## 覆盖层与异步任务

Windows 覆盖层提交使用最新 `OverlayScene` 的共享快照，提交方不等待绘制完成。macOS 覆盖层由自己的 `NSPanel`、View 和 Core Animation 图层管理，所有 UI 更新必须回到正确线程。

UIA、AX 与 Vision 扫描放在 worker；结果带 scan id 与 owner，Engine 丢弃旧 Mode、旧屏幕或取消后返回的结果。新增异步操作至少要定义：owner/generation、取消路径、队列或结果上限，以及不阻塞 Engine 的交付方式。

## 变更检查清单

改输入或平台代码时，除单元测试外至少人工检查：

- key-down/key-up、自动重复、模式切换和退出后没有按键或鼠标卡住；
- 低刷新率、高刷新率、切屏和短暂掉帧下连续移动没有按帧数计算的距离误差；
- 配置或输入错误恢复后，不残留覆盖层、普通点击颜色、timer、扫描或帧 worker；
- Windows 和 macOS 都有对应 Backend 行为，未支持平台返回清晰错误。

发布前运行：

```bash
cargo fmt --check
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --no-deps
pnpm docs:check
pnpm docs:build
```
