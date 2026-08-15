# Changelog

本文件记录 Aether 每个版本的变更。格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。

## 写法规范

### 分类

每个版本下按以下分类组织条目，无内容的分类省略：

| 分类 | 含义 | 示例 |
|---|---|---|
| **Added** | 新功能 | 新增市场监控模块 |
| **Changed** | 已有功能的变更 | 配额面板改为美元显示 |
| **Fixed** | 缺陷修复 | 修复账号删除后未清理用量记录 |
| **Removed** | 移除的功能 | 移除旧版本地时间格式 |
| **Security** | 安全相关修复 | 修复代理认证绕过 |

### 条目写法

- 用一句话描述用户可感知的变更，不写内部实现细节。
- 以动词开头：新增、修复、移除、优化、重构。
- 涉及多个模块的拆分到各自条目，不要合并成一条。
- 破坏性变更在条目末尾标注 `**[Breaking]**`。
- 关联 commit 时可在括号中附短 hash：`(6331632)`。

### 版本标题格式

```markdown
## [X.Y.Z] - YYYY-MM-DD
```

- 日期为发版日期，不是开发完成日期。
- 未发布的变更放在 `## [Unreleased]` 下。
- 预发布版本同样标注：`## [0.1.0-alpha.2] - 2026-08-15`。

---

## [Unreleased]

## [0.1.0-alpha.21] - 2026-08-15

### Added

- 新增账号锁定与解锁，批量移除报错上游时自动保留已锁定账号
- 新增店铺手动刷新进度展示，显示已完成数量和当前店铺
- 新增 Team 取号页面与订单导入流程

### Changed

- 对齐 Codex OAuth 官方额度窗口，区分官方用量百分比与本机 API 计价统计
- 按现代 iOS 风格统一工作区、卡片、表格、设置、弹窗和交互动效

### Fixed

- 修复 OpenAI 授权新 Tab 白屏、原生 WebView 残留遮挡其他 Tab 的问题
- 修复 WebView 创建或切换失败后主界面失去焦点、其他 Tab 无法点击的问题

## [0.1.0-alpha.20] - 2026-08-14

### Fixed

- 修复内置网页标签访问联动小铺站点时因 ESA 回源请求头缺失而返回 520 的问题
- 修复店铺监控抓取联动小铺商品接口时持续返回 520、无法刷新数据的问题

## [0.1.0-alpha.19] - 2026-08-13

### Added

- 新增 Codex OAuth 设备指纹收敛设置，支持关闭、设备级、会话级和完整模式

### Changed

- 统一 HTTP、HTTP/SSE 桥接与原生 WebSocket 的 Codex 会话指纹和轮次标识
- 扩展 Responses 用量解析，兼容 `data.usage` 与 `data.response.usage`

### Fixed

- 修复 Responses 消息、推理和工具调用项目 ID 前缀不符合 Codex 要求的问题
- 修复上游返回空 `response.completed`/`response.done` 时未及时切换账号的问题
- 修复上游 HTML 403 被错误记为账号级故障的问题
- 修复 Responses 探测将失败或 `max_output_tokens` 截断响应误判为不支持的问题

## [0.1.0-alpha.18] - 2026-08-12

### Added

- 新增 OAuth Codex 模型列表同步，并在账号编辑中提供同步入口
- 新增按会话 ID 对不同中转站进行稳定分流，单中转站容量满时自动切换候选站

### Changed

- WebSocket 长连接改为按每个 `response.create` 计入并释放并发容量
- 原生 WebSocket 与 HTTP/SSE 桥接增加首个响应事件超时和慢上游路由保护
- 同一中转站的多个 API Key 共享中转站级并发槽位，避免绕过上游单并发限制

### Fixed

- 修复 OAuth Responses 的 `truncation: disabled` 请求被上游拒绝，并明确提示不兼容的 `auto`
- 修复 OAuth HTTP、HTTP/SSE 桥接和 WebSocket 握手返回 401 后未自动刷新 Token 重试
- 修复 WebSocket 多轮请求长期占用并发槽位和上一轮未完成时覆盖响应观察器的问题
- 修复大上下文 WebSocket 请求首包过大导致连接失败，自动使用 HTTP/SSE 桥接并保留完整上下文
- 增加超过 15 MiB 请求体的日志告警和传输保护，不隐式删除会话历史

