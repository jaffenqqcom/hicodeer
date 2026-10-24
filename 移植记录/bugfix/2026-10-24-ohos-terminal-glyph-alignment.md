# 终端 fallback 符号与首字符宽高不对齐

## 问题描述

在终端里显示 `✹`(U+27B9)、`✺`(U+27BA)、`✶`(U+27B6) 这三个 Dingbats 符号时，字形宽度和垂直位置与终端网格对不齐；运行 `claude` 后其输入框里的第一个字符底部也会被抬高约 1/5 字高，从第二个字符起恢复正常。普通文字显示正常。

问题只在新字体链（HiCodeer 的 OHOS 字体栈）下暴露；上游 Zed 的桌面端同样存在渲染缺陷，但桌面字体链很少把这类符号交给非等宽的符号字体，所以在那边看不出来。

## 问题表现

- `✹`/`✺` 字形比格子宽，墨迹直接盖住右邻字符，横向错位
- `✶` 字形本身在格内偏右，即使不溢出也显得与前后字符间距不均
- 含这三个符号的行整体向上偏移（约 2~3px@16px），与上下行不对齐
- `claude` 输入框的首字符单独成批，底部被抬高约 1/5 字高，字号不变
- 纯主字体文本不受影响

## 问题原因

因果链分三层：

### 1. 网格层：判定正确

alacritty 用 `unicode-width 0.2.2` 判定列宽，`U+27B6..27BA` 属 Narrow，网格按 1 格排版——这一层没有问题。

### 2. 字体层：落到非等宽符号字体

主字体 Lilex 与回退主选 HarmonyOS Sans SC 的 cmap 都不含这三个码点（逐字体解析 cmap/hmtx 确认）。gpui_ohos 的 `layout_line` 把缺字交给 cosmic-text 内建 fallback，命中 `/system/fonts/NotoSansSymbols2-Regular.ttf`：

- advance：`✶` 1.43 格、`✹` 1.54 格、`✺` 1.51 格
- ascent/descent：NSS2 约 1.069 / 0.63 em，主字体约 1.025 / 0.275 em

### 3. 渲染层：只钉起点，不裁剪不缩放

`paint_line` 用 force_width 把后续字形的起点钉回网格，但**不对字形位图做任何裁剪或缩放**，溢出的墨迹原样画出；基线又取整行的 `max_ascent`，于是含 NSS2 字形的行被更高的 ascent/descent 撑偏。首字符单独成批时同理：该批的 `max_ascent` 只来自它自己，基线被顶高。

```
网格:   [ a ][  ✹   ][ b ]      <- ✹ 只占 1 格
墨迹:   [ a ][✹✹✹✹][ b ]       <- 墨迹 1.5 格，盖住 b
基线:   普通行 descend=0.275em
        含 ✹ 行 descend=0.63em  -> baseline 上移
```

## 解决方案

核心思路：**终端知道每个字符占几格、也知道主字体的基线在哪，把这些网格语义传给通用绘制层**，绘制层据此把 fallback 字形在格内居中、把整行基线钉回主字体度量。不做压缩、不做放大。

### 终端侧

1. `BatchedTextRun` 增加 `char_cell_widths: Vec<u8>`，拼批时按 `Cell::is_wide()` 记 1 或 2，零宽装饰字符记 0（继承 base 字符）
2. 新增 `glyph_width_budgets`：先扫 run，**整行都是主字体就返回 `None`（零开销）**；否则为每个字符预解析格宽（零宽字符继承前一个 base），再对每个字形用 `partition_point` 二分定位所属字符——不依赖字形顺序，RTL 也正确；`is_emoji` 的字形跳过
3. `BatchedTextRun::paint` 每帧只做一次主字体探针：用主字体 shape 一个 `"M"`，取其 cosmic `ascent`/`descent`，作为全帧所有行的基线基准
4. 通过 `LineLayout::paint` 的两个新可选参数下发「每字形格宽」与「基线度量」；其余调用点（编辑器、面板、`repl`）一律传 `None`，行为不变

