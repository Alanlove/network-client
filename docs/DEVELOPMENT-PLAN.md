# Network Client 下一轮开发计划与技术方案

> 编写日期：2026-09-14
> 当前状态：V1 核心链路已实现并通过实测（系统代理模式 + Xray/sing-box 内核 + 三态健康 + 崩溃恢复）
> 本文档整合：one-ip 融合计划 + Clash Verge / v2box 功能参考 + 二轮审计清单 + 开发路线

---

## 1. 现状基线（已完成且实测通过的能力）

| 能力 | 状态 | 实现位置 |
|---|---|---|
| 双进程架构（GUI + Rust Service） | ✅ | WinUI 3 + tokio |
| Named Pipe + Protobuf IPC | ✅ | ipc/server.rs, frame.rs |
| 连接状态机（9 态 + 热切换） | ✅ | app/state.rs, lifecycle.rs |
| 系统代理模式（WinINET 注册表直写） | ✅ | route/manager.rs |
| Xray / sing-box 内核适配 | ✅ | proxy/adapter.rs, manager.rs, xray_builder.rs |
| 三态健康监督（Healthy/LocalDead/UplinkDead） | ✅ | health/monitor.rs, proxy/manager.rs |
| 崩溃恢复（孤儿 reap + 持锁恢复 + 代理还原） | ✅ | app/lifecycle.rs |
| 订阅管理（下载/解码/解析/原子落盘） | ✅ | subscription/ |
| 节点评分（EWMA + 加权 + 迟滞切换） | ✅ | nodes/health.rs |
| 分流规则（domain/IP/process/GeoIP） | ✅ | routing/engine.rs |
| 流量统计（SQLite + 2s 刷盘） | ✅ | statistics/ |
| 诊断引擎（网络/DNS/代理/丢包/测速） | ✅ | diagnostics/engine.rs |
| DPAPI 加密订阅 URL | ✅ | config/crypto.rs |
| 5 个 GUI 页面（首页/节点/规则/诊断/设置） | ✅ | client/Views/ |

---

## 2. 能力差距分析

### 2.1 对比 Clash Verge（成熟桌面代理客户端）

| 功能 | Clash Verge | 本项目 | 差距 |
|---|---|---|---|
| 活跃连接面板（实时 TCP 连接列表 + 进程） | ✅ | ❌ | 服务端无连接查询 IPC；客户端无 Connections 页面 |
| 实时流量图表（折线图） | ✅ | 仅数字 | 客户端只有文本速率，无图表 |
| 实时日志查看器 | ✅ | ❌ | 服务端有 tracing 日志文件，但无 IPC 日志流 |
| 多配置文件管理（Profile 切换） | ✅ | ❌ | 只有单一 app.json + routing.json，无多 profile |
| 规则测试（输入 URL 看匹配哪条规则） | ✅ | ❌ | 服务端有 routing engine，但无 `routing.test` IPC 方法 |
| 系统托盘（最小化到托盘 + 快捷操作） | ✅ | ❌ | 无托盘图标 |
| 深色/浅色主题切换 | ✅ | 跟随系统 | WinUI 3 默认跟随系统，但无手动切换 |
| 内核外部控制器 API | ✅ | N/A | 本项目不暴露 REST API（设计决策，非缺陷） |
| TUN 模式 | ✅ | 预留 | tun/ 模块存在但 enabled=false，未启用 |
| 全局/规则/直连模式快捷切换 | ✅ | ✅ | RulesPage 已有模式按钮 |

### 2.2 对比 v2box（轻量客户端）

| 功能 | v2box | 本项目 | 差距 |
|---|---|---|---|
| 极简一键连接 | ✅ | ✅ | 首页已有 |
| 节点分组/排序 | ✅ | 基础 | NodesPage 有列表，无分组/多列排序 |
| 订阅自动更新 | ✅ | ✅ | 已修复，开机即跑 |
| 节点延迟排序 | ✅ | ✅ | batchTest 有 |
| 简洁 UI | ✅ | ✅ | WinUI 3 原生 |

### 2.3 对比 one-ip（网络诊断工具箱，可融合）

