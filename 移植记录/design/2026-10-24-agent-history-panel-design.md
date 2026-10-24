# Agent 历史会话：整体设计

日期：2026-10-24
状态：设计定稿（接口 v2 经子 agent 评审修订）
界面原型：同目录 `2026-10-24-agent-history-panel.html`（浏览器打开可交互）

## 1. 背景与目标

Agent 面板目前只显示 herdr 正在管理的 space 与 agent（`crates/agent_panel`）。各 agent CLI（CodeBuddy、Claude 等）在各自家目录下保存历史会话，用户离开终端就无法找回。

目标：在 Agent 面板内显示历史会话，以 space 为一级分组、会话为二级；支持单击打开（space 未打开则先建再开）、置顶、重命名、归档/恢复。已打开的会话不出现在历史区。

第一批范围：**Claude + CodeBuddy 闭环**。FAD（franken-agent-detection）一次接入，其余 agent 留槽，后续按需开放。

## 2. 整体分层

```
UI（agent panel）
  ↓ 只认 SessionRecord
SessionService（app 侧）
  归一 RawSession → SessionRecord；持有置顶/归档/改名状态；
  过滤已打开会话；open_session 后的补偿观测
  ↓ trait AgentController（宿主抽象，herdr 只是一个 impl）
HerdrController（app 侧 impl）
  ↓ 现有 client.rs 的 RPC；新增 agent.sessions（daemon 扫描）
采集源（hicodeerd 内）
  FAD connectors + 自研 CodeBuddy connector → RawSession
```

关键分工（已确认）：

- **daemon 只采集**，不做去重、排序、过滤，不知道「归档/置顶」的存在
- **hicodeer 负责统一**：不同 agent 的标题来源、时间单位、id 语义在此收敛
- **置顶/归档/改名状态存 hicodeer 本地**，与会话文件本身解耦；agent 清理历史不影响状态

## 3. AgentController 接口 v2

```rust
pub trait AgentController: Send + Sync {
    /// None = 该宿主没有可部署的终端（长期状态）；未就绪一律走
    /// ControllerError::NotReady，两个通道不分叉。
    fn host_terminal(&self) -> Option<HostTerminalSpec>;

    fn workspaces(&self) -> Result<Vec<WorkspaceRef>>;
    fn create_workspace(&self, root: &Path, label: Option<&str>) -> Result<WorkspaceRef>;
    fn focus_workspace(&self, id: &str) -> Result<()>;
    fn rename_workspace(&self, id: &str, label: &str) -> Result<()>;
    fn close_workspace(&self, id: &str) -> Result<()>;

    fn agents(&self) -> Result<Vec<AgentRef>>;
    fn focus_agent(&self, pane_id: &str) -> Result<()>;

    /// 历史会话（daemon 扫描各 agent 的会话存储）。
    /// workspace_id：Some = 已打开的 space；None = 未打开的 space。
    fn sessions(&self) -> Result<Vec<RawSession>>;

    /// 打开一条历史会话，见下方「三条本质限制」。
    fn open_session(&self, request: OpenSessionRequest) -> Result<OpenedSession>;

    fn integrations(&self) -> Result<Vec<IntegrationRef>>;
    fn install_integration(&self, target: &str) -> Result<Vec<IntegrationRef>>;
    fn reload_config(&self) -> Result<()>;
}
```

```rust
pub struct WorkspaceRef { pub id: String, pub label: String }
pub struct AgentRef { pub pane_id: String, pub workspace_id: String, pub kind: String,
    pub status: AgentStatus, pub session: Option<SessionRef>, pub focused: bool }
pub struct SessionRef { pub agent: String, pub id: String }
pub struct OpenSessionRequest { pub workspace_root: PathBuf, pub agent_kind: String,
    pub session_id: String, pub tab_label: String }
pub struct OpenedSession { pub workspace_id: String, pub pane_id: String }
pub enum AgentStatus { Working, Blocked, Done, Idle, Unknown }
pub struct RawSession { pub agent: String, pub session_id: String, pub title: Option<String>,
    pub workspace_root: Option<PathBuf>, pub updated_at: Option<i64>,
    pub source_path: PathBuf, pub workspace_id: Option<String> }
pub enum ControllerError { NotReady, Unavailable(String),
    Rejected { code: String, message: String }, Io(std::io::Error) }
```

三条本质限制（写进 trait doc）：

