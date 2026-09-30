# 第 12 章 可复现步骤清单

这一章要**能照着跑**。命令用绝对路径，可直接复制。每条标注性质：

- `[只读]`：不改任何文件。
- `[改文件]`：会写文件。
- `[长耗时]`：分钟级任务。

约定变量（下文用）：

```sh
REPO=/storage/Users/currentUser/workspace/hicodeer           # 我们的 1.23 树（仓库根）
TMP=/storage/Users/currentUser/tmp                           # 临时资产根（/tmp 只读，勿用）
I18N=$TMP/zed-i18n                                           # 汉化项目副本
BASE=$TMP/zed-i18n-baseline-1.21                             # 1.21 基线（另存）
VENV=$TMP/zed-i18n-venv                                      # 本任务的 python venv
PYBIN=$HOME/.harmonybrew/bin/python3                         # harmonybrew 的 python 3.14.7
```

> 命令性质汇总：步骤 1、5、6、9 含 `[只读]` 或调研；步骤 2、4 改临时区；步骤 10 编译、11 装包改设备状态。**本清单不涉及 clean 与 release**（全局规则第 12、17 条）。

## 步骤 1：拉取汉化项目 `[只读]`

```sh
git clone --depth 1 --progress https://github.com/LI-NA/zed-i18n.git $I18N
# 校验：
git -C $I18N log -1 --format="%h %ad %s" --date=short     # 期望 c45ba0f 2026-09-25 feat: update Zed to v1.21.0
git -C $I18N tag --points-at HEAD                          # 期望 v1.21.0-i18n.1
cat $I18N/config/project.toml                              # 期望 zed_version = "v1.21.0"
```

## 步骤 2：准备 python 环境（含两个 musl 坑）`[改文件]`

本机**没有 `uv`**，只有 harmonybrew 的 Python 3.14.7；`tree-sitter` 无 cp314 预编译轮子 → 必须源码编译。

```sh
# 坑①：缺 -lintl → 用 harmonybrew 的 libintl
export LDFLAGS="-L$HOME/.harmonybrew/lib -Wl,-rpath,$HOME/.harmonybrew/lib"

$PYBIN -m venv $VENV
$VENV/bin/pip install --upgrade pip
# 坑②：pip 打包阶段 bdist_wheel 的 musllinux 探测会执行 /lib/ld-musl-aarch64.so.1 被 Permission denied
#        → 必须 --no-build-isolation，并在 venv 内先装 setuptools + wheel
$VENV/bin/pip install setuptools wheel
$VENV/bin/pip install --no-build-isolation 'tree-sitter>=0.25,<0.26' 'tree-sitter-rust>=0.24,<0.25'

# 校验（期望 tree_sitter-0.25.2 + tree_sitter_rust-0.24.2）：
$VENV/bin/pip list | grep -i tree-sitter
```

- **全程不用 `sudo`，不动系统 / 全局 Python。**

## 步骤 3：备份产物 `[只读]`（对临时区而言是复制）

- 把 `$I18N/catalog`、`$I18N/manifest`、`$I18N/translations` 另存为 1.21 基线（用于第 5 章的差异比对）：

```sh
mkdir -p $BASE
cp -r $I18N/catalog $I18N/manifest $I18N/translations $BASE/
```

- 落地阶段还要**先备份**要改的每个仓库文件到 `$TMP/i18n-landing-backup/`（见第 10 章第 1 步）。

## 步骤 4：跑 extract（对 1.23 树重提取）`[改文件][长耗时]`

```sh
$VENV/bin/python <driver.py> --root $I18N extract --zed-root $REPO
# 期望输出：Extracted 7977 source strings from 10210 occurrences
# 期望 rc=0，耗时约 250 秒
```

- `<driver.py>`：做**运行时替换** `tools.zed_i18n.cli.ensure_inside_workspace` 为恒等函数后再调 `cli.main` 的驱动脚本（绕过工作区守卫，**不修改项目任何文件**）。
- **注意**：`extract` 会**覆盖** `$I18N/catalog/en-US.json` 与 `$I18N/manifest/ui-strings.json`，所以步骤 3 的另存要在本步**之前**做。
- 校验：跑前跑后 `git -C $REPO status` 应无新增改动（我们的仓库全程只读）。

## 步骤 5：比对差异 `[只读]`

- 用 `python3` 读 JSON 算（**不要用 grep**，本机 grep 会静默失效，且对中文路径直接报错）：

```sh
$PYBIN - <<'PY'
import json
a=set(json.load(open('/storage/Users/currentUser/tmp/zed-i18n-baseline-1.21/catalog/en-US.json')))
b=set(json.load(open('/storage/Users/currentUser/tmp/zed-i18n/catalog/en-US.json')))
print("共有", len(a&b), "仅1.21", len(a-b), "仅1.23", len(b-a), "并集", len(a|b))
PY
```