### 通用绘制层（gpui）

- `paint_line`：基线改用 `baseline_metrics.unwrap_or((layout.ascent, layout.descent))`
- `paint_glyph` / `paint_emoji`：收到格宽时，把字形墨迹在格内水平居中（`(cell - ink) / 2`，允许对称溢出）；无格宽时行为与上游完全一致
- `ShapedLine` 增加 `layout()` 与 `decoration_runs()` 只读访问器，供终端拿裸布局

### 关键取舍与试错（未来勿重蹈）

| 尝试 | 结果 | 结论 |
| --- | --- | --- |
| 对所有字形按格宽压缩 | 单位错（`raster_bounds` 是设备像素、预算是逻辑像素），2x 屏上所有字被压窄一半，观感「字体全变了」 | 预算必须乘 `scale_factor` 再与墨迹比较 |
| 压缩 + 10%/2% 容差 | 普通字正常，但符号被压得偏瘦；`✶` 仍偏右（它墨迹未超格，压缩不触发） | 压缩只解决大幅超格；偏右必须靠居中 |
| 基线直接用 `FontMetrics`（不含行距） | 光标抬高、文字底部被裁 | cosmic 的 `max_ascent` ≠ 字体原始 metric，必须用探针取同源值 |
| 墨迹左偏移按压缩比缩放 | `✶` 反而更偏右 | 改为格内居中 |
| 最终：**不压缩、只居中** | 用户确认「完美」 | 符号保持原始粗细，与 warp 按格钉位的观感一致 |

### 性能

- 纯主字体行：扫一遍 run 即返回 `None`，零分配、零逐字处理
- 探针：每帧一次 `shape_line("M")`（缓存命中）
- 预算向量：只对含 fallback 字形的行分配一次，O(n) 构建、O(log n) 每字形查找
- 逐字额外运算（居中偏移）相比 `raster_bounds`/atlas/quad 提交可忽略

## 修改文件

- `crates/gpui/src/text_system/line.rs` — `paint_line` 新增 `glyph_width_budgets`、`baseline_metrics` 两个可选参数并用于基线；`LineLayout::paint` 透传；`ShapedLine` 增加 `layout()`/`decoration_runs()` 访问器
- `crates/gpui/src/window.rs` — `paint_glyph`/`paint_emoji` 新增 `max_width` 参数，按格宽把墨迹在格内居中（逻辑像素乘 `scale_factor` 换算为设备像素）
- `crates/terminal_view/src/terminal_element.rs` — `BatchedTextRun` 记录每字符格数；新增 `glyph_width_budgets`；paint 每帧一次主字体探针并把格宽与基线下发给 `LineLayout::paint`
- `crates/terminal/src/alacritty.rs` — 新增 `Cell::is_wide()`，读取 `WIDE_CHAR` 标志用于格数判定
- `crates/repl/src/outputs/plain.rs` — 适配 `BatchedTextRun::paint` 新签名，传 `None`（repl 输出区暂不启用居中）

门控方式：终端侧行为用 `#[cfg(target_env = "ohos")]` 包裹；gpui 共享函数无法按平台拆分签名，用 `// [OHOS PORT BEGIN]` / `// [OHOS PORT END]` 标注起止点。

## 遗留与注意

- 未保留自动化测试（项目约定不跑 `cargo test`，验证走 `./script/bundle-ohos` 实机观察）
- `repl` 的输出渲染传 `None`，同样的问题在那边仍存在；如需修复，把探针与预算一并传入即可
- 构建期两个环境坑：`hnpcli` 写完 HNP 立即被校验脚本读取时的竞态（表现为 `end of central directory not found`，补跑 `--hap-only` 即可）；并行构建撞车会导致 `bundleName` 与签名配置不匹配的瞬时误报

参考：[[ohos-debug-lessons]]