1. `workspace_root` 原样作为 `workspace.create` 的 `cwd` 交给宿主校验，接口层不预检
2. `agent_kind` 不做 resume 支持性背书；herdr 的 `agent.start` 参数是自由 argv，不支持的组合只能在 TUI 里表现为报错
3. `agent.start` 成功 ≠ 恢复成功；CLI 内部 resume 失败（如会话文件损坏）接口层不可观测，补偿见第 7 节

评审确定的修正：

- `WorkspaceRef` 不带 root：herdr 的 `WorkspaceInfo` 不回 cwd（`herdr/src/api/schema/workspaces.rs:62`），space 归属的匹配由 daemon 完成（它持有 herdr 状态，知道每个 workspace 的真实根目录），`RawSession.workspace_root` 就是匹配结果
- `install_integration` 返回装完重读后的完整列表（现有实现 `client.rs:149-153` 语义如此）
- 面板标识符统一用 pane_id 一种；`terminal_id` 不进接口
- 同步 trait：现有 client.rs 就是阻塞 TCP（`client.rs:219`），调用方 `cx.background_spawn` 包裹，与面板现状一致

## 4. 采集层（hicodeerd）

- 依赖 `franken-agent-detection`（纯 MIT，无附加条款），git 依赖锁自有 rev。已验证：`--no-default-features --features connectors` 在 `aarch64-unknown-linux-ohos`、stable 1.97.1 下 1 分 40 秒编译通过，依赖仅 serde/chrono/dirs/walkdir/bloomfilter/anyhow/tracing/dotenvy/once_cell
- 自带 `rust-toolchain.toml` 要求 nightly-2026-08-31，但基础 `connectors` 特性在 1.97.1 可编，此偏离记入构建说明
- Claude 用现成 connector（逐行读 cwd 填 `NormalizedConversation.workspace`，已核实）
- **CodeBuddy 自研 connector**：存储与 Claude 同构（`~/.codebuddy/projects/<去掉开头斜杠、其余斜杠换连字符>/<session-id>.jsonl`），标题优先 `ai-title` 行的 `aiTitle`，其次 `summary` 行；目录编码与 Claude 不同（Claude 连开头 `/` 也替换为 `-`）
- 时间取行内毫秒 `timestamp`；cwd 从文件内容读，不从目录名反解（路径里的 `-` 无法还原）
- 解析必须容错：字段缺失跳过该条；只读头部与元数据行，不全量 parse（单会话可到 32MiB，FAD 对超大会话有压缩元数据路径）
- 新增 RPC `agent.sessions`，扫描结果带 `(path, mtime, size)` 缓存

FAD 覆盖 19 种我们识别的 agent（Claude/Codex/Gemini/Cursor/Devin/Antigravity/Cline/Omp/OpenCode/GithubCopilot/Kimi/Kiro/Droid/Amp/Grok/Hermes/Qwen/Muse/Pi），缺 10 种（CodeBuddy/Qodercli/Mastracode/Kilo/Letta/Maki/ZCode/MiniMax/Trae/DeepSeek），其中 CodeBuddy 第一批自研补上。

需要 SQLite 的 feature（cursor/opencode/devin/hermes/goose/crush/copilot-vscdb 等，拉 `fsqlite`+`asupersync`）在 OHOS 上**未验证**，第二批再开。

## 5. 归一与状态（app 侧）

标题归一顺序：本地改名 > 原始 title > 「未命名会话」。唯一键 `(agent, session_id)`。

过滤规则：「已打开的不显示」的判据是 `agents()` 返回里 `session.id` 相等，**不是** `workspace_id`（space 打开不等于该会话打开）。`workspace_id` 只用于分组：`Some` 的会话若未被打开也应显示在对应 space 组下。

状态存储：`.HiCodeer` 目录下一个 JSON 文件，原子写（复用 `herdr_settings.rs` 的 `write_atomically` 模式）。

```json
{
  "version": 1,
  "sessions": {
    "codebuddy:01a1695b": {
      "pinned": true, "archived": false,
      "title_override": null, "updated_at": 1792800000000
    }
  }
}
```

选 JSON 而非 SQLite：条目小、量级几百、无查询需求；SQLite 属于为 YAGNI 买单，量大后再迁。

打开会话后的补偿观测：`open_session` 返回后 10 秒内轮询 `agents()`，该 `session_id` 出现才算成功，否则 toast「会话可能未恢复成功」。

## 6. UI 设计

### 布局

Agent 面板（268px）内，自上而下：

