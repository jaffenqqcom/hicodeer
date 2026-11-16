# 微信 iLink 通道接入（agent panel IM 通道）

## 问题描述

agent panel 的 IM 通道此前只支持飞书、钉钉、QQ 三条，没有微信入口。微信个人号长期没有官方 Bot 接口（Web 协议、Xposed Hook、iPad 协议均属违规且随时封号），因此一直没法做。2026 年腾讯推出 iLink（ClawBot）协议，OpenClaw 的微信通道即由 `@tencent-weixin/openclaw-weixin` 基于该协议实现。本次把 iLink 作为第四条 IM 通道接入 HiCodeer，并在接入过程中修掉两个暴露出来的问题：连接状态最长 35 秒才显示、通道名在中文界面未翻译。

## 问题表现

- 设置页「IM 通道」下拉只有 Off / QQ / DingTalk / Feishu，没有微信
- 接入首版后，扫码绑定成功，但切到微信通道后「连接状态」长时间停在未连接
- 通道选择器里显示 `WeChat`，中文界面下未显示「微信」

## 问题原因

**1. 平台是编译期静态注册，三处缺一不可**

`crates/im_channel` 的设计是「加平台 = 加模块 + 加枚举变体 + 在 `platforms()` 加一条表项」。但设置页还有两处独立硬编码：

- `CHANNEL_LABELS` / `CHANNEL_CHOICES` 决定下拉里有哪几项
- `platform_name` 与四个 `*_button_id` 的 `match` 必须穷举新变体

只加注册表会让下拉里看不到微信，只加下拉会让 `spec()` 的 `expect` 落空。

**2. iLink 与现有三家的协议差异**

- 鉴权不是 app_id/app_secret 对：一次扫码给出 `bot_token`、`ilink_bot_id`、`ilink_user_id`，部分情况还带 `baseurl`。原 `PollOutcome::Ready` 只有两个固定字段，靠「按位置填 spec 前两个字段」工作，第三个 `owner_id` 与第四个 `base_url` 无处安放。
- 收消息是 HTTP 长轮询（`POST /ilink/bot/getupdates`，服务端 hold 35 秒），不是 QQ 那种 WebSocket。
- 每条回复必须回带**最近一条入站消息的 `context_token`**，服务端才会接受；没有它只能算主动推送，会被拒。
- 扫码状态机比 RFC 8628 多几个分支：`scaned_but_redirect`（换接入主机继续轮询）、`binded_redirect`（该号已绑到别的实例）、`need_verifycode`（手机要求验证码）。

**3. 连接状态慢 35 秒的直接原因**

`bridge` 的 `connected` 标志由各 provider 在「链路真正可用」时置位。QQ 在 WebSocket `connect` 成功时置位，是即时事件；微信首版照搬成「第一次 `getupdates` 成功返回后置位」——而长轮询恰恰要挂到有新消息才返回，空闲时最长 35 秒。于是界面表现成「明明绑好了却一直未连接」。

**4. 名称未翻译的原因**

通道选择器用的是 `static CHANNEL_LABELS: [&str; 4]`，静态数组里放不下运行时求值的 `localized_str!`；同时 `assets/locales/zh-CN.json` 里只有 `Feishu`/`DingTalk`，没有 `WeChat` 条目。

## 解决方案

**新增 provider：`crates/im_channel/src/wechat.rs`**

- 发送：`POST /ilink/bot/sendmessage`，body 为 `{ msg: { to_user_id, client_id, message_type: 2, message_state: 2, context_token, item_list: [{ type: 1, text_item }] }, base_info }`
- 接收：循环 `POST /ilink/bot/getupdates`，维护 `get_updates_buf` 游标；`ret/errcode = -14` 视为会话过期，返回错误让 bridge 退避并提示重新扫码
- 每个会话保存最新 `context_token`，发送时按 `conversation_id` 取
- 只接受绑定者本人（`owner_id`）的消息，其他发送者丢弃并告警——现有 bridge 是单会话模型，混入他人消息会串台
- 请求头按官方客户端补齐：`AuthorizationType: ilink_bot_token`、`Authorization: Bearer`、`X-WECHAT-UIN`（随机 uint32 的 base64）、`iLink-App-Id`、`iLink-App-ClientVersion`（`major<<16|minor<<8|patch`）