one-ip 是一个 React + Cloudflare Workers 的 Web 应用，提供 IP 查询/网络诊断/浏览器检测/AI 服务状态。**它与本项目不冲突，其诊断能力可以融合进客户端的 Diagnostics 页面**：

| one-ip 功能 | 融合价值 | 融合方式 |
|---|---|---|
| IP 信誉/风险评估（score 0-100 + flags: 住宅/IDC/VPN/Tor/滥用） | **高**：连接后显示当前出口 IP 的信誉分，帮助用户判断是否被标记 | 服务端 `diagnostics.proxy` 增加出口 IP 信誉查询（调 one-ip `/api/ip/health?format=json` 或自建类似逻辑） |
| 全球 Ping（Globalping API，多地区探针） | **中**：诊断页增加"全球延迟分布" | 服务端新增 `diagnostics.globalPing` IPC 方法，调 Globalping API |
| 网站连通性检测（多轮 HTTP 采样 + 中位耗时） | **高**：诊断页增加"网站可达性"批量检测 | 服务端已有 `diagnostics.proxy`，扩展为多目标采样 |
| AI 服务可达性（ChatGPT/Claude/Gemini 等） | **高**：连接后一键检测哪些 AI 服务可用 | 服务端新增 `diagnostics.aiCheck` IPC 方法 |
| DNS/CDN 检测 | **中**：诊断页已有 DNS 测试，可补充 CDN 节点信息 | 扩展现有 `diagnostics.dns` |
| WHOIS/RDAP | **低**：过于专业，可放后期 | 暂不融合 |
| 浏览器指纹检测 | **不融合**：与代理客户端定位不符 | — |

---

## 3. 二轮审计清单（开发前必须完成）

> 以下审计项在第一轮健壮性修复中已覆盖一部分，但新增功能开发前需重新验证。

### 3.1 已审计且通过的（保持有效）

- [x] 连接状态机完整性（9 态 + 白名单迁移）
- [x] IPC panic 隔离 + 帧编解码
- [x] 系统代理备份/还原（三值逐字还原 + 精确端点匹配）
- [x] 崩溃恢复（孤儿 reap + 持锁恢复 + 失败路径清理）
- [x] 订阅管线（下载→解码→解析→原子落盘，失败保留旧节点）
- [x] DPAPI 加密 + BOM 剥离 + JSON 解析损坏告警
- [x] 三态健康监督（supervise 自循环，无递归 future）
- [x] 节点评分 EWMA + 迟滞切换

### 3.2 本轮需新增审计的

| 审计项 | 关注点 | 风险等级 |
|---|---|---|
| **TUN 模块** | tun/ 目前是 dead code（enabled=false），启用前需审计 Wintun 集成、包读写、IPv6、Loop Prevention | 高 |
| **routing engine 实际匹配** | engine.rs 有完整的规则链但从未在运行中被触发（当前只有 global/smart/direct 三模式，smart 模式的实际规则匹配路径未经真机验证） | 高 |
| **GeoIP 数据库** | routing/geoip.rs 引用 geo 数据库但项目内无 mmdb 文件，需确认来源和更新机制 | 中 |
| **DNS 模块实际使用** | dns/ 有完整实现但当前内核内置 DNS 为主，服务自身 DNS 模块在运行链路中的实际角色需确认 | 中 |
| **common.rs L97 unwrap** | 第一轮标记但未修的 unwrap，新增功能可能触发 | 低 |
| **dns/wire.rs, resolver.rs, cache.rs** | 第一轮未细读，新增 DNS 功能前需审计 | 中 |
| **subscription/downloader.rs** | 第一轮未细读，新增订阅功能前需审计 | 低 |
| **statistics 采样精度** | 当前 2s 采样，新增流量图表需要更高频次（500ms-1s），需评估 SQLite 写入压力 | 中 |

---

## 4. 下一轮开发计划

### Phase 1：用户体验补齐（参考 Clash Verge / v2box）

**目标**：让客户端从"能用"变成"好用"，补齐代理客户端标配功能。

#### P1-1 系统托盘 + 最小化

**范围**：
- 最小化到系统托盘（NotifyIcon）
- 托盘右键菜单：连接/断开/切换模式/退出
- 托盘图标反映连接状态（绿=已连接 / 灰=未连接 / 黄=恢复中）
- 双击托盘恢复窗口

