# Agent 历史会话扫描：实现文档

日期：2026-10-28
状态：已实施并通过单测 / 端到端验证
上游设计：同目录 `2026-10-24-agent-history-panel-design.md`（面板与接口层整体设计）。本文只讲**采集层**——会话是怎么被扫出来的、代价多大、哪些能删。

## 1. 范围

`agent.sessions` 这个 RPC 的返回值来自各 agent CLI 自己的会话存储。这些存储格式互不相同：Claude 系是 `projects/<编码后的 cwd>/<session>.jsonl`，OpenCode 系是 SQLite，Codex 是 `rollout-<timestamp>-<uuid>.jsonl`，MiniMax 的 `~/.minimax/v2/sqlite/runtime-state.sqlite`，OpenDesk 是 `tasks/task_list.json` 索引加每任务一份 JSON。

本文覆盖：

- 用哪个 connector 读哪个存储，以及为什么只注册这些
- 一次 `agent.sessions` 要花多少 I/O，缓存怎么避免重复解析
- 归一化：会话 id、标题、时间、工作目录怎么定
- 删除能力从哪来，为什么由 daemon 执行
- 测试与已知限制

不在本文范围内：面板的展示与交互（见上游设计）、会话恢复（`herdr/src/agent_resume.rs`，那是另一条链路：把 agent 自己的 `--resume <id>` 拼出来）。

## 2. 分层

```
agent 会话存储（各家目录/SQLite）
  ↓  Connector trait（FAD 定义的统一形状）
fad_stores()   —— FAD 自带的 10 个 connector，按名字手写构造
connector_registry() —— 上面 10 个 + 8 个自研，统一成 RegisteredStore
  ↓  collect()（herdr/src/sessions/fad.rs）
签名缓存：每个 store 一份「上次答案 + 文件签名」
  ↓  normalize()：统一成 CollectedSession
collect_sessions()（herdr/src/sessions/mod.rs）：去重、匹配 space、排序
  ↓  RawSessionDto
agent.sessions（herdr/src/app/agents.rs 的 handle_agent_sessions）
  ↓
agent panel 的会话列表
```

关键分工：**connector 只负责「读出这个存储里的会话」，不关心面板怎么用**；归一、去重、匹配 space 全在 `sessions/mod.rs`；面板只认 `RawSessionDto`。

## 3. Connector 注册表

### 3.1 FAD 的 10 个：手写白名单，不用工厂函数

FAD（franken-agent-detection）自带 `get_connector_factories()`，但它把**每个** connector 的函数指针都放进去，链接器无法丢弃任何一个。所以 `fad_stores()` 按名字手写构造这 10 个：claude、codex、gemini、hermes、kimi、openclaw、opencode、pi_agent、pi_durable、qwen。

两个后果，都是想要的：

- 没被引用的 connector 会被 `--gc-sections` 丢掉，二进制不会因为 FAD 升级而膨胀。
- FAD 新增 connector 不会自动开始扫描——这台设备上根本没有那个 CLI，扫了只是白花 I/O。

**Aider 是促成这个决定的案例**：FAD 的 Aider connector 在找不到自己的根目录时会退回**进程的工作目录**，而 daemon 的工作目录是 `/`，于是每次扫描都把整个根文件系统走了一遍（实测 14 秒），只为一个没装的工具。手写白名单之后它不在列表里，问题消失。

### 3.2 自研的 8 个：FAD 覆盖不到的存储

`connector_registry()` 在 10 个之上追加：

- `codebuddy`：存储与 Claude 同构但目录编码不同（Claude 连开头 `/` 也替换成 `-`）。
- `deepseek`：DeepSeek Harness 自己的目录。
- `codewhale`、`atomcode`：各自一份会话存储，FAD 没有。
- `deveco`、`zcode`：两者都是 OpenCode 形状的 SQLite，但路径与 FAD 的 OpenCode connector 找的位置不同，所以用同一个 `OpencodeFamilyConnector` 传不同 family 参数，注册成两个 slug。
- `minimax`：会话在它的 runtime-state 数据库里。
- `opendesk`：`tasks/task_list.json` 索引加每任务一份 JSON；只读索引（id / 名称 / 时间都在里面），不读整份 transcript。

自研 connector 与 FAD 的实现同一个 trait，因此共用同一条扫描、缓存、归一化路径——`collect()` 里没有任何 `if slug == ...` 的分支。

三处 SQLite（deveco / zcode / minimax）都**不链 SQLite 驱动**，而是调系统的 `sqlite3` CLI 做一次只读查询：每次请求只为读几十行，不值得把这个进程再拖上一个数据库驱动（deveco/zcode 共用 `OpencodeFamilyConnector`，函数文档在 `deveco.rs`；minimax 的查询直接读列式字段，不解析 JSON 列）。

