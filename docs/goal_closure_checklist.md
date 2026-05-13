# goal.md 闭环检查

日期：2026-05-13（最终复核）

## 结论

在“冻结列目标暂时移除”的前提下，当前 `goal.md` 的核心功能链路已闭环。

## 闭环项与证据

1. 秒级打开 / 不等待完整加载 / 大文件可预览
- 证据：`src/core.rs` 行偏移索引+分页读取；`src/worker.rs` 后台打开；5GB 报告 `/tmp/csvfastview-5gb-report.txt`。

2. 浏览流畅与交互不冻结
- 证据：UI 主线程只渲染当前页；过滤与搜索在 worker 线程；支持取消。

3. 过滤能力
- 证据：按列 contains/equals/unique；进度显示；取消；结果数量。

4. 搜索能力（独立语义）
- 证据：`Job::Search` / `Event::Search*` 独立任务、独立进度、取消、结果列表、点击跳转。

5. 列操作
- 证据：列名展示、列宽调节、隐藏/显示。

6. 行定位与行号
- 证据：jump/go；表格 `#` 行号列。

7. 导出
- 证据：导出当前视图；导出搜索结果。

8. CSV 兼容与自定义
- 证据：delimiter/quote/headers/flexible；支持含引号、空字段、不规则行（flexible 模式）。

9. 编码兼容
- 证据：UTF-8/GBK/GB18030/Big5/Shift-JIS/ISO-8859-1 手动选择；Auto Detect 做基础编码推断。

10. 状态反馈与错误提示
- 证据：索引、过滤、搜索过程状态文本与进度条；失败状态显示错误原因。

11. 本地优先
- 证据：无上传逻辑，纯本地文件读写。

12. 平台兼容
- 证据：桌面 `eframe/egui`；`cargo check --target` 已验证 linux/windows/macos 目标；CI 矩阵配置存在。

## 验证门禁

- `cargo check -q` 通过
- `cargo test -q` 通过（4 tests）
- 5GB 合成样本基准通过并有报告

## 说明

- 如果后续恢复“冻结列”目标，需要把该项重新加入闭环清单并继续实现增强。