**技术方案**：
- WinUI 3 的 `AppWindow.Closing` 事件拦截关闭操作，改为 Hide
- 使用 H.NotifyIcon 或 Win32 Shell_NotifyIcon 实现 tray
- 托盘菜单调 IPC `connection.connect/disconnect` + `routing.setMode`
- 服务已有 `connection.stateChanged` 事件驱动托盘图标更新

**影响**：仅客户端改动，不涉及服务端

#### P1-2 实时流量图表

**范围**：
- 首页增加上传/下载速率折线图（最近 60s 滚动窗口）
- 历史流量按日/周/月柱状图

**技术方案**：
- 客户端：WinUI 3 + Win2D 或开源 LineChart 控件
- 数据源：已有 `traffic.updated` 事件（2s 推送）
- 历史数据：已有 `statistics.getToday` IPC，扩展 `statistics.getRange(start, end)`
- 服务端：statistics/manager.rs 采样频率从 2s 改为 1s，增加 `get_range` 查询

**影响**：服务端 statistics 模块小改 + 客户端新增图表组件

#### P1-3 活跃连接面板

**范围**：
- 新增 Connections 页面：实时 TCP 连接列表
- 每条连接显示：源/目标地址、端口、进程名、上传/下载字节、连接时长、匹配的规则
- 支持搜索/过滤/关闭连接

**技术方案**：
- 服务端：利用内核 API（Xray API `StatsService.QueryStats` / sing-box API `connection.QueryConnections`）
- 新增 IPC：`connection.list` / `connection.close`
- 新增事件：`connection.listChanged`（定期推送 diff）
- 进程信息：用 Win32 `GetExtendedTcpTable` + `GetModuleFileNameEx` 从 PID 获取进程名
- 客户端：新页面 ConnectionsPage + ConnectionsViewModel

**影响**：服务端新增 connection 模块 + 客户端新增页面

#### P1-4 实时日志查看器

**范围**：
- 设置或诊断页增加"内核日志"区域
- 实时滚动显示 Xray/sing-box 输出
- 支持级别过滤（INFO/WARN/ERROR）和搜索

**技术方案**：
- 服务端：proxy/manager.rs 已捕获内核 stdout/stderr，增加 IPC `core.getLogs` + 事件 `core.log`
- 客户端：诊断页增加日志面板，订阅 `core.log` 事件

**影响**：服务端 proxy 模块小改 + 客户端诊断页扩展

#### P1-5 深色/浅色主题切换

**范围**：
- 设置页增加主题选择：跟随系统/深色/浅色
- 持久化到 app.json

**技术方案**：
- WinUI 3：`Application.Current.RequestedTheme = ApplicationTheme.Dark/Light`
- 设置页新增 Theme 选项，写入 `app.json` 的 `ui.theme` 字段
- 服务端 config/schema.rs 新增 `ui` 段

**影响**：仅客户端改动 + 配置 schema 小扩展

#### P1-6 规则测试

**范围**：
- 规则页增加"规则测试"输入框
- 输入 URL/域名/IP，显示匹配的规则和最终决策

**技术方案**：
- 服务端：routing/engine.rs 新增 `test_match(domain_or_ip)` 公开方法
- 新增 IPC：`routing.test`（payload = { input: string }，返回 { matched_rule, decision, chain }）
- 客户端：RulesPage 增加测试输入框 + 结果展示

**影响**：服务端 routing 模块小改 + 客户端 RulesPage 扩展

---

### Phase 2：诊断能力升级（融合 one-ip）

**目标**：把 one-ip 的网络诊断能力融合进客户端，让用户连接后一眼看到出口质量。

#### P2-1 出口 IP 信誉检测

**范围**：
- 首页连接成功后自动检测出口 IP 信誉
- 显示：出口 IP、地理位置、ISP、信誉分(0-100)、风险标签（住宅/IDC/VPN/Tor/滥用）
- 诊断页可手动查询任意 IP

