# Agent 重命名：设计

日期：2026-10-29
状态：已实现（提交 `c7e1fe2`）
范围：Agent 面板的 agent 行改名，含 herdr 侧名称规则与持久化恢复

## 1. 背景与目标

Agent 面板里 space 行一直可以改名（`AgentPanel::rename_space`，`crates/agent_panel/src/agent_panel.rs:1021`），agent 行却只能显示服务端生成的名字（`space-kind-tab` 形式，见 `herdr/src/app/agents.rs` 的 `auto_agent_name_with`）。

目标：

- agent 行就地改名，交互与 space 行、历史会话行一致
- 名字跨应用重启、跨 daemon 重启都保留
- 改名只影响这一行，不动同 space 的其他 agent，也不动 pane 标签
- 名字可以是中文、大写开头、带空格——用户写的是给人看的标签

不在本次范围：批量改名、把名字同步进 tab 标题、为 agent 行加右键菜单。

## 2. 现状与约束

- **RPC 已存在**：`agent.rename`（`herdr/src/api/schema.rs:132`）与 CLI `herdr agent rename <target> <name>|--clear`（`herdr/src/cli/agent.rs:758`）都是上游既有能力，本次不新增协议，只改名称规则并接入面板。参数是 `{ target, name? }`：`name` 缺省或 `--clear` 表示清除（`herdr/src/api/schema/agents.rs:46`）。
- **名字只有一个来源**：服务端的 `TerminalState.agent_name`。启动时要么用显式名（校验后存入），要么由 `auto_agent_name_with` 按 space/kind/tab 生成。面板不自己造名字。
- **名字与 pane 标签是两个字段**：`manual_label` 是 pane 边框上的用户标签，`agent_name` 是 agent 行的名字。改名不动 `manual_label`，有回归用例 `agent_rename_does_not_replace_the_pane_label`（`herdr/src/app/api/agents.rs:925`）。
- **名字不能当作「这里有 agent」的证据**：`is_agent_terminal()` 只看观测到的 agent 标签，或一次仍在进行中的托管启动（`herdr/src/terminal/state.rs:2395`）。否则进程退出后残留的名字会让空 pane 永远挂在 `agent.list` 上。
- **名字也可以用来寻址**：`resolve_agent_target` 先按 public pane id 解析，再按 `agent_name` 全等匹配；命中多个返回 `Ambiguous`，不取第一个（`herdr/src/app/terminal_targets.rs:74`、`:121`）。

## 3. 名称模型

### 3.1 语法

`valid_agent_name`（`herdr/src/app/agents.rs:63`）：

- 首字符必须是字母（`is_alphabetic`，任意语种）
- 其后允许任意语种字母、数字、空格、`-`、`_`
- 长度 ≤ 32 **字符**（`MAX_AGENT_NAME_CHARS`，`herdr/src/app/agents.rs:16`），按字符而不是字节计，中文名不会因编码长度被提前截断
- 末字符不能是空格：target 里看不见，等于让用户在 CLI 里猜

被拒的还有 `:`、引号、控制字符。理由：名字可能被当作 target 使用，`:` 是 public id 的分隔符，引号会破坏 CLI 与 JSON 两侧的转义假设。名字里允许空格，代价是从 shell 按名字寻址时要加引号——pane id 不需要，面板始终用 pane id。

规则比上游放宽了两点：原来要求全小写 ASCII，因此 `Reviewer`、`总指挥`、`QA lead` 都不合法。放宽放在服务端一处，CLI 与 API 自动一致；面板只做 trim，不重复实现规则。

### 3.2 归一化

`normalize_agent_name`（`herdr/src/app/agents.rs:86`）把非 ASCII 的空白折叠成 ASCII 空格：中文输入法默认插入的 U+3000、网页粘贴带来的 NBSP（U+00A0）与细空格（U+2009）。它们在行里看起来都是空格，语义上也应当是一个。

ASCII 空白中的 tab 与换行原样保留，交给语法拒绝——它们不是「看起来像空格」，而是会破坏单行编辑与 target 的字符，不该被悄悄修好。

### 3.3 归属：名字跟着 agent 会话，不跟 pane