- 期望：共有 7817 / 仅 1.21 有 64 / 仅 1.23 有 160 / 并集 8041。

## 步骤 6：归类（把 160 条分层）`[只读]`

- 按第 7 章的分层口径：28（自己的，`qemu.rs`）/ 22（上游可见）/ 62（上游 doc）/ 48（误抓）。
- 校验手段：按 manifest 的 `needs_review` 条目，按 `file` 与 `kind` 分组统计。

## 步骤 7：翻译 `[改文件]`

- 产出补充词表 `$TMP/i18n-supplement/zh-CN-supplement.json`（112 条）与复核清单 `review.md`。
- 术语一致性用第 8 章做法：从既有 `translations/zh-CN.json` 找术语出处。
- 自检：`json.load` 通过、112 条、112/112 key 与 `catalog` 相等、占位符保留、无空串。

## 步骤 8：生成语言包 `[改文件][长耗时]`

```sh
$VENV/bin/python <driver.py> --root $I18N generate-runtime-bundles <相关参数>
cp $I18N/<产出的语言包> $REPO/assets/locales/
```

- 同样可能遇工作区守卫，用 `<driver.py>` 绕过。

## 步骤 9：接入骨架 `[改文件]`

- 接入 overlay 既有件（源在 `$I18N/tools/zed_i18n/runtime_overlay/crates/`）：`localization`（已接）、`zed/ui_locale.rs`、`title_bar` 菜单标签、`settings_ui/locale_picker.rs`、`language_selector/localized_language_name.rs`。
- 在最早启动路径调 `localization::initialize(...)`。
- **每改一个仓库文件前先备份**到 `$TMP/i18n-landing-backup/`。

## 步骤 9b：全量回注（`apply-universal`）`[改源码][长耗时]`

回注的**实际命令序列**（顺序不能换；绝对路径可直接复制）：

```sh
VENV=/storage/Users/currentUser/tmp/zed-i18n-venv
DRIVER=$TMP/i18n-driver.py
ROOT=$I18N
REPO=/storage/Users/currentUser/workspace/hicodeer

# 1) 重提取（让字节偏移对准当前树）[改临时区][长耗时]
$VENV/bin/python $DRIVER --root $ROOT extract --zed-root $REPO
#    判据：Extracted 7977 source strings from 10210 occurrences

# 2) 核对 catalog_sha256（词表/语言包与 catalog 的绑定值一致）[只读]

# 3) 需要时重生成运行时语言包 [改文件]
$VENV/bin/python $TMP/run_i18n_bundles.py
#    判据：assets/locales/*.json 被刷新

# 4) 一次性回注 [改源码][长耗时]
$VENV/bin/python $DRIVER --root $ROOT apply-universal --zed-root $REPO
#    判据：Applied universal localization to 9439 occurrences across 90 crates
```

- **必须用 `apply-universal`，不能用 `apply`**：后者**写死单语言**（把英文直接换成中文、破坏语言切换）且目标路径写死到它自己的 1.21 缓存；`apply-universal` 是「包成 `localized_str!` + 内存规划后一次性落盘 + 幂等标记」。详见第 15.1 节。
- **驱动脚本形态**（`$TMP/i18n-driver.py`）：运行期把 `tools.zed_i18n.cli.ensure_inside_workspace` 替换为**恒等函数**后再调 `cli.main`——**不改 zed-i18n 检出里的任何文件**。`$TMP/run_i18n_bundles.py` 同理。
- **步 1 会覆盖** `$I18N/catalog/en-US.json` 与 `$I18N/manifest/ui-strings.json`；1.21 基线要在步 1 之前另存（见步骤 3）。
- **铁律**：**任何改动源码之后，都必须重跑 extract 再回注**（偏移会失效；见第 15 章第三道闸）。
- **回注后判据**：跑一句机检确认「新增行里的中文字符数 = 0」（第 10.3 节），以证没有误用 `apply`。

## 步骤 10：编译 `[长耗时]`

```sh
cd $REPO && script/bundle-ohos
# 判据：EXIT=0 且出现构建成功标志（HAP BUILD SUCCESSFUL）
```

## 步骤 11：覆盖安装 `[改设备状态]`

```sh
cd $REPO && ./install-local.sh          # 不带任何卸载参数
```

- **严禁 `--reinstall` 或任何卸载**（会清空沙箱）。

## 步骤 12：取证 `[只读]`

```sh
timeout 5 hdc hilog 2>&1 | grep "<程序名>" | head -20
# 关注 localization initialized: requested=... system=... resolved=... 之类日志
```

- 界面效果**人工确认**（**禁止截图**）。
