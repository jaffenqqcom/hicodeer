# 终端「跳到底部」悬浮按钮设计

日期：2026-10-25
状态：已实现（编译通过，设备验证通过）
提交：87bc62a

## 1. 目标

在终端里滚动回看历史输出后，提供一个可点击的入口回到实时提示符，行为对齐 Warp 的「Jump to bottom」：

- 只在「已离开底部」时出现，回到最底部自动消失
- 触摸设备上没有 `shift-end`，这是唯一可用的返回方式
- 不改变 macOS / Linux 上的任何现有交互

## 2. 现状与约束

### 2.1 上游已有的部分

| 能力 | 位置 |
| --- | --- |
| `ScrollToBottom` 动作定义 | `crates/terminal/src/terminal.rs:641` |
| `Terminal::scroll_to_bottom`（投递 `Scroll::Bottom` 事件） | `crates/terminal/src/terminal.rs:2159` |
| `TerminalView` 的动作处理 | `crates/terminal_view/src/terminal_view.rs:991` |
| 是否在底部 | `Terminal::scrolled_to_bottom()`，`crates/terminal/src/terminal.rs:2167` |

也就是说：动作、滚动实现、状态查询全都是现成的，缺的只是一个可见、可点的 UI 入口。

### 2.2 关键约束

**（1）`scrolled_to_bottom` 的语义刚好合适**

它的取值来自 `content.display_offset == 0`（`crates/terminal/src/alacritty.rs:956`）：

- 正常滚回看时 `display_offset > 0` → 返回 false → 按钮显示
- 在底部时 `display_offset == 0` → 返回 true → 按钮隐藏
- alt-screen（`vim`、`less` 这类全屏程序）没有回滚区，`display_offset` 恒为 0 → 自动隐藏，不需要额外判断

**（2）状态比 render 晚一帧**

`TerminalView::render` 读的是 `terminal.last_content`，而它由 `Terminal::sync` 在 `TerminalElement::prepaint`（`crates/terminal_view/src/terminal_element.rs:1428`）里重建。同一帧里 render 在 prepaint 之前执行，于是：

- 滚动动作 → `cx.notify()` → 新一帧 render 读到旧状态（仍在底部）→ 按钮不出现
- 随后 prepaint 才把新状态写进 `last_content`
- 如果这一帧之后再没有别的输入事件，界面就停在「该显示却没显示」

单次滚动（按一次键、拖一下滚动条）必然踩到；连续滑动因为事件密集，看起来只是「稍晚出现」。

**（3）触摸设备没有 hover、没有修饰键**

`gpui_ohos` 把触摸 tap 合成为标准的 mouse down/up，事件里没有修饰键。所以按钮必须「单击即用」，不能依赖悬浮展开或组合键。

## 3. 方案

### 3.1 结构与显隐

`TerminalView::render` 最外层已经是 `.size_full().relative()`（`crates/terminal_view/src/terminal_view.rs:1612`），因此按钮直接作为它的绝对定位子元素即可，不需要改动布局层级。

显隐判据（`crates/terminal_view/src/terminal_view.rs:1602`）：

```text
show = mode == Standalone && !terminal.scrolled_to_bottom()
```

- `Standalone` 限定：`Embedded` 模式（Agent panel 里的只读输出）不显示，避免与面板自身的滚动控件重叠
- 不额外判断 alt-screen：见 2.2（1）

按钮本身在 `render_scroll_to_bottom_button`（`crates/terminal_view/src/terminal_view.rs:1470`）里构建：

| 属性 | 取值 | 理由 |
| --- | --- | --- |
| 尺寸 | `40px` 圆形 | 触摸最小热区 |
| 位置 | `bottom: 16px`、`right: 20px` | 让开右侧滚动条（约 12px） |
| 背景 | `element_background` @ 60% | 半透明，不遮死底下的终端文字 |
| 边框 | `border` @ 40% | 与终端深色背景区分 |
| 阴影 | `shadow_sm` | 轻微浮起，不做重投影 |
| 图标 | `IconName::ArrowDown`（`assets/icons/arrow_down.svg`） | 复用既有图标资源，不新增素材 |
| 悬停 | `element_hover` @ 80% | 外接鼠标时给反馈；触摸下不生效 |

