# 配置与模拟器(Beta)

配置与模拟器适合“先试试再保存”：在浏览器中编辑键位、预览鼠标操作、调整 `Grid`/`Recursive Grid`/`UI Hint` 的样式。数据只在本机浏览器中处理，不会上传。复杂动作、外部命令和高级字段请以 TOML 文档为准。

<p class="ks-open-simulator">
  <a href="../simulator" target="_blank" rel="noopener">打开配置与模拟器 ↗</a>
</p>

## 推荐用法

1. 打开模拟器，导入现有 TOML，或从默认配置开始。
2. 修改键位和模式样式，观察预览。
3. 下载生成的 `keysteer.<名称>.toml`。
4. 放入 KeySteer 数据目录，点击状态栏菜单中的 Reload Configuration。
5. 遇到配置错误时运行 `keysteer --check -c <文件>`，以 Rust 程序的校验结果为准。

模拟器适合快速试错和预览，但不是完整配置，而且可能出错。复杂字段、平台权限、外部命令和最终配置校验仍由 KeySteer 程序负责。完整语法见 [配置文件](/reference/configuration) 和 [模式与动作](/reference/modes-and-actions)。

模拟器会在新页面打开，以免宽键盘布局遮挡文档侧栏；也可以把它放到另一块显示器上使用。
