# 扩展市场脱离 Zed 服务：静态注册表（Gitee）与本地推荐

- 日期：2026-10-30
- 范围：`crates/extension_host/`、`crates/extensions_ui/`、`crates/extension_suggest/`（只读上游）、
  项目外的同步工具 `hicodeer-ext-sync/`、Gitee 仓库 `jaffengit/hicodeer-extension`
- 结论：扩展的**发现、检索、安装、升级**四条路径在 OHOS 构建下全部走 Gitee 静态文件，
  不再向 `api.zed.dev` 发起任何请求；打开 `.toml` 时的安装提示由编译期静态表驱动，同样不联网。

## 1. 目标与边界

Zed 的扩展市场是一个按查询过滤的服务端接口（`/extensions`、`/extensions/updates`、
`/extensions/{id}`、`/extensions/{id}/{version}/download`）。HiCodeer 不能使用该服务，
但扩展页面的语义必须保留：分类浏览、搜索、检查更新、版本选择、安装、卸载。

**做法**：把"服务端按查询过滤"改成"客户端拉一份全量目录、在内存里按同样的语义过滤"。
目录是一个 `index.json`，包是每扩展一个 `tar.gz`，都放在 Gitee。

**不在范围内**：AI/协作/遥测等其它官方服务（见 `2026-09-17-zed官方服务依赖分析.md`）；
扩展包的签名体系（我们只有目录内的 sha256 绑定）；扩展的编译（上游模型本来就是预编译 wasm）。

`index.json` 的定位是**纯缓存**：客户端把它当"服务器返回的那一页"，从不回写；真正的状态是
本机 `extensions/installed/` 目录，`rebuild_extension_index` 扫描它重建本地索引。

## 2. 总体结构

```
Gitee: jaffengit/hicodeer-extension (master)
├── index.json                 # { "data": [ExtensionMetadata + download_url + sha256, ...] }
└── packages/
    └── <extension-id>-<version>.tar.gz   # 可复现打包，sha256 与 index 一致

客户端（OHOS 构建）
  扩展页面 / 自动检查更新 / 版本选择 / 安装
        │  fetch_extensions(path, query)
        ▼
  fetch_extensions_from_api  ──(static_registry_base().is_some())──▶  静态注册表
        │                                                                 │
        └── 否则（桌面构建）走 api.zed.dev ◀──────────────────────────────┘
```

`static_registry_base()`（`crates/extension_host/src/extension_host.rs:143`）的取值：

1. 环境变量 `HICODEER_EXTENSION_REGISTRY` 非空 → 用它（便于本地起一个目录服务调试）；
2. 否则 `cfg!(target_env = "ohos")` → 返回常量 `STATIC_REGISTRY_BASE`（`:139`，Gitee raw 地址）；
3. 桌面构建返回 `None`，行为与上游一致。

**换服务器只需改这个常量或设环境变量**；`download_url` 是绝对地址，因此目录与包可以分属不同
主机/仓库（拆仓扩容时只改 `index.json` 里的 URL）。

## 3. 客户端实现

### 3.1 目录解析要容错

`parse_static_registry`（`:178`）逐条反序列化，单条坏数据只记日志跳过，不连坐整页——
早期版本一条缺字段会让整个市场页空白。`StaticRegistryEntry::is_valid`（`:167`）要求
`download_url` 非空、`sha256` 为 64 位十六进制，避免把"看起来像包"的条目放进目录。

### 3.2 把服务端语义搬到本地

`filter_static_registry`（`:290`）实现三条路径的语义：

| 路径 | 上游语义 | 本地实现 |
|---|---|---|
| `/extensions` | `filter`（搜索）+ `provides`（分类） | 按 id/名称/描述做子串匹配；`provides` 取交集 |
| `/extensions/updates` | 只返回 `ids` 里点名的扩展 | **空 `ids` 返回空**，不是"全目录" |
| `/extensions/{id}` | 该扩展的**全部版本**，由 UI 置灰不兼容项 | 跳过兼容性过滤，保留全部版本 |

三个空值语义是踩过坑的：

- `provides=""`：UI 在"全部分类"时也会传这个参数。空集合必须视为"不过滤"，否则"全部"页为空。
- `ids=""`：对 `updates` 必须视为"没有可更新的"，否则会把用户关掉自动更新的扩展一起升。
- `/extensions/updates` 不能被当成 id `"updates"`（`strip_prefix("/extensions/")` 的经典陷阱），
  否则自动更新静默失效。

浏览列表按 id 只保留最新版本（去重），再按名称排序，避免同一扩展产出多张卡片。

### 3.3 拉取：共享任务 + 缓存

