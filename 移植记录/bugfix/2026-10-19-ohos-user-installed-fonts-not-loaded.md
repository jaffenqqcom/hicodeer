# OHOS 加载不到用户安装字体：字体枚举漏掉字体管理器安装目录

> 相关定位手段见 [[ohos-debug-lessons]]（按进程 uid 判断沙箱可读性、用系统常量而非猜测路径、用官方源码确认唯一安装通道）。

## 问题描述

HiCodeer（OHOS 2in1）的字体列表里看不到用户自己安装的字体；即使把 `buffer_font_family` 手写成用户字体名，实际渲染也会回退到 HarmonyOS Sans。

OHOS 上没有 fontconfig，`gpui_ohos` 的文本系统自己实现了系统字体枚举：在首次 `font_id()` 调用时扫描一组硬编码目录并 `fontdb::Database::load_fonts_dir()`。这组目录只有 `/system/fonts`、`/system/font`、`/vendor/fonts`、`/system_ext/fonts`、`/data/fonts`，全部是系统预置字体的位置；用户通过系统「字体」能力或主题商店安装的字体不在这里。

## 问题表现

- 设置里的字体族下拉列表（数据源是 `PlatformTextSystem::all_font_names`）只有系统字体，搜不到用户安装的字体。
- 在 `settings.json` 里显式写用户字体的族名，文本仍按回退链渲染成 HarmonyOS Sans。
- 安装字体后即使应用不重启，字体也不可能出现；重启后仍然没有。
- 本机环境可直接复现：`/data/service/el1/public/for-all-app/fonts/` 下有 17 个用户安装的 `MapleMono-NF-CN-*.ttf`，但应用内不可见。

## 问题原因

### 字体枚举依赖硬编码目录

`crates/gpui_ohos/src/ohos/text_system.rs:232` 的 `ensure_system_fonts_loaded()` 在 240 行声明了固定的目录列表：

```rust
const FONT_DIRS: [&str; 5] = [
    "/system/fonts",
    "/system/font",
    "/vendor/fonts",
    "/system_ext/fonts",
    "/data/fonts",
];
```

OHOS 的 `fontdb::Database::load_system_fonts()` 走的是 Linux 的常见字体目录（`/usr/share/fonts` 等），在本机一个都不存在，所以代码改为手写目录扫描；但手写列表只覆盖了「系统预置字体」，漏掉了「用户安装字体」。

### 用户字体的唯一落点被漏掉

OpenHarmony 的字体管理服务（global_font_manager）只有一条安装路径，源码 `frameworks/fontmgr/src/font_manager.cpp` 中的常量：

```cpp
static const std::string INSTALL_PATH = "/data/service/el1/public/for-all-app/fonts/";
static const std::string FONT_CONFIG_FILE = INSTALL_PATH + "install_fontconfig.json";
static constexpr int32_t MAX_INSTALL_NUM = 200;
```

无论用户是通过系统字体安装器还是主题商店安装，最终都调用该服务的 `InstallFont`，字体文件被拷贝到上述目录，并在 `install_fontconfig.json` 的 `fontlist[].fontfullpath` 中登记绝对路径。

本机实测（进程 uid 20020117，普通应用身份）：

```
$ ls -ld /data/service/el1/public/for-all-app/fonts/
drwxr-xr-x 2 font_manager font_manager 3440 Sep 27 19:33 .
$ ls /data/service/el1/public/for-all-app/fonts/ | head
MapleMono-NF-CN-Bold.ttf
MapleMono-NF-CN-BoldItalic.ttf
...
```

目录权限 `drwxr-xr-x`、文件 `-rw-r--r--`，应用沙箱可读；`install_fontconfig.json` 里的 `fontfullpath` 全部指向该目录。同时确认按 userId 的旧路径（`/data/service/el1/100/for-all-app/fonts`）不存在，`temp/` 也不存在。

### 为什么设置里也看不到

`all_font_names()`（`text_system.rs:87`）直接遍历 `font_system.db().faces()`，而 `db` 只包含扫描过的目录。设置界面的字体列表来自 `crates/theme/src/font_family_cache.rs:47` 的 `all_font_names()`，所以未扫描的字体既不能选、也不能按名字命中。

## 解决方案

### 选型：加目录 vs 走 ArkGraphics 路径枚举

最初的方案是移植 warp-ohos 的做法：调用 NDK 的 `OH_Drawing_GetFontPathsByType(ALL, &count)` 拿系统字体管理器登记的所有字体文件路径，取父目录后并入扫描（`../warp-ohos/crates/warpui/src/platform/ohos/fonts.rs:155` 的 `reported_font_dirs()`）。该方案与安装方式解耦、更通用，但需要约 50 行 FFI 声明，且该符号自 API 23 才存在，低版本设备上静态链接会导致整个 `libcore.so` 加载失败。

实际查证后确认：用户字体的落点是**唯一且固定**的常量目录（字体管理器只有一条安装通道），因此采用更小的改动——只补一个目录。

### 改动

```rust
const FONT_DIRS: [&str; 6] = [
    "/system/fonts",
    "/system/font",
    "/vendor/fonts",
    "/system_ext/fonts",
    "/data/fonts",
    // User-installed fonts, installed by the system font manager.
    "/data/service/el1/public/for-all-app/fonts",
];
```

`load_fonts_dir` 内部递归遍历，`temp/` 正常为空，不会引入噪声。

### 验证

- 构建：`bash script/bundle-ohos --release` → `=== HAP BUILD SUCCESSFUL ===`（42m36s）。
- 安装：`./install-local.sh` → `install bundle successfully`，应用启动成功。
- 运行：应用进程正常，hilog 中无 `panic` / `abort`，也没有 `OHOS text system: no family match` 之类的字体回退告警。

### 已知边界

- **必须重启应用**：`system_fonts_loaded` 是进程内一次性标志（`text_system.rs:232-234`），运行期间新装字体不会被感知。
- **主题商店字体未实测**：本机未安装主题字体。依据是字体管理服务为唯一安装通道、且其 `INSTALL_PATH` 仅此一个，推断主题商店字体同样落该目录；若后续遇到例外，再补 ArkGraphics 枚举兜底。
- **数量上限**：字体管理器最多安装 200 个字体（`MAX_INSTALL_NUM`）。
- **启动成本**：首次扫描会多解析用户字体（本机 17 个约 20MB 的 ttf，fontdb 走 mmap，主要成本是读 face 元数据），量级与 `/system/fonts` 的 263 个文件相当。

### 排查过程中的死路

- **一开始直接推荐 FFI 枚举**：技术上正确，但对「目录是否固定」未做查证就上了更重的方案；后续确认安装路径是源码常量后改为一行目录。
- **先怀疑字体重名/归一化**：`normalize_family_name()` 已处理大小写、下划线、连字符，用户字体名带空格也能匹配；排除该方向后回到枚举源头。
- **尝试直接读 `/proc/<app_pid>/maps` 验证字体是否 mmap**：应用是独立沙箱 uid，`maps` 返回 `Operation not permitted`，无法作为判据；改用 hilog 与目录可读性检查。

## 修改文件

- `crates/gpui_ohos/src/ohos/text_system.rs:240` — `FONT_DIRS` 由 5 项扩为 6 项，新增 `/data/service/el1/public/for-all-app/fonts`（系统字体管理器安装用户字体的唯一目录）。