1. 现有 space/agent 树（不变）
2. 分隔区：上边距 20px + 细分隔线 + 「历史会话」小标题（右侧提示「单击打开」）
3. 历史区：
   - 「置顶」组（跨 space，排最前，空则隐藏；悬停行时鼠标旁显示所属 space 的小提示）
   - 各 space 组（一级，可折叠，右侧数字为该组会话数）
   - 「已归档」组（最底部，默认折叠；组内按 space 子分组，行标题降灰、图标 40% 透明）
4. 整个面板一个滚动区；历史超出时上下滚动

### 会话行

复用 `ui::ThreadItem`（`crates/ui/src/components/ai/thread_item.rs`，已 `pub use`）：16px agent 图标（`IconName::AiClaude` / `AiCodeBuddy`，终端会话用文本 `>_`）、标题（单行截断）、相对时间（`12m / 1d / 3w`，与 threads 列表同格式）。行高 32px，与 threads sidebar 一致。

悬停：整行 `ghost_element_hover`（白色 6% 叠加）；右侧渐变遮罩后浮出操作区（`element_background`/`element_active`），按钮为 `IconName::Pin`（未置顶，灰）/ `Unpin`（已置顶，accent 色）、`Pencil`（重命名）、`Archive`（归档）。归档行悬停只有恢复按钮（逆时针箭头，可用 `IconName` 现有图标）。

### 交互

- **单击打开**：行进入 `opening` 态（转圈 + 时间位显示 opening），成功后从历史移除；10 秒未观测到则 toast「会话可能未恢复成功」
- **置顶/取消置顶**：移入/移出置顶组；归档组空则整组隐藏
- **重命名**：标题就地变单行 `Editor`，Enter/失焦保存、Esc 取消
- **归档/恢复**：移入/移出「已归档」组；空 space 子组自动消失
- 折叠：组头整行可点，箭头 `ChevronDown/Right`，悬停仅行底色变化（遵循 threads sidebar 的 hover 约定，不加自定义描边）

### 第一批不做

搜索（`IconName::MagnifyingGlass` + 单行 Editor 现成，二期加）、右键菜单（ContextMenu 现成，二期加）、`events.subscribe` 替代轮询、10 个缺失 agent 的 connector。

## 7. 错误处理

- `ControllerError::NotReady`：daemon 未就绪，面板安静重试（现有逻辑）
- `Unavailable`：桥接连不上，断连态（现有 `note_disconnect`）
- `Rejected { code, message }`：herdr 错误码原样透传（`agent_not_ready`、`workspace_not_found` 等）
- 扫描侧：单个会话文件损坏 → 跳过该条并计数；某 agent 目录不存在 → 该 agent 返回空；扫描异常不阻塞面板（历史区显示空态 + 重试）
- 打开会话：`workspace.create` 失败（目录无效）→ toast 透传宿主错误；`tab.create`/`agent.start` 失败 → toast；启动成功但未观测到恢复 → 第 5 节的补偿提示

## 8. 实施切分与验证

| 步骤 | 内容 | 验证标准 |
|---|---|---|
| 1 | daemon：接 FAD 依赖 + CodeBuddy connector + `agent.sessions` RPC + workspace 匹配 | 用 agent.sock 实调 `agent.sessions`，返回本机真实 Claude/CodeBuddy 会话（含 workspace_id） |
| 2 | app：`AgentController` trait + `HerdrController` 迁移现有 client.rs（行为零变化） | 面板全部现有功能手测不变；`cargo check -p agent_panel` 通过 |
| 3 | SessionService：归一 + 状态 JSON + 过滤/合并 | 重启后置顶/归档/改名保留；已打开会话从历史区消失 |
| 4 | UI：历史区渲染与交互 | 真实会话出现并正确分组；单击打开后进入会话；转圈/补偿提示按设计 |
| 5 | 清理 | 诊断日志全删（hilog-persist guard 确认未落盘） |

## 9. 已知限制

- Claude 官方声明会话格式是内部的、随版本变化；FAD 锁 rev，升级需回归
- Claude 默认 30 天自动清理，历史不保证完整
- FAD 的 SQLite 类 feature 在 OHOS 未验证，Cursor/OpenCode/Devin 等第二批先不承诺
- FAD 要求 nightly-2026-08-31，实际用 1.97.1 编译，升级 FAD 时需重新验证
- 亚秒级的状态变迁（working→done→working）会被一秒轮询漏掉（历史会话不受影响，仅提示）