**技术方案**：
- 服务端：新增 `diagnostics.ipHealth` IPC 方法
- 数据源：调 `https://ip-api.com/json/` 获取出口 IP + 地理（已验证可用）
- 信誉分：调 one-ip `/api/ip/health?ip=<exit_ip>&format=json`（自部署或用公共实例）
- 或自建逻辑：基于 ASN 类型（IDC vs 住宅）、已知 VPN/Tor 出口列表判断
- 事件：`diagnostics.ipHealthResult`

**one-ip 融合点**：直接复用其 `/api/ip/health` 端点，或 fork 其 Worker 代码自部署

**影响**：服务端 diagnostics 模块新增 ip_health 模块 + 客户端首页/诊断页扩展

#### P2-2 AI 服务可达性检测

**范围**：
- 诊断页增加"AI 服务检测"卡片
- 一键检测：ChatGPT / Claude / Gemini / Grok / Perplexity / DeepSeek / 通义千问 / Kimi
- 每个服务显示：可达 / 不可达 / 延迟
- 结果帮助用户选择合适的节点

**技术方案**：
- 服务端：新增 `diagnostics.aiCheck` IPC 方法
- 对每个 AI 服务发 HTTP HEAD/GET（8s 超时），通过代理隧道
- 检测 URL（轻量化，只检测可达性不检测功能）：
  - ChatGPT: `https://chat.openai.com/cdn-cgi/trace`（200=可达）
  - Claude: `https://claude.ai/`（200=可达）
  - Gemini: `https://gemini.google.com/`（200/302=可达）
  - 其他同理
- 并发检测（tokio::join_all），15s 总超时
- 返回 `[{ service, reachable, latency_ms, http_code }]`

**one-ip 融合点**：参考 one-ip 的 AI 服务列表和检测逻辑

**影响**：服务端 diagnostics 模块新增 ai_check 模块 + 客户端诊断页扩展

#### P2-3 全球延迟分布

**范围**：
- 诊断页增加"全球延迟"视图
- 从多个地区（亚洲/欧洲/北美/南美/非洲）探针测当前出口的可达性

**技术方案**：
- 服务端：新增 `diagnostics.globalPing` IPC 方法
- 数据源：调 Globalping API `https://api.globalping.io/v1/measurements`
- POST `{ "target": "<exit_ip_or_domain>", "type": "ping", "locations": [{ "continent": "AS" }, ...] }`
- 轮询 measurement 结果，30s 超时
- 返回 `[{ region, city, rtt_ms, loss }]`

**one-ip 融合点**：直接使用 Globalping API（one-ip 也是这样做的）

**影响**：服务端 diagnostics 模块新增 global_ping 模块 + 客户端诊断页扩展

#### P2-4 网站连通性批量检测

**范围**：
- 诊断页增加"网站可达性"批量检测
- 预设列表：Google / YouTube / GitHub / Twitter / Wikipedia / Netflix / Disney+ / Spotify
- 每个显示：可达 / 延迟 / 出口 IP（验证是否走了代理）

**技术方案**：
- 扩展现有 `diagnostics.proxy`：从单目标改为多目标并发采样
- 每个目标 3 轮 HTTP HEAD，取中位延迟
- 返回 `[{ url, reachable, latency_ms, exit_ip }]`

**影响**：服务端 diagnostics/engine.rs 扩展 + 客户端诊断页扩展

---

### Phase 3：TUN 模式启用（高难度，需二轮审计后进行）

**目标**：启用 TUN 虚拟网卡模式，实现全局透明代理（不依赖系统代理）。

**前置条件**：完成 §3.2 的 TUN 模块审计。

#### P3-1 Wintun 集成

**范围**：
- 集成 wintun.dll（置于 data/bin/）
- 实现 TUN adapter 生命周期：create / configure IP / destroy
- MTU 1500（IPv4）/ 1280（IPv6）

**技术方案**：
- Rust 绑定：wintun crate 或直接 FFI
- IP 分配：TUN adapter 配 172.19.0.1/30
- Route：默认路由指向 TUN（0.0.0.0/1 + 128.0.0.0/1，避免覆盖 existing default route metric）

#### P3-2 Bypass Manager（防回环）

