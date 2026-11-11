# Agent 面板布局、Talk to Agent 与签名配置的系列修正

日期：2026-11-11
涉及平台：HarmonyOS（OHOS）移植层 + 上游共享 crate 的资源与依赖声明

## 问题描述

本轮围绕 Agent 面板的可视层级、Talk to Agent 的未保存文件支持、英文环境下的占位符替换、以及构建脚本的签名配置选择，共处理 9 项需求。其中 4 项是明确缺陷（选中行蓝框语义错误、英文显示占位符字面量、签名固定取第一套配置、space 间距被意外放大），其余是布局与资源调整。

用户原始需求：

1. 工作中的 space 名前的 icon 换成历史记录 space 名前的 icon
2. 历史与归档的 session 列表左对齐到 space 名的左侧
3. 历史区上移，放在 IM 通道下面，间隔一个 label 高度
4. Add Skill 与 IM Channel 同一行，Add Skill 左对齐、IM Channel 右对齐
5. Add Skill 的 icon 换成 slash.svg
6. 历史行的图标槽与 Space 行的文字左对齐（归档同理）
7. space 的最后一个 agent 与分隔线的距离，等于 Add Skill 与下方分隔线的距离
8. 选中行的蓝框只属于键盘光标，鼠标操作不应出现
9. Talk to Agent 支持未保存的新文件，只发选中文本、不发路径与行号

## 问题表现

- 鼠标点击或终端切到某个 agent 后，该行常驻蓝色细边框；面板并没有键盘导航，蓝框语义与实现矛盾
- 历史 session 行比 space 名右移约 36px，图标槽未与 space 文字对齐
- 历史区与底部入口之间的间距是 80px（12 加两个行高），与"一行 label"的要求相差甚远
- 英文界面下 Talk to Agent 的路径区块显示 "Line {start} to Line {end}" 字面量；中文正常
- 未保存的新文件里选中文本后，右键菜单能点开 Talk to Agent，但弹窗不出现（被提前 return 拦掉）
- ./install-local.sh 报 code:9568322 "signature verification failed due to not trusted app source"，因为脚本固定取 signingConfigs[0]，而 build-profile.json5 已改成两套签名、当前选的是 debug
- 检视阶段发现：给每个 space 块加底部留白后，space 之间的间距从 8px 变成 12px（需求只要求最后一个 space 与 footer 对齐）
- 检视阶段发现：Add Skill 的说明弹层原在固定头部，随按钮移入滚动区后，向下展开会被视口底部裁剪

## 问题原因

### 蓝框跟错了对象

row_container 把"服务器当前选中的 agent/终端"（selected）直接映射成 colors.border_focused 边框。selected 由 agent.focused 驱动，鼠标点击与终端切换都会改变它，因此边框一直显示。正确语义是"键盘光标位置"，而面板没有键盘导航，所以边框应恒为透明。

### 历史行缩进叠加

原实现是 row_container(indent = AGENT_INDENT) 加两个独立槽（status_slot 与 history_agent_icon）。space 行的文字起点是 6（左右内边距）加 16（图标槽）加 6（间距）等于 28px；历史行则是 6 加 16（AGENT_INDENT）再加两个槽与间距，标题落到 60px 以上。

### 英文占位符来自索引缺条

带参数的文案走 Registry::format_message，它不解析源码字符串，只查 assets/locales/index.json 的 source_formats（490 条，来自上游 catalog 抽取）。"Line {line}" 与 "Line {start} to Line {end}" 是 HiCodeer 新增的文案，不在索引里；en-US 的 bundle 为 null，没有独立翻译文件，查表落空就原样返回字面量。zh-CN.json 手工带了这两条 formats，所以只有英文异常。全量核对后确认源码里 443 条静态 format_message 字面量中只有 5 条缺失，4 条属于 Talk to Agent，第 5 条 "Unknown {}" 只出现在测试里。

### 签名固定取第一套

bundle-ohos 原先用正则从 build-profile.json5 里取第一个 certpath/profile/storeFile/keyAlias，不看 products 的 signingConfig 指向。文件里加入第二套（正式）签名后，取到的仍是数组第一项，若顺序变化就会拿错证书与口令。

### 首个修复引入的间距回归

需求 7 的修法是给 space 块加 pb_1（4px）。它作用在每一个 space 块上，而块间本来就有 pt_1，于是块间距变成 4 加 4 加 4 等于 12px，只有最后一个块后才是期望的 8px。

### 弹层被滚动容器裁剪

footer（Add Skill 与 IM Channel 那一行）位于 overflow_y_scroll 的内容里。说明弹层是普通流式元素，接在按钮行下方，因此当 footer 已经贴到视口底部时，弹层向下展开的部分会超出可见区域。

## 解决方案

### 面板视觉层级

