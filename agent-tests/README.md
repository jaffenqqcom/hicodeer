# agent-tests

「agent 历史会话」功能（面板 History、`agent.sessions` 接口、会话恢复）的测试资产。
被测代码在 `herdr/src/sessions/`，本目录只存放测试数据与端到端脚本，不含可编译代码。

分两块，另有单元测试入口脚本：

- `fixtures/<agent>/` — 单元测试用的最小样本，被 `herdr/src/sessions/*.rs` 末尾的
  `#[cfg(test)] mod tests` 直接读取。
- `e2e/` — 端到端脚本（拉起 daemon、打面板、验证恢复），由 e2e 同事维护；
  本目录的 fixtures 可复用，但两边改动互不影响。
- `run-unit-tests.sh` — 单元测试入口：构建、strip、自签名后执行，本机唯一可用跑法。

## 跑单元测试

从仓库根目录跑（本机唯一可用入口）：

```sh
./agent-tests/run-unit-tests.sh 'sessions::'
```

脚本先 `cargo test -p hicodeer-agent --lib --no-run` 构建，再把测试二进制复制到
`${TMPDIR:-/tmp}/herdr-unit-tests/`，`strip` 后用
`binary-sign-tool sign -selfSign 1` 自签名，最后直接执行签名后的二进制。
**本机必需这一步**：设备拒绝 exec 未签名的大体积 ELF。中间产物留在该临时目录，
下次构建会复用已签名的副本（源二进制更新时才重新签）。

参数：

- 不带参数：跑 lib 里的全部单元测试。
- 第一个参数是测试过滤器，直接传给测试二进制，因此命中规则是测试全路径子串。
  本目录的测试在 `sessions::<agent>::tests::…`，所以 `'sessions::'` 正好只跑
  `src/sessions/` 下的用例；不加过滤则跑整个 lib。
- `--` 之后的参数原样透传给测试二进制，例如
  `./agent-tests/run-unit-tests.sh 'sessions::' -- --nocapture`。

### 裸 `cargo test` 在本机会失败，且不是测试的问题

直接 `cargo test -p hicodeer-agent --lib` 编出的二进制在本机跑不起来：

- 未 strip：`Operation not permitted`（execve 直接 EPERM）；
- strip 后：`permission denied`（缺 `.codesign` 段）。

这是设备对未签名大体积 ELF 的执行限制，与用例本身无关。小 crate（例如 icons）
的测试二进制体积小，未签名也能跑，所以别拿它的结果推断 herdr。

## fixtures 约定

- 每个 agent 一个子目录，`fixtures/<agent>/` 下按真实存储布局摆放，例如
  `fixtures/deepseek/sessions/--repo-demo--/session-alpha/session.v4.jsonl`。
- 测试用 `env!("CARGO_MANIFEST_DIR")` 拼 `../agent-tests/fixtures` 定位，
  **不读也不改 `HOME`**：测试并行跑，改环境变量会互相干扰。
  真正依赖 `HOME` 的入口（`sessions_root()`、`database_path()`）不在单测覆盖范围内，
  由 e2e 用真实目录覆盖。
- 需要造临时文件的测试（例如 deepseek 的 zstd 分支）用 `std::env::temp_dir()`
  拼唯一名字，结束时自己删；不引入 `tempfile` / `rusqlite` 等新依赖。
- 依赖 `sqlite3` 命令行的连接器（deveco、minimax）的 fixtures 存放
  **CLI 的 `-json` 输出**，测试只覆盖解码与行映射；SQL 里的过滤条件
  （archived / visibility / session_kind / parent_id）需要真实数据库，交给 e2e。

## 各 agent 的样本与覆盖点

- `codebuddy/` — `projects/-repo-demo/*.jsonl`：正常解析（含首行损坏仍能读出标题）、
  `ai-title` 缺失时回退 `summary`、两者都缺失时标题为 `None`、文件不存在返回 `None`、
  存储遍历只收非空 `.jsonl`（`empty.jsonl` 与 `notes.txt` 被跳过）。
- `deepseek/` — `sessions/--repo-demo--/session-*/session.v4.jsonl`：正常解析
  （头部 + 首条用户消息拼标题）、`delegationDepth != 0` 的子代理会话被过滤、
  头部截断被跳过、无用户消息时标题为 `None`、`.jsonl.zstd` 解压分支（测试内实时压缩）、
  文件不存在返回 `None`、`is_transcript` 的命名规则、存储遍历只认转录文件
  （只有 `session.lock` 的目录被跳过）。
- `deveco/` — `session_rows.json` / `prompt_rows.json`（`sqlite3 -json` 输出）：
  `New session …` 占位符回退到首条用户消息、空白标题产出 `None`、
  `parent_id` 子会话判定、WAL/SHM 后缀拼接、输出截断与空输出不 panic。
- `minimax/` — `session_rows.json`（`sqlite3 -json` 输出）：行解码、标题 trim、
  工作目录与 `updated_at_ms` 映射、`source_path` 为「数据库路径/会话 id」、
  空/损坏输出返回空。
- `codewhale/` — `sessions/*.json`：正常解析（metadata + mtime）、`journal.spawn_depth != 0`
  的子代理会话被过滤、缺 `metadata.id` 时回退文件名、空白标题与工作目录为 `None`、
  `session_boot_owners.json` 无 metadata 被丢弃、点文件不被扫描。
- `atomcode/` — `sessions/<project hash>/*`：只认 `.index`（`.events` / `.rewind` /
  `.lease` / `.meta.lock` 被忽略）、正常解析、缺 id 回退文件名、缺 `updated_at` 时用 mtime、
  截断/缺失文件返回 `None`。
- `opendesk/` — `tasks/task_list.json`：索引解析、`parent_id` 子任务过滤（空字符串不算）、
  标题 trim、`source_path` 指向任务文档、缺失文档时 `ended_at` 为 `None`、
  截断/缺失索引返回空。