**范围**：
- 代理服务器 IP 必须走物理网卡，不能走 TUN
- DNS 服务器 IP 不能走 TUN
- 内网地址（10/172.16-31/192.168）不走 TUN

**技术方案**：
- 新增 `route/bypass.rs`：维护 bypass IP 列表
- 用 `route add <proxy_ip> mask 255.255.255.255 <physical_gateway>` 显式路由
- 连接时自动添加，断开时自动删除

#### P3-3 IPv6 支持

**范围**：
- TUN adapter 同时配 IPv6 地址
- IPv6 默认路由指向 TUN
- 确保不会 IPv4 走代理、IPv6 直连泄露

---

### Phase 4：Kill Switch + 高级安全

#### P4-1 Kill Switch

**范围**：
- 代理断开时自动阻断非代理流量（防火墙策略）
- 可独立开关

**技术方案**：
- 新增 `security/kill_switch.rs` 模块
- Windows Firewall API：代理连接时添加 "allow all" 规则，断开时改为 "block all except local"
- 或用 WFP（Windows Filtering Platform）更底层

#### P4-2 DNS 泄露防护

**范围**：
- 确保所有 DNS 查询走代理隧道，不走系统 DNS
- DoH 全局强制

---

### Phase 5：多配置文件 + 自动更新

#### P5-1 Profile 管理

**范围**：
- 支持多套配置文件（不同订阅组合 / 不同路由策略）
- 一键切换 Profile

#### P5-2 应用自动更新

**范围**：
- 检查新版本 → 下载 → 验证签名 → 停服务 → 替换 → 重启

---

## 5. 开发优先级与依赖关系

```
Phase 1（用户体验补齐）
  ├─ P1-1 系统托盘        ──┐
  ├─ P1-2 流量图表         ──┤
  ├─ P1-5 主题切换         ──┤── 无相互依赖，可并行
  ├─ P1-6 规则测试         ──┘
  ├─ P1-4 日志查看器       ──── 依赖：P1-1（托盘也可显示日志入口）
  └─ P1-3 活跃连接面板     ──── 依赖：内核 API 适配（Xray StatsService / sing-box API）

Phase 2（诊断升级，融合 one-ip）
  ├─ P2-1 出口 IP 信誉     ──┐
  ├─ P2-2 AI 服务检测       ──┤── 无相互依赖，可并行
  ├─ P2-3 全球延迟         ──┤
  └─ P2-4 网站连通性       ──┘

Phase 3（TUN，依赖 Phase 1 完成）
  ├─ P3-1 Wintun 集成      ──── 依赖：§3.2 TUN 审计通过
  ├─ P3-2 Bypass Manager   ──── 依赖：P3-1
  └─ P3-3 IPv6             ──── 依赖：P3-1, P3-2

Phase 4（安全，依赖 Phase 3）
  ├─ P4-1 Kill Switch      ──── 依赖：P3-1（TUN 启用后才有意义）
  └─ P4-2 DNS 泄露防护     ──── 依赖：P3-1

Phase 5（配置 + 更新，独立）
  ├─ P5-1 Profile 管理     ──── 独立
  └─ P5-2 自动更新         ──── 独立
```

**建议执行顺序**：P1-1 → P1-2 → P1-5 → P1-6 → P1-4 → P1-3 → P2-1 → P2-2 → P2-4 → P2-3 → 审计 TUN → P3

---

## 6. IPC 协议扩展清单

新增的 IPC 方法（追加到现有方法面）：

```
// Phase 1
connection.list          // 活跃 TCP 连接列表
connection.close         // 关闭指定连接
core.getLogs             // 获取内核日志
routing.test             // 规则测试（输入 URL → 输出匹配链）

// Phase 2
diagnostics.ipHealth     // 出口 IP 信誉检测
diagnostics.aiCheck      // AI 服务可达性
diagnostics.globalPing   // 全球延迟分布

// Phase 3
tun.getStatus            // TUN 状态
tun.setEnabled           // 启用/禁用 TUN（仅设置）

// Phase 4
security.getKillSwitch   // Kill Switch 状态
security.setKillSwitch   // 开关 Kill Switch

// Phase 5
profile.list             // 配置文件列表
profile.switch           // 切换配置文件
profile.create           // 创建新配置
profile.delete           // 删除配置
```