### 3.3 agent label 必须与 pane 侧一致

`agent_label()` 把 FAD 的 slug 映射成 `agent.list` 报的那个 label（例如 `pi_agent`、`pi_durable` 都映射到 `pi`）。面板用「agent + session id」这一对判断某条历史是否已经打开，label 一旦不一致，就会出现在跑的会话同时又留在历史列表里。

### 3.4 四个 store 的额外知识：`AdjustedConnector`

FAD 的 connector 只做通用解析，有几个存储的关键信息它不碰。这些知识**不能**下沉到 `normalize()`（那会变成按 slug 分支的公共路径），也不能改 FAD，所以挂在 connector 外面：`with_store_adjustments()` 给指定 slug 套一层 `AdjustedConnector`，`scan()` 之后原地改这批会话。目前四个：

- `kimi`：transcript 在 `sessions/<cwd-hash>/<session>/agents/main/wire.jsonl`，目录名是单向哈希，只能读同级 `state.json` 拿回 `cwd`；否则面板把整批会话堆在一个不知道的目录下。
- `claude`：子代理的 transcript 落在 `<session-id>/subagents/agent-*.jsonl`，它们带着父会话的 id，列出来就是重复行、点开还会恢复成父会话，所以只保留 projects 根下一层的文件。
- `codex`：文件名是 `rollout-<timestamp>-<uuid>.jsonl`，而 `codex resume` 只认裸 uuid，FAD 的 metadata 里又只有 `source`，只能从文件名里取。
- `gemini`：transcript 在 `~/.gemini/tmp/<slug>/chats/`，`<slug>` 是 `~/.gemini/projects.json` 给启动目录起的名字。FAD 直接把存储目录当 workspace，面板就会把这些会话归到 `~/.gemini/tmp/tmp` 下、并从那里恢复；反查 projects.json 才能拿回用户真正工作的目录（`gemini --resume <id>` 也只在当前 project 的 chats 里找 id）。

每个 adjust 都是独立的 `*_store.rs`，带自己的单测；注册表只声明「哪个 slug 有一步要补」，与「哪个 store 有删除函数」是同一种写法。

## 4. 缓存：签名 + 上次答案

每个 connector 每次调用都会重读自己的整个存储，这在鸿蒙设备上不便宜。所以 `fad.rs` 顶部有一个 `static CACHE: Mutex<Option<HashMap<&'static str, CachedScan>>>`，每个 slug 存一份：

- `signature`：把所有会被读到的文件的 `(路径, mtime, 大小)` 排序后整体哈希（`source_signature`）。存储为空时签名是 0，重查几乎零成本。
- `sessions`：上次扫描的结果。

命中条件只有一个：签名相同。命中就直接复用上次结果，不做解析。

一次 `agent.sessions` 的代价因此是「每个会话文件一次 `stat`」，而不是「重新 parse 所有会话」。真正解析只发生在：首次请求、某个文件新增/改动、或某条会话被删除。

`collect()` 给每个 connector 单独造 `ScanContext`，scratch 目录是 `temp_dir()/herdr-fad-scan/<slug>`。FAD 会往这个目录写自己的 bookkeeping，所有 connector 共用一个会互相踩。

### 4.1 扫描失败不清空列表

某个 store 这次扫描报错（目录权限、数据库被锁）时，代码保留它上一次的答案并继续：

```rust
Err(error) => {
    tracing::warn!("agent sessions: {slug} store scan failed: {error}");
    if let Some(cached) = previous.get(slug) { ... 沿用旧结果 ... }
}
```

理由是面板把空列表画成「这个 agent 没有历史」，用户会以为会话丢了；沿用旧结果至少不会更差。这时会打一条 warn，属于异常路径，正常路径没有日志。

## 5. 归一化

`normalize()` 把 `NormalizedConversation`（FAD 的形状）变成 `CollectedSession`：

- **session_id**：优先取会话自己记录的 id；FAD 的 `external_id` 只在不是路径时才用（Claude 的 `external_id` 是相对 projects 根的路径）；最后退回文件名。选错会让恢复命令拼出无效参数——`codex` 曾经把整串 rollout 文件名当 id，就是这么发现的。
- **title**：沿用 connector 给的；面板层再按「本地改名 > 原始标题 > 未命名会话」兜底。
- **updated_at**：`ended_at` → `started_at` → 文件 mtime，逐级回退。
- **workspace_root**：connector 填的绝对路径；面板用它把会话分到对应 space。
- **deletable**：来自该 store 有没有注册删除函数（FAD 侧还要再过一层 slug 白名单，见第 6 节）。

