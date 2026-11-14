# keymap 动作名去 Zed 化（zed:: 改 hicodeer::）

## 问题描述

HiCodeer 的用户可见面统一要求不出现 Zed 品牌，但 keymap 体系里的 action 名一直以 `zed::` 命名空间出现：OHOS 默认 keymap 文件、keymap 编辑器、which-key 面板、命令面板补全、keymap JSON 的 schema 补全，以及 keymap 编辑器写回用户 `keymap.json` 的内容，全都带着 `zed::` 前缀。用户能直接看到这些字符串，与产品名不一致。

## 问题表现

- `assets/keymaps/default-ohos.json` 里有 20 处 `"zed::Xxx"`，该文件会编译进发布版本的资源（release 下由 `fs_embed!` 嵌入并压缩，debug 下运行时读磁盘）。
- keymap 编辑器的动作列表、补全候选显示为 `zed: Open Settings` 这类形式。
- which-key 面板等待按键时，候选动作显示 `zed: ...`。
- 在 keymap JSON 里输入动作名时，语言服务的自动补全给的是 `zed::Xxx`，schema 校验也只认 `zed::Xxx`。
- 在 keymap 编辑器里保存绑定后，写回用户 `keymap.json` 的仍是 `zed::Xxx`。
- 单纯把 `default-ohos.json` 里的 `zed::` 改掉不会生效，运行时会报 `didn't find an action named "hicodeer::Quit"`。

## 问题原因

action 的注册全名是 `namespace::ActionName`，其中 namespace 由宏参数显式指定，不是 crate 名：

- `crates/zed/src/zed.rs` 的 `actions!(zed, [Hide, Minimize, ToggleFullScreen, ...])`
- `crates/zed_actions/src/lib.rs` 的 `actions!(zed, [OpenSettings, Quit, ...])` 与十余处 `#[action(namespace = zed)]`
- `crates/gpui/src/action.rs` 的内建 `NoAction` / `Unbind` 同样是 `namespace = zed`
- `crates/onboarding`、`crates/extensions_ui` 各有少量

keymap 解析时按这个全名去 ActionRegistry 查表（`settings/src/keymap_file.rs` 的 `build_keymap_action_value` → `App::build_action`），名字对不上就报 Action not found。因此「只改 JSON 字符串」不可行，必须在解析链路上做命名空间映射；反过来，只在解析层接受 `hicodeer::` 而不改显示与写回，用户又会在编辑器和 schema 补全里继续看到 `zed::`。

## 解决方案

采用「内部标识不变、对外呈现换名」的双向映射，参考命令面板既有的显示层做品牌替换（commit 82d3559），把改动限制在名字的进出口，不触碰 action 注册本身。

- **显示**：`command_palette::display_action_name`（humanize 后把 `zed` 词替换成 `hicodeer`），keymap 编辑器列表、补全候选、which-key 面板统一使用。
- **补全插入**：`rebranded_action_name` 把 `zed::X` 换成 `hicodeer::X`，保证插入 keymap JSON 的字面量也是产品名。
- **解析**：`KeymapFile::normalized_action_name` 把 `hicodeer::X` 还原成 `zed::X` 再进 registry，旧文件里的 `zed::X` 原样通过。
- **写回**：`KeybindUpdateTarget` 拆成 `action_value()`（内部名，用于查找）与 `serialized_action_value()`（`hicodeer::` 名，用于写文件），Replace / Add / unbind 三条写入路径统一使用后者。
- **比较**：`normalized_action_value` 在比较前把两侧统一成内部名，且 target 值只在 `find_binding` 入口归一化一次，保证旧文件与新文件都能命中同一条绑定，不会重复新增。
- **schema**：`settings_content::rebranded_action_name` 用于 `ActionName::build_schema` 与 `KeymapFile::generate_json_schema` 的 const / enum / 废弃提示，补全与校验都只给 `hicodeer::`。
- **资源与文档**：`default-ohos.json` 20 处、`docs/src/key-bindings.md` 与 `docs/src/vim.md` 的示例同步；`docs_preprocessor` 增加反向归一化，保证文档校验与键位表生成都能识别新名字。

刻意未做的两件事：action 注册本身不改名（避免动上游 crate 与全部 keymap 的解析契约）；废弃别名 `zed_actions::X` 保持原样（解析层只把 `hicodeer::` 映射到 `zed::`，没有到该别名的映射，强行换名会解析失败）。

## 修改文件

- `assets/keymaps/default-ohos.json` — 20 处 `zed::` 改为 `hicodeer::`。
- `crates/command_palette/Cargo.toml` — 增加 `settings_content` 依赖，复用换名函数。
- `crates/command_palette/src/command_palette.rs` — `palette_command_name` 改为公开的 `display_action_name`，并重导出 `rebranded_action_name`。
- `crates/docs_preprocessor/src/main.rs` — 新增 `internal_action_name`，动作查找与按键查找前做 `hicodeer::` → `zed::` 归一化。
- `crates/keymap_editor/src/action_completion_provider.rs` — 补全插入文本使用 `hicodeer::X`。
- `crates/keymap_editor/src/keymap_editor.rs` — 列表/补全/动作名回填改用 `display_action_name`；输入框同时接受两种命名空间；复制 action 用 `hicodeer::X`，telemetry 仍上报内部名。
- `crates/settings/src/keymap_file.rs` — 解析归一化、schema 换名、写回换名、比较归一化，并补充对应单测。
- `crates/settings_content/src/action.rs` — 新增 `rebranded_action_name`，schema 输出与废弃提示换名。
- `crates/settings_content/src/settings_content.rs` — 导出 `rebranded_action_name`。
- `crates/which_key/src/which_key.rs` — 候选绑定标签使用 `display_action_name`。
- `crates/which_key/src/pending_bindings.rs` — 测试期望同步为 `hicodeer: ...`。
- `crates/which_key/src/pending_keystrokes_indicator.rs` — 测试期望同步为 `hicodeer: ...`。
- `docs/src/key-bindings.md` — keymap 示例改为 `hicodeer::OpenKeymap`。
- `docs/src/vim.md` — keymap 示例改为 `hicodeer::OpenLog`。

## 验证

- `cargo test -p settings --lib keymap_file`：13 passed。
- keymap 相关全量：settings 49 passed / 1 failed（`settings_store::test_agent_profile_tool_schema`，属 agent profile schema，与本次改动无关）、settings_content 33 passed、command_palette 23 passed、keymap_editor 47 passed、which_key 43 passed、docs_preprocessor 10 passed。
- `./script/bundle-ohos`：`verified 15 hap entries + 2 hnp payload(s)`，`=== HAP BUILD SUCCESSFUL ===`。
- `./install-local.sh`：覆盖安装并启动成功。

[[ohos-debug-lessons]]