`agent_name_owner` 记下这个名字属于哪个 agent 标签、哪个会话（`AgentNameOwner`，`herdr/src/terminal/state.rs:145`）。`reconcile_agent_name_owner`（`:2407`）在观测到不同 agent、或同一 agent 换了会话时清掉名字。

这样「名字粘到下一个 agent」不会发生：pane 里前一个 agent 退出、用户又起了一个别的 agent，旧名字不会跟过去。托管启动中的同 kind agent 例外——那是同一次启动的中间态，不能把名字清掉。

### 3.4 冲突：改名不比唯一，启动仍比

改名不再检查重名（原 `agent_name_conflicts` 调用已从 `rename_agent_target` 移除，`herdr/src/app/agents.rs:203`）；`start_agent` 仍检查，重复时返回 `DuplicateName` → `agent_name_taken`（`herdr/src/app/agents.rs:553`）。

- 改名是给人看的行标签：两行同名时状态标记、图标、所属 space 仍然不同，用户不会混淆
- 启动时的显式名是调用方给的寻址标识，一开始就歧义没有意义
- 重名后按名字寻址由 `single_terminal_match` 返回 `Ambiguous` 并列出候选，不会静默挑一个（`herdr/src/app/terminal_targets.rs:106`）

## 4. 面板交互

行渲染在 `render_agent_row`（`crates/agent_panel/src/agent_panel.rs:1349`）。

- **进入编辑**：行悬停时右侧浮出铅笔（`IconName::Pencil`），点击进 `begin_agent_rename`（`:1036`）。标题位置换成单行 `Editor`，内容取自 `client::display_name` 的结果，并全选——编辑的是「用户看到的那个名字」，没改就确认等于无操作。
- **提交/取消**：`menu::Confirm` 与 `menu::Cancel` 分别调 `finish_agent_rename(true/false, ...)`（`:1080`）。保存路径 trim 后为空、或与原名相同，都不发请求：空名会把行变空，服务端也把它当「不变」。
- **失焦保存**：订阅 `EditorEvent::Blurred`，把判定 `cx.defer_in` 到下一帧再做（`:1052`）。点击引起的焦点迁移会在手势中途发生，立刻取消会把「点别处保存」变成「点别处丢弃」。
- **行点击不穿透**：编辑区吞掉 `on_mouse_down` 与 `on_click`（`:1379`），否则点进编辑器会同时 focus 该 agent。
- **成功后立即改行**：`apply_agent_rename`（`:1133`）直接改本地 `Agent` 的 `name`，不等下一次状态轮询——轮询有秒级间隔，等它就出现「名字回跳再变回来」。
- **失败提示**：`agent_rename_failure_message`（`:2650`）按服务端错误码给一句可行动的话，其余透传原始文本：

| 错误码 | 面板文案 |
|---|---|
| `invalid_agent_name` | 名称只能包含字母、数字、空格、`-`、`_` |
| `agent_launch_pending` | 智能体仍在启动中，请稍后再试 |
| `agent_not_found` | 该智能体已不在运行 |
| 其他 / 无内容 | 无法重命名智能体（其后附服务端文本） |

四条新文案已进 `assets/locales/*.json`（13 个语言文件，各 4 条）。

## 5. 会话标题前缀

改名会顺带把该 agent **正在展示的已打开会话**的标题改成 `{新名}:{原标题}`，实现是 `prefix_open_session_title`（`:1160`）与纯函数 `title_with_agent_prefix`（`:2639`）。

- 旧前缀属于旧名字，先 `strip_prefix("{旧名}:")` 再拼新的，所以连续改两次仍只有一个前缀
- 当前标题的取值顺序：本地标题覆盖（`SessionService::title_override_of`，`crates/agent_panel/src/session_service.rs:131`）> 扫描到的标题。都没有就放弃——冒号后面没东西可写
- 写入走既有的会话改名通道（`SessionService::rename`），与历史面板里手动改标题是同一份状态，写盘由 SessionService 负责
- 只在该 agent 报告了 `agent_session` 时生效；纯 shell 或被恢复成 shell 的 pane 没有会话可改