`collect_sessions()` 再做三件事：

1. **去重**：按 `(agent, session_id)` 保留 `updated_at` 最新的一条。FAD 把 Pi 注册了两次（两种存储），不去重会出现重复行。
2. **匹配 space**：`match_workspace()` 拿 `workspace_root` 与各 space 的 `identity_cwd` 比，命中就填 `workspace_id`，面板据此分组。要求**路径完全相等**，不是前缀——早先用前缀匹配时，开在家目录的 space 把所有项目都认领了，面板只剩一个以家目录命名的组。比较前先 `canonicalize` 存在的部分；精确匹配落空后再做一次大小写折叠（本机存储是 hmdfs，`/repo/Demo` 与 `/repo/demo` 是同一目录，但 `canonicalize` 原样返回传入的拼写），折叠只是回退，大小写敏感的文件系统仍以精确拼写优先。
3. **排序**：整体按 `updated_at` 降序。面板会重新分组与排序，这里保持稳定只是让缓存行为可预期。

## 6. 删除能力

`RegisteredStore` 里 `delete` 是 `Option<fn(&Path) -> Result<(), String>>`：

- 有 → 面板显示删除按钮，`deletable = true`。
- 没有 → `deletable = false`，面板不显示按钮，RPC 也会明确拒绝而不是假装成功。

自研的 8 个都直接给了删除函数。FAD 那 10 个不是「一律可删」，而是另有一张 `DELETABLE_FAD_SLUGS` 白名单，只列了 claude / codex / gemini / kimi / pi_agent / qwen 六个。原因是 FAD 没有暴露「这个 connector 的一条会话是不是恰好一个文件」这种能力，只能由人逐个判定，用路径去猜会让不认识的存储意外变成可删：

- `pi_durable` 被**刻意排除**：它每条会话上报的 `source_path` 都是同一个 `main.jsonl`，通用 unlink 会把整个存储一起删掉；要删单条得走它自己的重写逻辑。
- `opencode` 与 `openclaw` 的会话在数据库里，各自有删除子命令（`opencode session delete`、`openclaw sessions delete`）——那要执行 CLI 而不是 unlink 路径，当前这张表表达不了。
- `hermes` 干脆没有删除命令，库上还有 `BEFORE DELETE` 触发器拒删带网关历史的会话。

为什么由 daemon 删而不是应用自己删：应用进程（uid 20020220）对用户目录下的文件 `unlink` 会拿到 `EPERM`，daemon 以用户身份跑，可以删。所以 `agent.session.delete` 把 `(agent, source_path)` 交给 daemon，由 `sessions::delete_session` 路由到该 agent 的 store 函数。

两道护栏，区别在于「谁知道存储布局」：

- `Guard::StoreOwned`（自研 connector）：store 在自己的删除函数里 `canonicalize` 后校验路径以存储根开头，越界直接拒绝（如 `codebuddy::delete_in`）。
- `Guard::DetectedRoots`（FAD connector）：注册表先调 `ensure_inside_store`，用 connector 自己 `detect().root_paths` 判边界，再交给 `delete_transcript`。

`delete_transcript` 只接受扩展名为 `jsonl` / `json` 且确实存在的文件，不做别的清理——FAD 的 connector 里一个会话就是一份 transcript，删掉它即为删掉该会话。

另一个路由细节：**一个 label 可能对应多个 store**（Pi 注册了 `pi_agent` 与 `pi_durable` 两次，Copilot 有 CLI 与编辑器两个存储）。`delete_session(agent, path)` 按 label 找出所有 store，依次交给它们尝试，第一个认领该路径的完成删除；都不认领时返回最后一个拒绝原因，而不是报成功。

## 7. 调用时机与线程

`handle_agent_sessions` 在**每次请求时同步扫描**，跑在 daemon 的请求处理线程上，没有后台定时器：

```rust
let sessions = crate::sessions::collect_sessions(&workspace_roots);
```

这样做的依据是第 4 节的缓存：除非有会话文件变化，一次请求只是对存储做一遍 `stat`。代价是**启动后的第一次请求**要把所有 store 解析一遍，会明显慢于后续请求。

没有改成后台定时刷新，是因为面板只在打开历史区时才请求，定时刷新在没人看的时候白花 I/O。

## 8. 测试

### 8.1 单测（每个 store 一份）