新增事件：

```
connection.listChanged   // 活跃连接变化
core.log                 // 内核日志行
diagnostics.ipHealthResult
diagnostics.aiCheckResult
diagnostics.globalPingResult
```

---

## 7. one-ip 融合技术细节

### 7.1 直接复用

| one-ip 能力 | 融合方式 | 实现路径 |
|---|---|---|
| `/api/ip/health` IP 信誉 | 服务端调 HTTP GET | `diagnostics/ip_health.rs` → `reqwest::get("https://ip.huzhihui.com/api/ip/health?ip={ip}&format=json")` |
| Globalping 全球探针 | 服务端调 API | `diagnostics/global_ping.rs` → POST `https://api.globalping.io/v1/measurements` |
| AI 服务列表 + 检测逻辑 | 参考其检测 URL 和判断逻辑 | `diagnostics/ai_check.rs` → 对每个 AI 平台发 HTTP 探测 |
| 网站连通性多轮采样 | 参考其 HTTP 采样 + 中位数逻辑 | 扩展 `diagnostics/engine.rs` → 多目标 3 轮 HEAD |

### 7.2 不融合的部分

| one-ip 能力 | 不融合原因 |
|---|---|
| 浏览器指纹检测 | 与代理客户端定位不符 |
| WHOIS/RDAP | 过于专业，投入产出比低 |
| 服务状态聚合 | 面向 Web 用户，代理客户端不需要 |
| Cloudflare Workers 部署 | 本项目是桌面应用 |

### 7.3 可选：内嵌 one-ip Web 面板

如果用户需要完整的 IP 诊断能力，可以在客户端内嵌一个 WebView 加载 one-ip（本地部署或公共实例）：
- 客户端增加一个"IP 工具箱"页面
- WebView2 加载 `https://ip.huzhihui.com/`（或自部署实例）
- WebView 的网络流量自动走系统代理（即走代理隧道）

**优点**：零开发成本，直接获得 one-ip 全部功能
**缺点**：依赖外部服务，离线不可用

---

## 8. Clash Verge 参考功能融合清单

| Clash Verge 功能 | 融合方式 | 优先级 |
|---|---|---|
| 活跃连接面板 | P1-3：服务端调内核 API 获取连接列表 | 高 |
| 实时流量图表 | P1-2：复用已有 traffic 事件 + Win2D | 高 |
| 日志查看器 | P1-4：复用已有内核 stdout 捕获 | 中 |
| 系统托盘 | P1-1：Win32 Shell_NotifyIcon | 高 |
| 深色主题 | P1-5：WinUI 3 RequestedTheme | 低 |
| 规则测试 | P1-6：routing engine 已有匹配逻辑，暴露 test 接口 | 中 |
| 多 Profile | P5-1：扩展 config 模块 | 低 |
| TUN 模式 | P3：启用预留的 tun/ 模块 | 高（但难度大） |
| 全局/规则/直连切换 | ✅ 已有 | — |
| 节点 URL scheme 导入 | 可参考：解析 `clash://` / `sing-box://` scheme | 低 |

---

## 9. v2box 参考功能融合清单

| v2box 功能 | 融合方式 | 优先级 |
|---|---|---|
| 节点分组（按地区/订阅） | NodesPage 增加分组折叠 | 中 |
| 节点排序（延迟/名称/评分） | NodesPage 增加排序选项 | 中 |
| 订阅二维码分享 | 可选：生成节点分享二维码 | 低 |
| 简洁模式（精简 UI） | 可选：设置页增加"精简模式"开关 | 低 |

---

## 10. 技术风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| TUN 启用后系统不稳定 | 蓝屏/网络中断 | 先在虚拟机验证；保留一键回退到系统代理模式 |
| 内核 API 差异（Xray vs sing-box） | 活跃连接面板需要两套适配 | proxy/adapter.rs 已有 trait 抽象，新增 `get_connections()` 方法到 trait |
| Globalping API 限流 | 全球延迟检测失败 | 加本地缓存（5min TTL）；失败降级为 ICMP ping |
| one-ip 公共实例不可用 | IP 信誉检测失败 | 可选自部署；失败时降级为 ip-api.com 地理信息 |
| SQLite 高频写入 | 流量图表 1s 采样增加写入压力 | 内存计数器 + 批量 flush（已有策略）；或改用 mmap |
| WinUI 3 图表性能 | 实时折线图卡顿 | 用 Win2D Canvas 而非 XAML Shape；限制 60s 窗口 |
| 进程信息获取权限 | 活跃连接面板拿不到进程名 | 需要管理员权限或按 PID 查 Toolhelp32 |

