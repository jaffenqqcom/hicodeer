# 终端链接跨软换行无法识别

## 问题描述

终端里的长链接（URL、文件路径）被窗口宽度自动折成两行以上后，只有第一行能识别：hover 第一行会出下划线，hover 第二行及之后完全没有反应。旧实现对折行链接的高亮范围也只画到第一行行尾，点击打开的是被截断的地址。本报告覆盖修复过程中定位到的两类原因：旧实现本身不跨行，以及照抄 warp-ohos cell 级内核时引入的三个移植缺陷。

## 问题表现

- 一条 200 字符的 URL 折成两行后，第一行 hover 有下划线，第二行没有
- 高亮只覆盖第一行，且末尾常粘上一个不属于链接的字符
- 185 字符的绝对文件路径整条不识别，hover 无 tooltip，Ctrl+点击无效
- 单元测试全绿（含 3 列窄终端下的跨行用例），真机行为与测试结论矛盾
- 无崩溃、无报错，纯粹是识别范围问题

## 问题原因

### 一、旧实现按「字符串」工作，跨行数据被裁剪

旧链路是：把 hover 所在的**逻辑行**拼成一个 `String` → 在字符串上跑正则 → 再按字节偏移换算回 grid 坐标。最后一环 `byte_range_to_grid_match` 会把匹配范围裁到「hover 所在的软换行段」：

```
逻辑行:  /test/very/long/path/to/some/cool.rs      (由 3 个物理行拼成)
正则命中: 0..=38   （整条路径）
裁剪后:   0..=19   （只剩第一物理行）
```

所以即使字符串层面识别对了，返回给视图层的 range 也只到行尾，第二行永远不会被点亮。

### 二、照抄 warp cell 级内核时的三个移植缺陷

把实现换成 warp-ohos 的逐 cell 扫描后，又暴露出三个问题，它们只在真实宽度的终端上出现，3 列窄终端的单测覆盖不到：

1. **游标走不出行尾**。alacritty 的行是变长的（`Row::len()` 可小于 `columns`），`cell_type` 对「行内未存储的列」返回 `None`，`advance_point_by_columns` 的推进循环直接在行尾停住，跨不到下一行。warp 的 grid 是定长数组，不存在这个情况。
2. **软换行标记判错**。`row_wraps` 只检查最后一列 `columns-1` 的 `WRAPLINE`；若该行末尾未写满，标记实际落在 `len-1`，判定为「不换行」，跨行随即断开。
3. **推进是 O(候选 × 行长)**。`advance_point_by_columns` 逐格循环，候选数最多 32×32，长行上每次 hover 要跑几十万次函数调用。

### 三、诊断阶段的误判（记录以免重蹈）

真机复现时在 agent 面板看到 URL 折行，一度判断为内核未跨行。用 `hicodeerd agent read` 对比两种快照后发现：

- `--source recent` 与 `--source recent-unwrapped` 输出**完全一致**，而后者本该合并软换行
- 折行处是真实换行，第二行还带 2 个空格缩进

这说明那是 codebuddy TUI 自己按宽度**硬折行**写出来的，不是终端软换行。链接在数据上就是两段，任何终端（含 warp）都连不起来。本地用同样的 80 列宽直接构造 grid 才能复现真问题。

## 解决方案

### 1. 用 cell 级内核替换字符串层

新增 `hyperlinks_detection.rs`，逐项对齐 warp-ohos 的 `grid_handler.rs`：`Viewport`（`Line(i32)` ↔ `usize` 归一化）、`GridCursor`（`Wrap::{None, Soft}`、跳宽字符 spacer）、`url_at_point`、`hyperlink_at_point`、`possible_file_paths_at_point`（`line_to_fragments` + `append_fragments_across_soft_wrap` + `advance_point_by_columns`）。`hyperlinks.rs` 删除整条字符串链路，入口按 OSC 8 → URL → 正则兜底 → 路径的顺序分发。

所有 range 都是真实 grid 坐标，跨行天然成立，不再有裁剪环节。

### 2. 修复 alacritty 变长行导致的越界

```rust
// 行内未存储的列当作空格，而不是“越界”
match row_cell(self.row(point.row)?, point.col) {
    Some(cell) => Some(cell_type(cell)),
    None => Some(CellType::RegularChar),
}
```

### 3. 修正软换行判定

```rust
// 同时查最后一个已存储单元与最后一列
let last_stored = row.len().checked_sub(1).unwrap_or(0);
let last_column = self.columns.saturating_sub(1);
[last_stored, last_column].into_iter().any(|col| {
    row_cell(row, col).is_some_and(|cell| cell.flags.contains(Flags::WRAPLINE))
})
```

### 4. 推进改为常数时间

warp 逐格推进只是因为它在越界时停下，结果等价于线性算术。改为一次算完，代价与行长无关：

```rust
let total = point.row * columns + point.col;
let last = (viewport.total_rows() * columns).saturating_sub(1);
let advanced = if forward { total.saturating_add(count).min(last) } else { total.saturating_sub(count) };
```

与 warp 的唯一差异是：链接一直延伸到屏幕最后一格时，warp 返回越界点、这里停在最后一格，已在注释中标注。

### 5. 尾句读裁剪按列宽而非字符数

```rust
// 原来 removed_characters += 1，宽字符句号（。、）会少移一列
removed_width += UnicodeWidthChar::width(c).unwrap_or(1);
```

### 6. 入口加形态守门

warp 靠视图层 stat 拒绝散文，zed 的内核是同步纯文本、拿不到文件系统，所以 `looks_like_path` 先做形态判断再交给视图层：无空白、首尾不是分隔符、无内部分隔符（`:`、`\`、成对括号例外）、不含 `://`，纯数字放行。代价是含空格的文件名与无扩展名的裸词不再识别，属于已知能力差距。

### 尝试过但无效的方向

- 在 `schedule_find_hyperlink` / `refresh_hovered_word` / `update_selected_word` 加 `PROBE`、`REFRESH`、`HOVER SET` 打印：能确认探测被调度，但不能区分坐标系问题与边界问题
- 用 `HL_NO_PATH` 环境变量关掉路径检测跑 `scroll_long_line_benchmark`：证明该基准超时（>4 分钟）与本次改动无关，是 debug 构建下 40000 次滚动的固有耗时
- 直接删除形态守门（全部候选都返回）：跨行正常，但 hover 到普通文字就会把整行当作目标，多出一次重绘，产品级测试随之失败

## 修改文件

- `crates/terminal/src/alacritty/hyperlinks_detection.rs` — 新增。warp-ohos 的 cell 级检测实现：`Viewport`、`GridCursor`、`url_at_point`、`hyperlink_at_point`、`possible_file_paths_at_point`；修正越界列、软换行标记、常数时间推进、按列宽裁剪句读
- `crates/terminal/src/alacritty/hyperlinks.rs` — 删除字符串层（`HoveredLine`、`collect_hovered_line`、`byte_range_to_grid_match`、`path_candidates_around` 等约 780 行），入口改为 OSC 8 → URL → 正则兜底 → 路径；新增 `looks_like_path` 形态守门；文件头加 `[OHOS PORT]` 标记
- `crates/terminal/src/terminal.rs` — 写输出后的测试期望由 `Wakeups(2)/Notifies(0)` 调整为 `3/1`，并注释说明多出的一次来自内容变化后的 hover 复探（该调整尚未与基线对比，属未验证项）
- `crates/terminal/Cargo.toml` — 新增 `unicode-width` 依赖，用于按列宽裁剪尾句读

[[ohos-debug-lessons]]