`herdr/src/sessions/` 下 13 个文件每个都带 `mod tests`（`atomcode` / `claude_store` / `codebuddy` / `codewhale` / `codex_store` / `deepseek` / `deveco` / `fad` / `gemini_store` / `kimi_store` / `minimax` / `mod` / `opendesk`），造临时目录写样本文件，断言解析出的 id / 标题 / 时间 / 工作目录；`*_store.rs` 另外断言它补哪一项（Kimi 的 cwd、Gemini 的 projects 反查等）。有删除函数的 store 另有两条对应用例：删掉后文件确实消失、越界路径被拒（`delete_in_removes_a_transcript_inside_the_store` / `delete_in_refuses_a_path_outside_the_store`）；`delete_transcript` 的扩展名与存在性检查在 `fad.rs` 单独测。

`fad.rs` 里另有针对注册表与归一化的测试：每个被扫描的 agent 都必须在恢复表里有对应项（`every_scanned_agent_has_a_resume_plan`），防止「能扫出来但点了恢复就失败」。

### 8.2 端到端（`agent-tests/e2e/test_scan.py`，只读）

断言四组：

- 结果类型是 `agent_sessions`；期望名单里每个 agent 都有 ≥ `--min-sessions` 条（默认 1，名单默认 17 个）。
- 每条会话的 `agent` / `session_id` / `source_path` 三件套非空——面板缺任一件就不渲染这一行。
- 每条会话的 `deletable` 是布尔值（只查字段有没有从 store 一路传到 DTO，不判断该不该显示按钮）。
- `(agent, session_id)` 全局唯一——daemon 就是按这一对去重的，出现重复说明去重退化。

缺 `workspace_root` 只告警：这类行仍出现在结果里，面板归进「未知目录」组、恢复时省略 `cwd`，属面板行为，API 侧观测不到。

删除本身是另一条需要显式授权的用例，在 `agent-tests/e2e/test_panel_ops.py`：必须同时给 `--allow-delete` 与 `--create-cmd`（先用该命令造一条本轮新建的会话，再只删这一条），两重护栏缺一即跳过。它同时断言这一行的 `deletable = true`——为 false 时面板根本不显示删除按钮，删不掉是设计使然而非故障。

## 9. 已知限制

- **启动后首次扫描慢**：所有 store 都要解析一遍。会话多的机器上第一次打开历史区会有可感知的停顿。
- **同步执行**：扫描在请求线程上跑，一个特别大的存储会拖慢该次请求。目前靠白名单（第 3.1 节）把会乱走目录的 connector 挡在外面，没有做超时或线程池。
- **FAD 覆盖不到的还有**：Trae、Cline、Kilo 等仍无 connector（其中 Trae/Cline 在鸿蒙上没有可用的 CLI，属于无意义）。新增 agent 需要照第 3.2 节的形状写一个 connector 并注册。
- **SQLite 系 feature 未开启**：`herdr/Cargo.toml` 里 FAD 的 feature 只开了 `connectors` / `codex-zstd` / `hermes` / `opencode` / `openclaw-sqlite` / `pi-durable` / `pi-sqlite`。cursor / devin / goose / crush / copilot-vscdb 这些会额外拉 `fsqlite` + `asupersync`，在 OHOS 上尚未验证，所以没有打开；deveco / zcode 的库虽然也是 SQLite 形状，读的是自研 connector（第 3.2 节）。
- **`source_path` 非 UTF-8 时降级**：条目仍可打开，但删不掉（路径为空），扫描时会打一条 warn。
- **FAD 侧有一半会话不可删**：`DELETABLE_FAD_SLUGS` 只覆盖 6 个 slug，`pi_durable` / `opencode` / `openclaw` / `hermes` 的行 `deletable = false`，面板不显示按钮。要补上得把「执行 agent 自己的删除子命令」做成一种新的删除形态，不是往白名单里加名字就行。

## 10. 关键位置索引

- 注册表与缓存：`herdr/src/sessions/fad.rs` 的 `fad_stores` / `connector_registry` / `collect` / `source_signature`
- FAD 输出的后处理：同文件 `with_store_adjustments` + `AdjustedConnector`，实现在 `kimi_store.rs` / `claude_store.rs` / `codex_store.rs` / `gemini_store.rs` 的 `adjust`
- 归一与去重：`herdr/src/sessions/mod.rs` 的 `collect_sessions` / `match_workspace`
- 自研 connector：同目录 `codebuddy.rs` / `deepseek.rs` / `codewhale.rs` / `atomcode.rs` / `deveco.rs`（含 zcode）/ `minimax.rs` / `opendesk.rs`
- RPC：`herdr/src/app/agents.rs` 的 `handle_agent_sessions` / `handle_agent_session_delete`
- 删除护栏：`fad.rs` 的 `DELETABLE_FAD_SLUGS` / `ensure_inside_store` / `delete_transcript`
- 端到端：`agent-tests/e2e/test_scan.py`（扫描，只读）与 `test_panel_ops.py`（删除用例，需 `--allow-delete` + `--create-cmd`），说明在同目录 `README.md`