## [0.1.0-alpha.17] - 2026-08-12

### Added

- 新增 Codex 提示词预设管理，可导入、编辑并激活 `AGENTS.md`
- 新增 Codex Skills 管理，可扫描、刷新并可逆启停本地 Skills
- 新增客户端 WebSocket 到中转站 HTTP/SSE 的协议桥接，并在请求日志中单独标记

### Changed

- 自定义 API Key 中转站默认按 HTTP/SSE Responses 协议连接，SOCKS5/SOCKS5H 仅负责其网络出站；OAuth 与官方 OpenAI API Key 继续使用原生 WebSocket
- WebSocket 转中转站 HTTP/SSE 时保留 Codex 会话、路由与客户端身份头，按中转站 Responses 协议完成连续请求
- 店铺监控改为仅由用户手动刷新，不再后台自动采集
- 更新默认店铺清单，并为已有安装增量加入新增店铺
- 优化 Codex 扩展布局，提示词与 Skills 使用紧凑的内部滚动区域

### Fixed

- 修复 OpenAI 授权页创建未受管理的 `about:blank` WebView 后遮挡其他标签页点击的问题
- 修复店铺手动刷新将空计划时间读取为字符串时出现 `invalid type: null` 的问题
- 修复中转站返回 `no available account` 时未按瞬时容量降载切换渠道的问题

## [0.1.0-alpha.16] - 2026-08-10

### Added

- 支持为 Codex 内置图片生成配置独立的 Base URL 和 OpenAI API Key，兼容官方 OpenAI 与中转站
- 在设置页合并 Codex 配置，并兼容旧版配置迁移
- 为 WebView 标签页增加刷新操作

### Fixed

- 修复图片生成请求未按独立上游配置转发的问题
- 修复 Ctrl+R 刷新主界面后原生 WebView 标签页失去焦点、无法点击的问题
- 修复授权页通过 `about:blank` 初始弹窗打开时白屏的问题
- 修复切换 WebView 标签页后页面未重新获得焦点的问题

## [0.1.0-alpha.15] - 2026-08-10

### Fixed

- 修复大型 Responses 请求的 SSE 前导事件超过 64 KiB 后被误判为上游失败的问题
- 收到 `response.created` 或 `response.in_progress` 后立即向客户端提交响应，避免中转站已计费但结果被丢弃并触发重复请求
- 将分段 SSE 前导事件安全上限提高到 8 MiB，兼容大型上下文和工具 Schema

## [0.1.0-alpha.14] - 2026-08-10

### Fixed

- 修复 WebSocket 首帧发送阶段偶发的 502/10053，失败时自动重建出站代理隧道并重试
- 记录上游 WebSocket 关闭码和关闭原因，避免正常完成后的关闭被误判为错误

## [0.1.0-alpha.13] - 2026-08-10

### Added

- 请求日志直接显示传输协议（WebSocket、HTTP SSE、HTTP）和出站代理方式
- 为旧请求日志自动迁移可识别的传输类型，无法追溯的代理方式标记为未知

### Fixed

- 修复 WebSocket 与 HTTP SSE 请求只能通过状态码判断、无法确认实际出站路径的问题

## [0.1.0-alpha.12] - 2026-08-10

### Changed

- Codex 接管默认启用 Responses WebSocket，并在应用启动时自动升级已有 Aether 接管配置

### Fixed

- 修复 Codex WebSocket 拒绝默认 HTTP/mixed-port 出站代理的问题，新增标准 HTTP CONNECT 隧道支持
- 修复旧接管配置保留过期地址、访问令牌或关闭 WebSocket 开关的问题

## [0.1.0-alpha.11] - 2026-08-09

### Added

- 新增官方 Codex Responses WebSocket 代理，支持通过 SOCKS5/SOCKS5H 出站代理连接上游
- 接管 Codex 时启用 Aether provider 的 WebSocket 能力

### Fixed

- 修复 Codex WebSocket 的账号路由、OAuth 令牌续期、响应事件转发和用量观察
- 支持通过 WebSocket 转发 `response.cancel` 取消帧

## [0.1.0-alpha.10] - 2026-08-08

### Fixed

- 修复手动出站代理复用旧 CONNECT 隧道导致上游间歇性返回 502 的问题

