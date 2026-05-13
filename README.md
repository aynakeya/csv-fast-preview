# CSV Fast View

CSV Fast View 是一个本地优先、低内存的大型 CSV 预览工具。目标是快速打开 5GB 级别甚至更大的 CSV 文件，先展示可用预览，再在后台持续建立行索引。

## 当前状态

- 使用 Rust + `eframe/egui` 实现桌面 GUI。
- CSV 解析、索引、搜索、过滤、导出逻辑集中在 `core + worker`。
- `app` 只负责 GUI 状态、控件绘制、发送 `Job`、消费 `Event`。
- 打开文件后会先做 preview，然后后台 indexing。
- indexing 期间 worker 主循环仍然可响应行读取请求，表格可以显示已经索引出的内容。
- 表格是虚拟滚动，不再使用分页按钮。
- 支持横向滚动、列显隐、跳转行、单元格 hover 查看完整内容。
- 支持跳过文件开头 N 行后再解析 CSV。
- 启动时会加载系统 CJK 字体 fallback，用于显示中文和其他 Unicode 内容。
- 行内容读取由 worker 异步完成，并带 worker 侧行缓存，避免 GUI 滚动时直接读文件。

## 目录结构

```text
src/
  lib.rs                 # 库入口，暴露 app/core/worker
  main.rs                # GUI 二进制入口，只调用 app::run()
  app/                   # GUI 层，只处理界面和 UI 状态
    mod.rs
    state.rs             # GUI 状态、可见行缓存、向 worker 请求行
    events.rs            # 消费 worker Event，更新 GUI 状态
    fonts.rs             # GUI 字体 fallback 配置
    ui.rs                # egui 控件和表格绘制
    constants.rs
    format.rs
  core/                  # 纯 CSV 逻辑
    mod.rs               # core 对外 API 和测试
    config.rs            # CsvConfig、CsvEncoding、字段解码
    sniff.rs             # 分隔符/表头/编码探测
    index.rs             # 行 byte offset 索引、按 offset 读取
    query.rs             # filter/search
  worker/                # 后台服务层
    mod.rs               # Worker 对外入口
    message.rs           # Job/Event
    snapshot.rs          # CsvSnapshot，给 UI 的轻量文件状态
    runtime.rs           # worker 主循环、后台 indexing、请求分发
    row_cache.rs         # worker 侧行缓存
    export.rs            # 导出当前行集合
  bin/
    bench.rs             # 命令行 benchmark
```

## 分层约定

`core`：
- 不依赖 GUI。
- 负责 CSV 配置、编码解码、sniff、索引、按行读取、搜索、过滤。
- 对外提供 `CsvConfig`、`CsvEncoding`、`CsvIndex`、`FilterMode`、`sniff_csv`、`sniff_csv_with_skip`。

`worker`：
- 不依赖 GUI。
- 持有当前 `CsvIndex` 和 worker 侧行缓存。
- 把 core 能力包装成异步 `Job/Event`。
- indexing 在独立线程执行，worker 主循环可以继续响应 `ReadRows`。

`app`：
- 不直接持有 `CsvIndex`。
- 不直接读取 CSV 文件，不调用 `CsvIndex::read_page`。
- 只保存 GUI 所需状态，例如路径、显示列、当前逻辑行、已返回的行缓存、搜索结果等。
- 通过 `Job::OpenFile`、`Job::ReadRows`、`Job::Filter`、`Job::Search`、`Job::ExportRows` 请求 worker。

## GUI 使用方式

启动：

```bash
cargo run
```

打开 CSV：

1. 在 `File` 输入框填入本地 CSV 路径，或点击 `Browse` 选择文件，也可以把文件拖到窗口上直接打开。
2. 设置 `Delimiter`、`Quote`、`Skip`、`Headers`、`Flexible`、`Encoding`。
3. 需要自动识别时点 `Auto Detect`。
4. 点 `Open`。

`Skip` 表示解析前跳过多少条 CSV record，适合文件开头有说明文字、元数据行的情况。跳过后，如果 `Headers` 勾选，则跳过后的第一条记录作为表头；否则从跳过后的第一条记录开始作为数据。

如果系统字体路径不在默认探测列表中，可以通过环境变量指定字体文件：

```bash
CSVFASTVIEW_FONT=/path/to/NotoSansCJK-Regular.ttc cargo run
```

浏览：

- 鼠标滚轮纵向滚动。
- 底部横向滚动条查看宽 CSV。
- 左侧 `Columns` 控制列显隐。
- 底部 `jump` 输入逻辑行号后点 `Go` 跳转。
- 点击单元格会打开完整内容窗口。

搜索和过滤：

- `Column filter` 支持 `contains`、`equals`、`unique`。
- `Search All Cols` 会搜索所有列。
- 过滤和搜索都在 worker 中执行，并显示进度文字。

导出：

- `Export Current View` 导出当前可见窗口附近的行。
- `Export Search Results` 导出搜索结果行。
- 导出路径由 `Export` 输入框指定。

## Benchmark

```bash
cargo run --bin bench -- /path/to/large.csv , utf8 0 keyword
```

参数含义：

```text
<csv_path> [delimiter] [encoding] [filter_col] [keyword]
```

支持编码：

- `utf8`
- `gbk`
- `gb18030`
- `big5`
- `shift-jis`
- `iso-8859-1`

## 大文件行为说明

- 内存中主要保存行 byte offset，而不是完整表格。
- GUI 层只缓存 worker 返回的少量显示行。
- worker 层有独立行缓存，用于减少快速滚动时的重复 seek/read。
- indexing 期间会持续更新已索引行数，已索引出的行可以被 GUI 请求显示。
- 快速拖动到未加载区域时，表格可能短暂显示空白，worker 返回行数据后会补上。

## 已知限制

- 搜索/过滤仍是全文件扫描，大文件下需要时间。
- 快速拖动到很远位置时仍可能先白屏再显示，这是异步行读取的结果。
- 暂无持久化用户设置。
