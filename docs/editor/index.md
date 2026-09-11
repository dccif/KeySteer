# 配置与模拟器(Beta)

配置与模拟器适合“先试试再保存”：在浏览器中编辑键位、预览鼠标操作、调整 `Grid`/`Recursive Grid`/`UI Hint` 的样式。数据只在本机浏览器中处理，不会上传。复杂动作、外部命令和高级字段请以 TOML 文档为准。

KeySteer 0.8.11 及更高版本可从 Windows 托盘或 macOS 顶部状态图标选择 **Configuration & Simulator...**，直接带入当前生效的配置。配置经 URL fragment 交给浏览器，GitHub Pages 请求不会包含它；页面读取后会立即清除 fragment。浏览器扩展在交接瞬间理论上仍可能读取页面地址，因此不要在配置命令中保存密码或令牌。

<p class="ks-open-simulator">
  <a href="../simulator" target="_blank" rel="noopener">打开配置与模拟器 ↗</a>
</p>

## 推荐用法

已有 `workspace.ksw` 时，从程序菜单打开模拟器会同时带入当前按键配置和布局、Tabs 预设列表，无需另选文件。选择布局编号后编辑分区；Ctrl+S 更新该布局的备注，或直接点“保存并下载工作区”保存当前修改并下载整个工作区文件，保留其他预设。把下载的 `workspace.ksw` 替换程序同目录的同名文件即可；每次 `Alt+W → R` 都重新读取，不需要重启、Reload Configuration 或文件监听。网页也保留“导入工作区文件”作为独立入口。

1. 打开模拟器，导入现有 TOML，或从默认配置开始。
2. 修改键位和模式样式，观察预览。
3. 下载生成的 `keysteer.<名称>.toml`。
4. 放入 KeySteer 数据目录，点击状态栏菜单中的 Reload Configuration。
5. 遇到配置错误时运行 `keysteer --check -c <文件>`，以 Rust 程序的校验结果为准。

模拟器适合快速试错和预览，但不是完整配置，而且可能出错。复杂字段、平台权限、外部命令和最终配置校验仍由 KeySteer 程序负责。完整语法见 [配置文件](/reference/configuration) 和 [模式与动作](/reference/modes-and-actions)。

模拟器会在新页面打开，以免宽键盘布局遮挡文档侧栏；也可以把它放到另一块显示器上使用。