---

## 11. 二轮审计执行计划

在开始 Phase 1 开发前，需先完成以下审计：

### 审计步骤

1. **routing engine 真机验证**（smart 模式实际触发规则匹配）
   - 连接 smart 模式 → 访问已知域名 → 检查是否走了对应规则
   - 验证 Domain Trie / IP CIDR Tree / Process HashMap 实际构建

2. **TUN 模块代码审计**
   - 逐行审计 tun/manager.rs, adapter.rs, packet.rs
   - 确认 Wintun 绑定方式、包读写循环、错误处理
   - 在虚拟机中试用 `tun.enabled=true` 验证是否可启动

3. **GeoIP 数据来源确认**
   - 确认 routing/geoip.rs 使用的 mmdb 文件来源
   - 确认更新机制（内置 vs 在线下载）

4. **DNS 模块运行时角色**
   - 确认 dns/ 模块在当前运行链路中是否被实际调用
   - 如果内核内置 DNS 为主，确认 dns/ 模块的定位

5. **剩余 unwrap/expect 扫描**
   - common.rs L97
   - dns/wire.rs, resolver.rs, cache.rs
   - subscription/downloader.rs

6. **statistics 采样压力测试**
   - 当前 2s 采样 → 改 1s 后 SQLite 写入压力
   - 验证 24h 运行后数据库大小和查询性能

---

## 12. 总结

当前 V1 核心链路已通过两轮健壮性修复 + 真机回归，系统代理模式 + 三态健康 + 崩溃恢复能力扎实。

下一轮开发重点：

1. **Phase 1（用户体验）**：补齐代理客户端标配功能（托盘/图表/连接面板/日志/主题/规则测试），让产品从"能用"到"好用"
2. **Phase 2（诊断升级）**：融合 one-ip 的 IP 信誉/AI 检测/全球延迟/网站连通性，做出差异化诊断能力
3. **Phase 3（TUN）**：启用全局透明代理（需先完成二轮审计）
4. **Phase 4（安全）**：Kill Switch + DNS 泄露防护
5. **Phase 5（配置+更新）**：多 Profile + 自动更新

**开发前必须完成二轮审计**（§3.2 + §11），重点是 routing engine 真机验证和 TUN 模块审计。

---

## 13. 开发进度记录（2026-09-14 持续更新）

### 二轮审计结论（已完成）

6 项审计全部完成，编译 0 error + 25 单测全绿：

| 审计项 | 结论 |
|---|---|
| routing engine | `decide()` 在生产链路未被调用（系统代理模式由内核路由），TUN 前需接入数据面 |
| TUN 模块 | 仅骨架（无包读写循环/无 bypass/无路由操作/无 IPv6），Phase 3 需大量开发 |
| GeoIP | `data/cache/geoip/` 不存在，smart 模式"本国直连"失效；推荐 APNIC CN CIDR 方案 |
| DNS 模块 | 仅用于诊断（设计正确），实现完整（DoH+UDP+wire+LRU 缓存） |
| unwrap/expect | 修复 3 处：common.rs DST 歧义、resolver.rs 时钟回退、nodes/manager.rs semaphore |
| statistics | 2s 采样低风险，缺数据保留策略（Phase 1 加 cleanup_old） |

### Phase 1 开发进度