这个联动是取舍：agent 名往往就是用户对这一路的称呼，历史行跟着改更连贯；代价是用户手动给该会话起的标题会被覆盖一次（见第 8 节）。

## 6. 服务端处理

`rename_agent_target`（`herdr/src/app/agents.rs:203`）：

1. `resolve_agent_target(target)`：public pane id 优先，其次按名字全等匹配，0 或 >1 都报错
2. `normalize_agent_name` + `valid_agent_name`，失败 → `invalid_agent_name`
3. 按 `terminal_id` 找 terminal，找不到 → target 错误
4. `managed_agent_launch_pending()` → `agent_launch_pending`：启动完成前身份还没落定，改名会被随后的检测覆盖
5. `effective_agent_label().is_none()` → `agent_not_found`：这里没有 agent，改名没有对象
6. `set_agent_name(name)` 或 `clear_agent_name()`
7. `mark_session_dirty()` + `schedule_session_save()`：名字随会话快照落盘
8. `emit_pane_updated(...)`，返回改名后的 `AgentInfo`

路由层在 `handle_agent_rename`（`herdr/src/app/api/agents.rs:64`），错误体由 `agent_rename_error_body`（`herdr/src/app/agents.rs:478`）生成。

## 7. 持久化与恢复

- 快照字段 `PaneSnapshot.agent_name`（`herdr/src/persist/snapshot.rs:105`），写入时跳过启动未完成的 pane（`:341`）
- 冷恢复（`herdr/src/persist/restore.rs:606`）与热恢复（`:712`）的分支一致：
  - 名字 + managed agent → `restore_managed_agent`，名字与启动身份一起还原
  - 名字有、managed 无、但快照带 agent 会话 → `set_agent_name`（本次新增）：pane 回来时只是 shell，名字先留着，恢复该会话后仍然对
  - 名字有、会话也没有 → 丢弃（新增的判据）：孤名没有归属，留下只会粘给下一个 agent
  - 热恢复路径上 managed 的分支也改为保留名字（原来直接丢掉）
- 聚合层（`herdr/src/workspace/aggregate.rs:39`）用 `agent_kind_label.is_none() && !managed_agent_launch_pending()` 决定是否把 pane 列进 `PaneDetail`，名字不再参与判定

净效果：重启后名字还在；但如果 pane 只剩 shell、没有可恢复的会话，它不会因为「有名字」而出现在 agent 列表里。名字与「这里有 agent」彻底解耦。

## 8. 取舍

1. **去掉改名的唯一性检查，保留启动的**。改名的目的是可读，启动名是调用方给的寻址标识；两者对唯一性的要求本就不同。寻址歧义已有 `Ambiguous` 兜底。
2. **名字跟会话走，不跟 pane 走**。pane 是可复用容器，名字描述的是里面那一路工作；换 agent 就换名字。为此新增了 `agent_name_owner` 校正与恢复时「无会话即丢弃」的判据。
3. **名字不做 agent 存在的证据**。否则残留名字会让空 pane 永久占位，且与「进程已退出」的状态矛盾。
4. **面板本地立即更新**。轮询间隔内的回跳比一次本地写入更糟；服务端返回值与本地写入一致，下一次轮询不会打架。
5. **前缀替换而非叠加**。连续改名不该积累 `a:b:c:`。
6. **规则只写在服务端**。CLI 与 API 同一条 `valid_agent_name`；面板若也校验一遍，两边迟早不一致（空名、纯空格、超长都交给服务端回答）。
7. **不新增 RPC**。上游的 `agent.rename` 语义正好是「一行 agent 的名字」，没必要为 UI 再造一个接口。

## 9. 门控与平台

全平台通用，没有 `cfg(target_env = "ohos")` 门控：

- 改动落在 herdr（`herdr/src/app/agents.rs`、`terminal/state.rs`、`persist/restore.rs`、`workspace/aggregate.rs`）与 agent panel（`crates/agent_panel/src/*`）
- 不涉及 ArkTS 插件、NAPI 或 OHOS 平台能力
- 与「space 改名」共用同一套 UI 形态，行为在桌面平台同样成立

## 10. 测试与验证

自动化用例：