**扫码绑定：改 `registration.rs`**

- 复用 `DeviceFlow`：`qrcode` 作 `device_code`，`qrcode_img_content` 作 `verification_url`（设置页用 `qr::modules` 直接画矩形，OHOS 没有图片管线）
- 处理 `scaned_but_redirect`：记下 `redirect_host`（单槽静态，一次只跑一个扫码），下次轮询切到该主机；`binded_redirect` / `need_verifycode` / `expired` 直接给出可读错误

**扫码结果结构改造：`PollOutcome::Ready`**

把原来的 `client_id` + `client_secret` 两个具名字段改成 `CredentialValues`（`BTreeMap<String, String>`，键与 spec 字段同名）。这样：

- 飞书返回 `app_id`/`app_secret`，钉钉返回 `client_id`/`client_secret`，微信返回 `bot_id`/`bot_token`/`owner_id`/`base_url`，各自按自己的字段名写，不再靠位置猜
- `bind_scanned` 退化成一次直接存储，不需要「取前两个字段」

**连接速度**

进入轮询循环即置 `connected = true`（此时通道已可用，只是没有新消息），请求失败或会话过期再清除。最长 35 秒的空窗降到一个 UI 刷新周期（2 秒）。

**本地化**

- 选择器标签改成 `channel_labels()` + `OnceLock`，微信项走 `localized_str!("WeChat")`
- `assets/locales/zh-CN.json` 增加 `"WeChat":"微信"`；`catalog_sha256` 保持不变，与 `index.json` 仍一致，加载校验通过

**走过的死路**

- 曾打算复用原有双字段模型：把 `bot_token` 塞进 `client_secret`、`ilink_bot_id` 塞进 `client_id`，provider 内再当语义不同的字段用。放弃原因：字段语义与 spec 声明错位，且扫码返回的 `owner_id`、`base_url` 完全没地方存。
- 曾打算把 redirect host 编码进 `device_code` 字符串传回 poll（`PollOutcome` 无法回传新基址）。放弃原因：要在 poll 入口与出口约定一套编码格式，比一个模块级单槽静态更绕。

## 修改文件

- `crates/im_channel/src/wechat.rs` — 新增：iLink provider，长轮询收、带 `context_token` 发、只收文本与本人消息
- `crates/im_channel/src/registration.rs` — 新增 `begin_wechat` / `poll_wechat` 与 iLink 头部、扫码状态机；`PollOutcome::Ready` 改为 `CredentialValues`
- `crates/im_channel/src/binding.rs` — `Platform` 增加 `Wechat`，新增 `WechatCredentials`（token 在 Debug 里脱敏）
- `crates/im_channel/src/platforms.rs` — 注册第四条通道、四个字段（`bot_id`/`bot_token` 必填，`owner_id`/`base_url` 可选）、扫码流程与 provider 构建
- `crates/im_channel/src/manager.rs` — `bind_scanned` 改为按字段名直接存储
- `crates/im_channel/src/lib.rs` — 导出 `wechat` 模块（`target_env = "ohos"` 门控）
- `crates/settings_ui/src/agent_container_settings.rs` — 下拉增加微信项；标签改为可本地化；补齐四个按钮 id 的 `match`
- `assets/locales/zh-CN.json` — 新增 `"WeChat":"微信"`

## 已知限制

- 只解析 `type = 1`（文本）的 item；图片、语音、文件、视频不下载
- `get_updates_buf` 只存在内存，应用重启后可能重放服务端窗口内的旧消息
- `need_verifycode`（手机要求输入验证码）分支直接报错，未做验证码输入 UI
- 一个 bot 只服务绑定者本人，多个微信用户同时聊同一个 bot 的场景不支持

## 验证情况

- `cargo test -p im_channel`：8 个单测通过（含新增的消息解析用例）
- `./script/bundle-ohos`：`EXIT=0`，`HAP BUILD SUCCESSFUL`
- 设备实测：扫码 → `wechat login confirmed` → `scan for wechat bound` → `started for wechat` / `running for wechat`
- **未验证**：端到端收发（手机发消息 → agent 面板 → 回复落回微信）尚无日志实证