| 项 | 状态 | 说明 |
|---|---|---|
| P1-5 主题切换 | ✅ 完成 | App.ThemeMode + ApplyTheme；设置页外观卡片（跟随系统/浅色/深色） |
| P1-6 规则测试 | ✅ 完成 | 服务端 `routing.test`（engine.test_match）+ RulesPage 测试卡片 + ps1 `-Payload` |
| P1-2 流量图表 | ✅ 完成 | 服务端 `statistics.getRange` + db.query_range；HomeViewModel 60 点速率历史；HomePage Canvas/Polyline 实时折线（蓝=下行/绿=上行，10KB/s 基线自适应缩放） |
| P1-1 系统托盘 | ✅ 完成 | TrayIconHost（纯 P/Invoke Shell_NotifyIcon，内存自绘圆点图标绿/灰，双击唤窗，右键菜单 打开/连接/断开/退出，连接动作用 connection.reconnect 空参数=上次节点+模式）；关闭窗口=隐藏到托盘（AppWindow.Closing 拦截） |
| P1-3 活跃连接面板 | ⬜ 未开始 | |
| P1-4 日志查看器 | ⬜ 未开始 | |

### 已实测通过（回归）

- connect JP09 → Google 204/200，出口东京 212.135.214.2
- routing.test：global 模式 google.com → proxy；smart 模式 192.168.1.1 → direct(private/lan)
- disconnect → 注册表还原，百度直连 200
- 客户端 UI 自动化验证：主页连接卡片、设置页外观卡片、规则页测试卡片均渲染正常
- 已知行为：启动恢复偶发 localdead（恢复时序），手动重连即恢复，不阻塞

### 环境备忘

- 构建 Rust 前必须停服务+客户端（exe 占用 os error 5）；`cargo build` cwd=service\network-service
- 客户端构建：`dotnet build client\WindowsClient\WindowsClient.csproj -c Debug -p:Platform=x64`
- 强杀进程后系统代理可能残留 → 重启服务即走 restore_after_crash 清理；本机直连基线 127.0.0.1:7897（ProxyEnable=0）
- Clash Verge 可能抢占系统代理（PID 19748 + verge-mihomo 19288），需提醒用户退出

### 当前进行中任务（2026-09-14 14:20）

**P1-2 流量统计接线（核心缺陷修复）—— 编译失败，待修复**

发现流量统计从未接通：`record_up/record_down` 在全代码库中无调用方，首页"实时速率/今日流量"永远为 0。

已完成的改动：
1. ✅ Cargo.toml：reqwest 加 `http2` feature
2. ✅ xray_builder.rs：加 `stats`/`api`/`policy` 配置 + `STATS_PORT=15490` dokodemo-door 入站
3. ✅ xray_stats.rs（新文件）：gRPC QueryStats 客户端（prost 手写帧，无 tonic 依赖，尝试 xray.core/v2ray.core 两个 service path）
4. ✅ statistics/manager.rs：加 `session_active()` 方法
5. ✅ main.rs：注册 `proxy::xray_stats` 模块 + 2s 轮询任务（session_active 时调 query_totals，差分喂 record_up/down）
6. 🔴 **编译错误**：xray_stats.rs 第 73 行 `resp.status().is_ok()` 应改为 `is_success()`
   - 用户拒绝了该次 Edit（打断要求先解决网络问题），需重新应用此修复

**当前服务二进制**：上次成功编译版本（不含 stats 接线），已启动并连接 JP09，Google 正常。

### 下一步任务（按优先级）

1. **修复编译错误**：xray_stats.rs `is_ok()` → `is_success()`，重新 `cargo build`
2. **实测流量统计**：连接后浏览网页，验证首页速率/今日流量非零，`statistics.getRange` 返回非空 samples
3. **客户端内"修复功能"**（用户明确要求）：
   - 诊断页加"一键修复"按钮：disconnect → 清理注册表代理 → 重连
   - 或设置页加"网络修复"入口：flushdns + 重置 WinHTTP 代理 + 重启服务子进程
4. **继续 P1-1 系统托盘实测**：TrayIconHost 已编译通过，但 CreateIconIndirect DllImport 修复后未实测
5. **P1-3 活跃连接面板** + **P1-4 日志查看器**
6. **回归测试**：杀 xray 快速恢复、热切换、断开还原、Google 204

### 用户要求

- 每次测试要有闭环：启动→连接→验证→断开→清理→确认无残留
- 客户端内需有"修复功能"（用户网络出问题时可自助修复）
- 开发迭代不能破坏主功能（连接+代理+Google 访问）