- `herdr/src/app/agents.rs:716` `a_wide_space_is_read_as_a_space`：U+3000、NBSP、细空格折叠，tab 保留
- `herdr/src/app/agents.rs:728` `agent_names_accept_letters_in_any_script`：中文、大写、空格、32 字符边界；拒绝前导空格、尾随空格、tab、数字开头、`-` 开头、`:`、33 字符
- `herdr/src/app/api/agents.rs:925` `agent_rename_does_not_replace_the_pane_label`
- `herdr/src/persist/restore.rs:1707` `cold_restore_with_gapped_public_tab_numbers_keeps_unmanaged_agent_name`
- `herdr/src/persist/restore.rs:1539` `restore_carries_persisted_agent_session_metadata`（断言改成名字随会话保留）
- `crates/agent_panel/src/agent_panel.rs:2928` `a_rename_replaces_the_previous_prefix`
- `crates/agent_panel/src/session_service.rs` `title_override_reads_back_what_a_rename_wrote`

手测项（本轮未复跑，交付前按需执行）：

1. 改一个 agent 的名字为中文 → 行名立即变，space 内其他行不变，pane 边框标签不变
2. 改成 33 个字符或含 `:` → toast 提示字符集/长度，行名保持原样
3. 连续改两次 → 会话标题只有一个新前缀
4. 杀掉应用重开 → 名字仍在；daemon 重启后再开一次同样在
5. 把该 agent 退出、pane 变回 shell → 名字仍显示，但该行从 agent 列表消失
6. 同一 space 内两行改成同名 → 都能改名；按名字操作时报歧义而不是随便挑一个

## 11. 关键位置索引

| 位置 | 作用 |
|---|---|
| `crates/agent_panel/src/agent_panel.rs:1036` | 进入行内编辑 |
| `crates/agent_panel/src/agent_panel.rs:1080` | 提交/取消 |
| `crates/agent_panel/src/agent_panel.rs:1102` | 发 `agent.rename` |
| `crates/agent_panel/src/agent_panel.rs:1133` | 成功后本地更新行名 |
| `crates/agent_panel/src/agent_panel.rs:1160` | 会话标题前缀 |
| `crates/agent_panel/src/agent_panel.rs:1349` | agent 行渲染 |
| `crates/agent_panel/src/agent_panel.rs:2639` | 前缀替换纯函数 |
| `crates/agent_panel/src/agent_panel.rs:2650` | 错误码→文案 |
| `crates/agent_panel/src/client.rs:97` | `rename_agent` RPC 调用 |
| `crates/agent_panel/src/session_service.rs:131` | 读回会话标题覆盖 |
| `herdr/src/app/agents.rs:16` | 名字长度上限 |
| `herdr/src/app/agents.rs:63` | 名字语法 |
| `herdr/src/app/agents.rs:86` | 宽空格归一 |
| `herdr/src/app/agents.rs:203` | 服务端改名流程 |
| `herdr/src/app/agents.rs:478` | 错误体 |
| `herdr/src/app/api/agents.rs:64` | RPC 入口 |
| `herdr/src/terminal/state.rs:2156` | 写入名字并记归属 |
| `herdr/src/terminal/state.rs:2395` | 名字不算 agent 证据 |
| `herdr/src/terminal/state.rs:2407` | 归属校正 |
| `herdr/src/persist/snapshot.rs:105` | 快照字段 |
| `herdr/src/persist/restore.rs:606` | 冷恢复分支 |
| `herdr/src/persist/restore.rs:712` | 热恢复分支 |
| `herdr/src/workspace/aggregate.rs:39` | 列表判定 |

## 12. 已知限制

- 重名允许之后，按名字调用 CLI 会得到 `Ambiguous` 并需要改用 pane id；面板不受影响（始终发 pane id）
- 名字里的空格在 shell 里需要引号，这是放宽语法的直接代价
- agent 改名会重写该 agent 当前会话的历史标题一次，用户此前手动改过的标题会被新前缀覆盖（第 5 节的取舍）
- `display_name` 只剥 `<workspace>-` 前缀用于显示；用户若把名字起成恰好以该前缀开头，显示时会被剥掉，但存储值不变
- 名字长度按字符计，但面板行宽固定，超长名字只靠截断 + tooltip，不换行