## [0.1.0-alpha.9] - 2026-08-08

### Added

- 新增请求日志上游响应模型审计，记录响应声明模型并支持筛选模型不一致的请求

### Changed

- 将 Codex 默认出站身份迁移为 `codex-tui`，保持 UA 首尾版本号与 `originator`、`version` 同步
- 将用量费用按数据库精度量化，避免累计金额出现浮点微小偏差

### Fixed

- 修复 WebView Tab 未显式设置下载目标导致文件无法落盘、下载完成后无法触发监听的问题
- 修复 Responses 工具 Schema 显式 `parameters.type: null` 导致上游 400 并在历史请求中反复失败的问题
- 修复 Codex 流前导帧后容量降载无法故障转移的问题；已产生输出时改写为客户端可重试错误码
- 修复上游域名或 IP 不可达时建连阻塞过久的问题，TCP/DNS 建连与 TLS 握手超时统一为 10 秒

## [0.1.0-alpha.8] - 2026-08-07

### Added

- 新增 Codex 官方稳定版自动同步，每 6 小时刷新出站客户端版本，并支持在设置中关闭或手动同步
- 新增 OpenAI 账号重置额度次数与到期时间展示，缓存读取时自动剔除已过期额度

### Changed

- 统一 OAuth 转发、额度查询、账号检测、授权兑换与令牌刷新使用的 Codex User-Agent、originator 和版本来源
- 优化渠道监控慢请求判定，将总耗时达到 20 秒的成功请求计为异常，并同时展示首包与总耗时

### Fixed

- 修复流内 `server_is_overloaded` 与 `slow_down` 被误判为账号故障并触发渠道冷却的问题，改为同账号有界重试后再切换渠道

## [0.1.0-alpha.7] - 2026-08-03

### Added

- 新增 API Key 中转站模型验真：手动执行模型声明、结构化输出、工具调用和多轮指令动态探针，保存检测历史并给出正常、可疑、高风险或无法判断的风险评分

### Changed

- 扩展渠道监控卡片与详情，展示最近验模模型、响应模型、有效探针、Token、耗时和逐项证据
- 明确模型验真属于黑盒风险检测，结果用于发现常见降级和错误路由，不作为模型身份的绝对证明

### Fixed

- 修复 OpenAI 内置授权页绕过出站代理的问题，并让授权流程新开的登录窗口继承同一代理与 OAuth 会话来源

## [0.1.0-alpha.6] - 2026-08-03

### Added

- 新增扁平 OAuth JSON 转换导入，支持顶层 Token、ChatGPT Account ID、邮箱和套餐字段，并过滤手机号、密码、授权码与 state

### Changed

- 扩展 Sub2API 兼容备份识别，支持没有 `type` 但带 `exported_at`、`proxies` 和 `accounts` 标记的完整备份
- 优化 GitHub Release 发布流程，自动使用当前标签对应的 CHANGELOG 章节作为发布正文

## [0.1.0-alpha.5] - 2026-08-03

### Added

- 新增已添加账号的编辑入口，可修改名称、优先级、权重、并发数与成本倍率，中转站账号还可更新 API Key、Base URL 和模型白名单
- 新增 OpenAI OAuth 账号额度价值测算，根据当前 7 天周期或 5 小时周期的实际请求费用与用量占比估算账号美元价值

### Changed

- 优化 New API 原始 quota 的近似美元展示与全局汇总，统一按站点 `quota_per_unit` 换算

## [0.1.0-alpha.4] - 2026-08-03

### Added

- 新增 OpenAI 账号手动授权向导：输入名称后自动打开 PKCE 授权链接，浏览器回跳本机回调地址后自动创建账号
- 新增即时生效的 Aether 出站代理设置，默认 `http://127.0.0.1:7890`，用于 OpenAI OAuth 和上游网络请求
- 新增 API Key 中转站成本倍率字段，并兼容导入、导出 Sub2API 的 `rate_multiplier` 与上游倍率同步开关
- 新增可选的 Sub2API 账单倍率同步：开启后每 30 分钟读取中转站的 `/v1/sub2api/billing`，也可手动立即同步
- 新增全局“成本保护”设置，可按最高成本倍率和安全缓冲在路由前排除超限上游
- 新增 New API（QuantumNous/new-api）中转站用量查询：识别站点 `/api/usage/token/` 令牌额度，结合远端日志统计今日/近30天消耗、请求数与最近模型，并按 `/api/status` 的 `quota_per_unit` 换算美元