静态宿主没有查询接口，每次按键都拉 `index.json` 不可接受。`static_registry_snapshot`
（`:1181`）做两件事：

- 成功后把目录存进 `static_registry`（`:457`），后续直接命中；
- 失败/进行中的请求存进 `static_registry_task`（`:460`）的 `Shared<Task<…>>`，
  并发调用共享同一次下载。

对外返回的是薄壳 `cx.spawn(async |_, _| shared.await)`：调用方（扩展页每次输入都会取消上一次
请求）丢弃句柄不会中断真正的下载。共享任务的错误类型是 `Arc<str>` 而不是 `anyhow::Error`，
因为 `Shared` 要求输出可 `Clone`；错误链在创建时用 `{error:#}` 展开。

### 3.4 安装与完整性

`install_latest_from_static_registry`（`:1431`）在本地按兼容性挑最新版本；
`install_or_upgrade_from_static_registry`（`:1509`）按指定版本挑。两者都把条目的 `sha256`
传给 `install_or_upgrade_extension_at_endpoint`（`:1263`），下载后校验（`:1316`）：

```rust
let actual = hex::encode(Sha256::digest(&tar_gz_bytes));
if !actual.eq_ignore_ascii_case(expected) { bail!("extension archive checksum mismatch …") }
```

`require_secure_url`（`:232`）在 release 构建里拒绝明文 `http://`：sha256 只绑定"包↔目录"，
能改目录的人也能改包，传输层是唯一的锚。`static_registry_url`（`:248`）用 `Url::join` 解析
相对路径，绝对地址原样使用。

### 3.5 界面侧的三处调整

| 调整 | 位置 | 原因 |
|---|---|---|
| 隐藏"调试适配器""MCP 服务"分类 | `extensions_ui.rs:1629` | 本构建没有这两类功能，分类下永远为空 |
| 卡片不再宣传这两类能力 | `extension_card.rs` `provided_feature_labels` | 同上，避免卡片承诺做不到的功能 |
| 隐藏 Configure 按钮 | `extension_card.rs:45`（`CONFIGURE_ACTION_AVAILABLE`） | 该按钮只服务 MCP 配置，MCP 已裁剪 |
| 不显示下载量 | `extension_card.rs:53`（`SHOW_DOWNLOAD_COUNT`） | 静态宿主没有按文件统计，卡片会全是 `Downloads: 0` |

四处都用一对互斥 `const` 加 `#[cfg(target_env = "ohos")]` 门控，桌面构建保持上游行为，
代码本身不删（避免与上游分叉成两份渲染逻辑）。

### 3.6 打开文件时的安装提示不联网

`crates/extension_suggest/src/extension_suggest.rs` 是上游就有的**编译期静态表**
（`SUGGESTIONS_BY_EXTENSION_ID`，56 个 id ↔ 文件名/后缀），例如
`("toml", &["Cargo.lock", "Pipfile", "toml", "uv.lock"])`。

触发链：`extensions_ui/src/extension_suggestions.rs:21` 订阅 `LspStoreEvent::LanguageDetected`
与 `WorkspaceEvent::ItemAdded` → `suggest_for_buffer`（`:101`）判断"语言为空或 Plain Text"
→ `suggest_extension(path)`（`:165`）本地匹配 → `show_suggestion`（`:202`）弹通知。

因此"怎么知道 toml 有扩展"不查任何服务器；点"安装"走 `install_latest_extension`，OHOS 下同样
落到静态注册表。

### 3.7 版本排序单一来源

版本比较抽成 `pub fn compare_extension_versions`（`:267`，semver 优先、`published_at` 兜底），
版本选择器（`extension_version_selector.rs`）直接复用，避免目录与选择器排序不一致。

## 4. 数据侧：同步工具 `hicodeer-ext-sync`

工具在 HiCodeer 仓库之外，只读写 Gitee 仓库的本地克隆。核心命令：

```sh
python3 sync.py --add --exclude-provides context-servers,debug-adapters --prune --jobs 12
```

### 4.1 目标集合怎么定

1. 上游 `extensions.toml`（全量，1600 左右）**并上** `/extensions` 列表接口（最多返回 1000 条，
   是截断后的子集）。只按接口同步会漏约 600 个。
2. 去掉客户端抑制名单：工具从 `extension_host.rs` 的 `SUPPRESSED_EXTENSIONS` 解析
   （14 个：`snippets`/`ruff`/`ty`/`basedpyright`/`basher` 与 8 个已内置的 ACP 客户端），
   单一来源，避免两处各写一份而漂移。