- space 图标改用 Folder（闭合态），与历史分组默认折叠时显示的图标一致
- 历史 session 行改为单图标槽：缩进常量 HISTORY_ROW_INDENT = 22px（16 槽加 6 间距），槽内平时放 agent 图标、打开中放加载标记；标题因此从 50px 起，图标槽左边缘正好落在 28px，与 space 文字起点重合
- 历史区上间距改为常量 HISTORY_TOP_GAP = 20px，约等于一个 Default label 的行高
- 底部入口合并成一行：Add Skill 靠左、IM Channel 靠右，中间用 flex_1 弹簧撑开；Add Skill 图标用 IconName::Slash，并保留 tab_index 与 focus_visible 的键盘可达性
- footer 增加顶部分隔线（border_t_1 加 colors.border），与每个 space 块顶部的分隔线同款
- 选中行只画 element_selected 背景，焦点边框恒透明但保留 1px 占位，避免选中切换时行盒子位移

### 间距回归

pb_1 改为只在最后一个 space 块上通过 when(is_last_space, ...) 生效。块间间距恢复 4 加 4 等于 8px，最后一个块到 footer 分隔线为 8px，与 Add Skill 到下方分隔线的 8px 一致。

### 弹层位置

说明弹层改到按钮行之前渲染（v_flex 内先 children(popover) 再 child(行)），展开方向变成向上占位，不再越过视口底部。

### 未保存文件

- 编辑器侧去掉"没有 file_path 就 return"的拦截，改用 Option 映射为空字符串继续派发
- compose_message 在 file_path 为空时不拼 File 与 Line 两段，只输出用户输入加选中文本的代码块
- 路径为空但 is_before_change 为真时，仍单独输出 "(before changes)"，避免丢失"改动前快照"这一语义
- render_header 在无路径时返回空元素

### 英文占位符

把 4 条缺失的 source_formats 补进 assets/locales/index.json（490 变为 494），而不是在 Registry 里加运行时兜底。理由：索引是数据层，兜底会让共享 crate 多一条处理路径；补齐后 12 个语言包本就都有对应 formats，非中英文不受影响。crates/localization 的 serde 补 derive feature，修掉该 crate 单独构建/测试时缺 derive 的问题（此前靠 feature unification 侥幸通过）。

### 签名配置

- read-sign-pwd.js 成为唯一的签名材料解析入口：新增 --config / --product / --material 三个参数，--material 输出 certpath、profile、storeFile、keyAlias 与配置名五行
- 解析改为 JSON5 容错：先用状态机去掉行注释、块注释与尾逗号，再 JSON.parse，使脚本不比写文件的 hvigor 更严格；解析失败直接报错，不再静默退回"取第一处文本匹配"（那条兜底会在带注释的文件上取错配置的口令）
- bundle-ohos 删除重复的内联 Node 解析块，改为调用 read-sign-pwd.js --material，产品名由 HVIGOR_PRODUCT 统一（可用 OHOS_PRODUCT 覆盖），hvigor 的 -p product 与签名材料取同一个产品
- 新增 OHOS_SIGN_CONFIG 变量指定配置名；四项材料全部由环境变量提供却没给 OHOS_SIGN_CONFIG 时，若也没给 OHOS_SIGN_PWD 就明确报错，不再猜配置

## 已验证

- cargo test -p agent_panel：28 passed, 0 failed（含新增的未保存文件用例与改动前标记用例）
- cargo check -p agent_panel：通过
- node --check script/read-sign-pwd.js、bash -n script/bundle-ohos：通过
- 实测 JSON5 变体（加注释与尾逗号）能正确解析；未知配置名退出码 1
- bundle-ohos 全量构建按用户要求中途停止，未重新出包；签名段未在完整构建中跑通
- 临时测试目录与脚本已清理

## 未处理的两项（记录取舍）

- agent 行标题在 24px 高的行盒里垂直居中，文字底到分隔线比 Add Skill 侧多约 3px；两侧 padding 已一致（8px）。要完全相等只能改行高或写死补偿值，前者改变列表密度、后者随字体行高失真，故保留现状
- 历史行打开中时，单图标槽显示加载标记而非 agent 图标（原实现是两个槽并存）。用户明确要求"一个图标槽与 Space 文字对齐"，单槽只能二选一，故保留合并

## 修改文件

- crates/agent_panel/src/agent_panel.rs — space 图标改 Folder、历史区上间距、历史行单槽与缩进、footer 合并与分隔线、弹层改到按钮上方、选中态去焦点边框、space 末块留白
- crates/agent_panel/src/talk_to_agent_modal.rs — 无路径时省略 File/Line、保留改动前标记、修正过期注释、新增两个测试
- crates/editor/src/ohos_talk_to_agent.rs — 未保存 buffer 不再提前返回，file_path 允许为空
- crates/localization/Cargo.toml — serde 启用 derive feature
- assets/locales/index.json — 补 4 条 source_formats
- assets/icons/hi_codeer.svg — 改为 16x16 方形画布、等比、修正 viewBox 右侧裁切
- script/bundle-ohos — 按 products 的 signingConfig 取材料、产品名单点、OHOS_SIGN_CONFIG 支持、复用 read-sign-pwd.js
- script/read-sign-pwd.js — JSON5 容错解析、--config/--product/--material、解析失败即报错、usage 同步