### Changed

- 将成本保护设计为默认关闭的本地路由条件，未启用时保持既有候选排序和故障切换行为
- 简化 New API 用量面板：仅展示用量与余额（无限额时显示无限制），悬停可查看按站点倍率换算的美元近似值

### Fixed

- 修复中转站用量面板的远端请求、最近模型与最近时间信息在窄卡片下被省略号截断无法阅读的问题

## [0.1.0-alpha.3] - 2026-08-02

### Changed

- 统一 README、官网、应用内提示与项目元数据的能力说明，明确当前上游类型、路由边界、市场监控、内部网页 Tab、数据存储和外部网络访问范围
- 将每日 USD 额度统一表述为本地估算预算，明确它仅作参考展示，超出后剩余显示为 0，不会停用上游或影响路由
- 完善市场商品、店铺、提醒与系统通知说明，明确分页、采集周期、事件留存和通知开关的实际行为

### Fixed

- 修复预发布标签不进入 GitHub 最新正式 Release、导致应用更新器请求 `latest.json` 返回 404 的问题
- 修复导入 Sub2API 的不限并发账号时 `concurrency: 0` 被错误拒绝的问题，并将其映射为本地支持的最大容量
- 修复应用内网页 Tab 打开后无法继续创建子 WebView，以及托盘和系统通知无法恢复主窗口的问题
- 修复“周限 team、速刷号、仅支持 Sub2API 和 CPA”商品未归入 BUG TEAM 的问题

### Security

- 收紧剪贴板自动导入的字段白名单并限制 JSON 结构复杂度，降低非凭据字段被误识别的风险

## [0.1.0-alpha.2] - 2026-08-01

### Added

- 官网首页：深色主题单页站点，含产品截图 Gallery、功能介绍和自动获取最新安装包下载链接
- GitHub Pages 部署 workflow，推送 `docs/` 变更自动发布
- 市场商品按名称去重：同名商品保留最低报价，标注同名报价数和汇总库存
- 店铺 token 输入框增加提示说明（链接最后一段）
- 检查更新操作增加 toast 反馈（已是最新 / 发现新版本 / 检查失败）
- 设置页新增回收站入口和意见反馈 / Bug 报告邮件模板

### Changed

- 市场筛选栏输入框改为底边对齐布局
- CI 添加 pnpm store 缓存，`pnpm install` 使用 `--frozen-lockfile` 加速构建

### Fixed

- 修复 Windows 下市场通知不弹出的问题

## [0.1.0-alpha.1] - 2026-08-01

首个 alpha 版本，核心功能基本完整。

### Added

- 上游管理：统一管理 OpenAI/Codex OAuth 账号与 OpenAI 兼容 API Key 中转站，支持导入、启停、测试、优先级、并发、回收站和批量操作
- 上游检测：支持用户手动连接检测，以及 OAuth 凭据和用量刷新
- 用量追踪：展示 OpenAI/Codex OAuth 的 5 小时/7 天窗口，以及兼容中转站返回的限额、订阅或钱包信息
- 每日预算：按本地估算费用展示全局每日预算、已用和剩余参考值
- 中转 API 代理：提供 OpenAI 兼容格式和流式响应，按模型、优先级、权重与容量路由到匹配上游
- 请求日志：记录真实上游尝试，支持搜索、筛选、分页和实时刷新
- 市场监控：采集链动小铺商品与店铺数据，提供比价、库存行情、提醒、系统通知、退避和熔断保护
- WebView 工作区：通过应用内网页 Tab 打开中转站、商品和店铺，复制或下载内容可生成待确认的导入候选
- 系统托盘：关闭主窗口后驻留托盘，可从托盘恢复窗口或完全退出
- 自定义标题栏：紧凑布局，集成窗口控制按钮和版本徽章
- 版本系统：构建时注入 commit hash 和构建时间，标题栏显示版本号

### Changed

- 时间存储统一为 UTC ISO 8601 格式（`%Y-%m-%dT%H:%M:%SZ`）
- 前端时间格式化收敛到 `src/lib/time.ts` 统一模块