### 3.2 点击链路

点击直接复用已有动作处理：

```text
on_click → TerminalView::scroll_to_bottom(&ScrollToBottom) → Terminal::scroll_to_bottom()
         → 事件队列 Scroll::Bottom → 下次 sync 应用 → display_offset = 0
```

没有新写滚动逻辑，因此 vi 模式、鼠标上报模式等既有分支的行为完全一致。

### 3.3 事件拦截

按钮浮在终端元素之上，但终端元素是**兄弟节点**而非父节点，它的 `mouse_down` 监听器同样会被派发。若不处理，点一下按钮会顺带在终端最右下那个格子开始一次选区。

gpui 的鼠标事件在 bubble 阶段按注册顺序倒序派发（`crates/gpui/src/window.rs:5906`），而 click 判定的监听器在 div 自身监听器之后注册（`crates/gpui/src/elements/div.rs:2937`），因此：

- `on_mouse_down(Left, cx.stop_propagation())` —— 先让 click 判定记下按下位置，再截断，终端收不到按下
- `on_click` 内先 `cx.stop_propagation()` —— 终端收不到抬起，不会终结选区

### 3.4 消除一帧延迟

在 `TerminalElement::prepaint` 的 sync 前后各读一次 `scrolled_to_bottom`（`crates/terminal_view/src/terminal_element.rs:1425`、`:1454`），一旦翻转就 `cx.notify(terminal_view_entity_id)` 补一帧。

- 只在变化时触发，稳定后不再产生额外帧
- 代价是一次布尔比较 + 一次 `last_content` 读取，可忽略
- 没有选择「在 render 里先 sync」：那会把 `make_content`（拷贝整屏可见单元格，`crates/terminal/src/alacritty.rs:911`）从每帧一次变成每帧两次

## 4. 取舍

**不用 `IconButton`**：仓库里的 `IconButton` 是方形、带内边距的工具栏样式，触摸热区不足 40px，且无法直接做成圆形半透明浮层。手拼 `div` 只多十几行，样式可控。

**不用 `deferred` / `anchored`**：那是给菜单、tooltip 这类需要脱层叠上下文的浮层用的。按钮是终端视图的固定角标，普通绝对定位就够，也避免了 `with_priority(1)` 带来的层级副作用。

**不读实时 `display_offset`**：滚动意图先进入终端的事件队列（`InternalEvent::Scroll`），要读到最新值必须先处理队列，也就是 `sync`；其成本见 3.4。

**不做出现/消失动画**：滚动本身已经带来视觉变化，再加动画反而干扰；也不引入额外的帧请求。

## 5. 边界与已知限制

- **分屏**：元素 id 用 `("terminal-scroll-to-bottom", terminal.entity_id())`，多个终端同时显示时各自独立，不会串 hover 状态
- **嵌入式终端**：`Embedded` 模式不显示按钮（见 3.1）
- **多指/长按**：沿用 `gpui_ohos` 现有的触摸判定，慢点（>220ms）不算点击，与其它按钮一致
- **按钮遮挡**：停靠在右下角，会盖住最后一行最右侧约两格；这是浮动按钮的固有代价，Warp 同样如此

## 6. 门控

改动全部落在 `crates/terminal_view`，该路径不含 `ohos`，因此按规则用 `#[cfg(target_env = "ohos")]` 门控，并用 `[OHOS PORT BEGIN] / [OHOS PORT END]` 标注起止：

- `terminal_view.rs:1464-1505`：按钮构建函数
- `terminal_view.rs:1602-1609`：显隐判据
- `terminal_view.rs:1701-1710`：挂载到元素树（该处为表达式位置，只能靠注释标注）
- `terminal_element.rs:1420-1426`、`1452-1457`：状态翻转补帧

非 OHOS 构建下这些代码整段不参与编译，行为与上游一致。

## 7. 验证

- `./script/bundle-ohos` 编译通过，HAP 打包与覆盖安装成功
- 设备验证：向上滚动回看后按钮出现，点击回到实时底部并自动隐藏；点击不会在终端里留下选区
- 外观迭代：初版背景不透明、阴影偏重，按反馈改为 60% 透明背景 + `shadow_sm`