3. 去掉**只**提供 `context-servers` / `debug-adapters` 的扩展（72 个）。
   注意判据是"能力集合 ⊆ 排除集合"：`swift`/`java`/`dart`/`haskell` 这类语言扩展顺带声明
   `debug-adapters`，按交集排除会误伤。
4. 兼容性：只取 `schema_version ≤ 1`、`wasm_api_version ≤ 0.7.0` 的版本（对应 stable 通道）。
5. 许可门：见 4.2。

### 4.2 许可门

只收明确的开源许可。判定顺序：GitHub `license.spdx_id` 在白名单内 → 通过；`NOASSERTION`/`NONE`
则读 LICENSE 原文按文本识别；文本里出现 `Rider`+`OpenAI`/`Anthropic`、`Restricted Parties`、
`non-commercial`、`research purposes only`、`all rights reserved` → 拒绝；识别不出 → 拒绝；
AGPL → 拒绝（网络条款超出本产品分发模型）。

### 4.3 打包与断点续跑

`pack_reproducible`：条目排序、`mtime`/`uid`/`gid` 归零、gzip 头 `mtime=0`，
同样的输入得到同样的 sha256。打包后解包校验 `extension.toml` 的 `id`/`version` 与目录一致。

包文件以 `<id>-<version>.tar.gz` 命名，已存在就复用（只重算 sha256），所以中断后重跑只补缺的。
`--prune` 负责删两类残留：客户端抑制/纯 MCP 的索引条目，以及没有任何索引条目引用的孤儿包。

### 4.4 本次同步结果

| 项 | 数值 |
|---|---|
| 上游并集 | 1598 个 id |
| 客户端抑制 | 14 |
| 无兼容版本 | 3（`cursor-dark-theme`/`cyberpunk-2077`/`v-theme`） |
| 只提供 MCP/调试 | 72 |
| 许可不合格 | 171（AGPL、无 LICENSE、带 rider 条款） |
| **入库** | **1338**，包体合计 91 MB |

Gitee 免费版单仓库上限 500 MB、单文件 50 MB；实测最大单包 2.4 MB，包体总量不到上限的两成
（含 git 历史的仓库体积约 190 MB）。

少数早期扩展发布的是旧格式包：根目录是 `extension.json` 而不是 `extension.toml`，
且旧清单没有 `id` 字段。工具对这两种布局都兼容，旧格式只校验 `version`。

## 5. 已知限制

1. **能装上 ≠ 功能齐全**：语言扩展只提供 tree-sitter 语法（wasm），补全/诊断依赖外部
   language server，OHOS 上多数没有对应二进制。装上后通常只有高亮。
2. 约 540 个 id 在 `extensions.toml` 里但不在 `/extensions` 列表接口返回的 1000 条内。
   这些扩展可正常查询与下载（静态目录不依赖列表接口），但"官方站点可见性"与本地不一致。
3. 下载量不统计：Gitee 不提供按文件的下载计数。要真实数字需自建计数端点或改用带计数能力的托管。
4. 完整性只到"目录内的 sha256"，没有扩展签名，防的是传输损坏与目录内混入错误包，
   不防"目录本身被替换"（该风险由 HTTPS 承担）。
5. 同步工具是手工执行的外部脚本，没有 CI 定时任务；上游新增扩展需要人工跑一次。

## 6. 改动清单

| 文件 | 内容 |
|---|---|
| `crates/extension_host/src/extension_host.rs` | 静态注册表：常量/开关、条目与解析、URL 解析、过滤语义、共享拉取、安装与 sha256 校验、`compare_extension_versions` 导出 |
| `crates/extension_host/src/static_registry_test.rs` | 6 个过滤语义单测（空 provides / 搜索 / updates 空 ids / updates 指定 ids / 版本查询 / 浏览去重） |
| `crates/extension_host/Cargo.toml` | 新增 `sha2`、`hex` |
| `crates/extensions_ui/src/extensions_ui.rs` | 分类行隐藏调试适配器与 MCP（cfg 门控） |
| `crates/extensions_ui/src/components/extension_card.rs` | 能力标签过滤、Configure 与下载量的 cfg 门控 |
| `crates/extensions_ui/src/extension_version_selector.rs` | 复用 `compare_extension_versions`，删除重复排序与无用 import |

## 7. 验证

- `cargo check -p extension_host -p extensions_ui --tests` → EXIT=0（本 crate 无警告）
- `cargo test -p extension_host --lib static_registry` → 6 passed / 0 failed
- `bash script/bundle-ohos` → `=== HAP BUILD SUCCESSFUL ===`；`./install-local.sh` 安装并拉起，
  `m.hicodeer.edit` 进程存活
- 同步工具：`--status` 全量差异为 0；`index.json` 条目与 `packages/` 文件一一对应，无孤儿
