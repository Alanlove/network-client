# Network Client 技术架构文档

> 适用版本：network-service 0.1.0（Rust）+ WindowsClient（WinUI 3 / .NET 9）
> 内核：Xray 25.12.8、sing-box 1.14.0（均为外部进程，置于 `data/bin/`）
> 本文全部内容以当前代码实现为准。

---

## 1. 项目概览

一个 Windows 平台的代理客户端，采用**双进程 + 命名管道**架构：

```
┌────────────────────────────┐        命名管道          ┌──────────────────────────────┐
│  WindowsClient.exe (GUI)   │  \\.\pipe\NetworkClient  │  network-service.exe (Rust)  │
│  WinUI 3 / .NET 9          │ ◀══════════════════════▶ │  tokio 异步运行时             │
│  MVVM Toolkit              │  u32-LE + protobuf 帧     │                              │
│                            │  请求/响应 + 服务端推送   │  ┌────────────────────────┐  │
│  首页 / 节点 / 规则 /       │                          │  │ 连接编排 ConnectionMgr │  │
│  诊断 / 设置                │ ◀──── EventBus 推送 ──── │  │ 健康监督 supervise     │  │
└────────────────────────────┘                          │  └───────────┬────────────┘  │
        │ 找不到管道时自动拉起                              │              │ spawn        │
        └──────────────────────────────────────────────▶│  ┌───────────▼────────────┐  │
                                                        │  │ xray.exe / sing-box.exe│  │
                                                        │  │ HTTP 代理 :2080        │  │
                                                        │  │ SOCKS    :2081         │  │
                                                        │  └────────────────────────┘  │
                                                        │  WinINET 注册表系统代理       │
                                                        └──────────────────────────────┘
```

设计原则：

- **控制面与数据面分离**：Rust 服务只管状态、编排、系统集成；真正转发流量的是外部内核进程，产品不内置任何代理线路。
- **故障可恢复、退出不留垃圾**：任何异常退出后重启都能回收孤儿内核、还原系统代理。
- **协议可扩展**：节点模型用自由 JSON 段承载协议差异，新增协议不影响控制面。

---

## 2. 技术栈

### 服务端（`service/network-service`）

| 分类 | 选型 | 用途 |
|---|---|---|
| 异步运行时 | tokio（full）+ tokio-util | 任务调度、命名管道、子进程、定时器、CancellationToken |
| IPC 序列化 | prost 0.13（protobuf） | 管道消息编解码，`build.rs` 用 protoc-bin-vendored 生成 |
| HTTP 客户端 | reqwest 0.12（rustls，socks feature） | 订阅下载、隧道存活探测、测速 |
| 持久化 | serde / serde_json（原子写）+ rusqlite（bundled） | 配置/节点/订阅用 JSON；流量统计用 SQLite |
| Windows 集成 | windows 0.58（WinINet/DPAPI/IpHelper/Security）+ winreg 0.52 | 系统代理注册表直写、DPAPI 加密、管道 SDDL |
| 并发原语 | parking_lot（同步读写锁）+ tokio Mutex（跨 await） | 状态存储与子进程句柄 |
| 日志 | tracing + tracing-appender | 按天滚动文件 `data/logs/service.log.<date>` |
| 内核 | Xray / sing-box 外部进程 | 协议实现与流量转发 |

### 客户端（`client/WindowsClient`）

- WinUI 3（`Microsoft.WindowsAppSDK` 1.6，非打包 `WindowsPackageType=None` 部署），目标框架 `net9.0-windows10.0.19041.0`，x64/ARM64。
- CommunityToolkit.Mvvm 8.4（`[ObservableProperty]`/`[RelayCommand]` 源生成器）。
- 手写 protobuf 线格式编解码（`Ipc/ProtoCodec.cs`），无 .proto 生成依赖。

---

## 3. 目录结构与模块职责

```
network-client/
├─ service/network-service/
│  ├─ build.rs                  # prost 代码生成
│  └─ src/
│     ├─ main.rs                # 装配根：构建所有 Manager、启动后台循环与 IPC、有序关停
│     ├─ common.rs              # SvcError 错误枚举(含 HTTP 风格 code 映射)、now_millis、URL 脱敏
│     ├─ app/
│     │  ├─ context.rs          # AppContext 依赖容器 + 全部 IPC 方法 dispatch
│     │  ├─ lifecycle.rs        # 连接/断开/热切换/恢复/开机恢复（核心编排）
│     │  └─ state.rs            # 连接状态机（9 态 + 合法迁移表 + 变更广播）
│     ├─ ipc/                   # 命名管道服务、帧编解码、事件总线、proto 消息
│     ├─ proxy/                 # 内核适配层：子进程生命周期 + sing-box/Xray 配置生成
│     ├─ health/monitor.rs      # 单任务健康监督循环（三态判定 → 自愈）
│     ├─ route/                 # WinINET 系统代理（注册表直写 + 备份/还原/崩溃清理）
│     ├─ nodes/                 # 节点存储、TCP 探测、运行时评分(EWMA/加权/迟滞)
│     ├─ subscription/          # 订阅 CRUD、下载、base64 解码、URI 解析(vless/vmess/ss/trojan/socks)
│     ├─ config/                # app.json/runtime_state 模式、读写、DPAPI 加密
│     ├─ statistics/            # SQLite(sessions/traffic_samples/node_health/diagnostics) + 2s 刷盘
│     ├─ diagnostics/           # 综合诊断：DNS/ICMP 丢包/隧道探测/测速
│     ├─ dns/                   # DoH/UDP 解析器 + 缓存（独立能力，当前内核内置 DNS 为主）
│     ├─ routing/               # 分流规则引擎（global/smart/direct + 规则匹配）
│     └─ tun/                   # TUN 虚拟网卡（预留，tun.enabled=false 时不启用）
├─ client/WindowsClient/        # GUI：Program 自定义入口 + App + MainWindow + 5 页面
├─ data/                        # NC_DATA_DIR 指向的运行数据根（见 §5）
├─ tools/                       # ipc_subscribe.ps1（调试）/ start-client.ps1（一键启动）
└─ docs/
```

---

## 4. 核心数据模型

### 4.1 连接状态机（[app/state.rs](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/app/state.rs)）

9 个状态，每次迁移都经过 `can_transition` 白名单校验，非法跳转直接报错；任何状态变更都广播 `connection.stateChanged`：

```
STOPPED → STARTING → CONNECTING → CONNECTED → STOPPING → STOPPED
                        │             │  ▲          │
                        ↓             ↓  └──────────┘ (热切换 Stopping→Starting)
                       ERROR      RECONNECTING
```

关键点：

- `(Stopping, Starting)` 是**热切换专用迁移**：已连接时对不同节点/模式发起 connect，只换内核不动系统代理（本地端口 2080 不变，用户备份不丢）。
- 同目标重复 connect 返回 409 `already running: connection`。
- 中间态（STARTING/CONNECTING/STOPPING）下的 connect 一律 409，由 `ConnectionManager.guard`（tokio Mutex）串行化。

### 4.2 统一节点模型（[nodes/model.rs](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/nodes/model.rs)）

```rust
Node { id, name, protocol, endpoint{host,port},
       authentication: JSON,   // uuid/password/method/flow/reality...
       transport: JSON,        // type=tcp/ws/grpc/xhttp + path/host/service_name...
       tls: { enabled, server_name, insecure, alpn },
       metadata: JSON, subscription_id, enabled, created_at }
```

- 支持协议常量：`vless / vmess / trojan / shadowsocks / socks / wireguard`。
- 协议特有字段是自由 JSON 而非 god-struct，加协议只加 parser 子模块 + 一个 dispatch 分支。
- **稳定 ID（FNV-1a 64）**：`node_<hash(protocol|host|port|identity)>`，订阅刷新后同一节点 ID 不变，历史测速统计得以延续。

### 4.3 可配置项（[config/schema.rs](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/config/schema.rs)）

`data/config/app.json`（缺字段全部 `#[serde(default)]` 兜底）：

| 段 | 关键值（默认） |
|---|---|
| `restore_previous_connection` | 崩溃重启是否自动重连（false；当前开发环境置 true） |
| `core` | kind=sing-box（可强制 xray）、mixed_port **2080**、socks_port **2081**、api_port 2082、start_timeout_secs 10、restart_max 3 |
| `smart` | switch_threshold 10.0（切节点迟滞阈值）、health_interval_secs **45**（健康巡检周期，下限 10s） |
| `tun` | enabled=false（预留；关闭时走 WinINET 系统代理） |
| `dns` | smart 模式；223.5.5.5 UDP + 1.1.1.1 DoH；缓存 4096 |
| `log_level` | info |

`data/runtime_state.json` 是崩溃恢复的**提示**而非指令：`{last_state, node_id, mode, updated_at}`，恢复时节点、内核、网络全部重新校验。

### 4.4 data 目录布局（`NC_DATA_DIR`）

```
data/
├─ bin/xray.exe, sing-box.exe      # 外部内核
├─ config/app.json, routing.json
├─ core/config.json                # 每次连接重新生成的内核配置（原子写）
├─ nodes/nodes.json                # 节点全量存储
├─ subscriptions/subscriptions.json# 订阅（URL 为 dpapi: 密文）
├─ runtime_state.json              # 上次会话状态
├─ proxy_backup.json               # 连接前系统代理快照（断开即删）
├─ db/statistics.db                # 流量/健康/诊断历史（SQLite）
└─ logs/service.log.<date>         # 按天滚动
```

---

## 5. IPC 协议（[ipc/](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/ipc/server.rs)）

### 5.1 传输与帧

- 管道名 `\\.\pipe\NetworkClient`，每连接一个管道实例；SDDL `D:P(A;;GA;;;OW)` —— **仅服务运行用户本人可连**。
- 帧：`u32 小端长度 + protobuf 字节`，上限 16 MiB（订阅报文可能较大）。
- 消息三类：`Request`（id+method+payload）/ `Response`（id+code+message+payload）/ `Event`（event_type+payload+timestamp）。
- 每条连接：读循环按请求 spawn 独立 task；**单写者** mpsc(64) 同时承载响应和事件推送；事件来自进程内 broadcast `EventBus`，lag 时跳过不中断。
- 请求处理有 **panic 隔离**（`AssertUnwindSafe(...).catch_unwind()`）：handler 内部 bug 只杀死该请求 task 并回 500，客户端不会永远等不到帧；帧解析失败回 422；未知方法回 404。

### 5.2 方法面（frozen surface，C# 端镜像）

| 域 | 方法 |
|---|---|
| 系统 | `system.getVersion` |
| 连接 | `connection.getStatus / connect / disconnect / reconnect` |
| 节点 | `node.list / get / add / update / delete / select / test / batchTest` |
| 订阅 | `subscription.list / add / update / delete / refresh` |
| 分流 | `routing.getConfig / setMode / listRules / addRule / updateRule / deleteRule` |
| DNS | `dns.getConfig / setConfig / test / flushCache` |
| 统计 | `statistics.get / getToday` |
| 诊断 | `diagnostics.run / cancel / ping / dns / proxy / speed / packetLoss / resolve` |

### 5.3 推送事件（[ipc/events.rs](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/ipc/events.rs)）

`connection.stateChanged`、`node.latencyChanged`、`node.healthChanged`、`subscription.updated/failed`、`traffic.updated`、`network.changed`、`dns.changed`、`core.started/stopped/error`、`diagnostics.progress/completed`。

---

## 6. 端到端关键流程

### 6.1 服务启动与崩溃恢复（[main.rs](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/main.rs) → `startup_recover`）

```
main(): 加载配置 → 初始化日志 → 构建 EventBus 与全部 Manager(AppContext)
  ├─ spawn startup_recover()        # 与 IPC 并发，管道先可用
  ├─ spawn 统计 2s 刷盘循环
  ├─ spawn 订阅后台循环(每 600s，首 tick 立即执行；MissedTickBehavior::Skip)
  └─ IPC 服务器（Ctrl-C 后有序关停：停 IPC→disconnect→刷统计）
```

`startup_recover` **全程持有连接 guard**（避免恢复期间 IPC connect/disconnect 与回收竞态）：

1. `reap_stale_cores()`：用 PowerShell `Win32_Process` 按**命令行是否引用本服务生成的 config.json 路径**精确匹配，只杀自己遗留的 xray/sing-box（10s 超时，CREATE_NO_WINDOW），不碰机器上其他内核。
2. 读 runtime_state：`last_state==CONNECTED` 且节点仍存在/启用且配置允许恢复 →
   `prime_from_backup()`（把崩溃备份载入内存但**不动注册表**，保证日后断开还原的是用户原始代理）→ 走与正常连接相同的 `connect_locked()` → `ensure_proxy_applied()` 兜底重写。
3. 不恢复/恢复失败 → `restore_after_crash(port)`（见 §6.4）并持久化 STOPPED。

### 6.2 连接流程（`connect_locked`，失败每步回滚已启动的部分）

```
STARTING → 校验节点(enabled) → nodes.select / routing.set_mode → CONNECTING
 → 1. core.start(): 选内核 → 生成 config.json(原子写) → spawn
       → 轮询 127.0.0.1:2080(200ms 间隔, 超时 start_timeout_secs)
       → try_wait 防"端口被孤儿占用导致新内核退出却误判成功"
 → 2. (可选) TUN，失败降级为系统代理
 → 3. spawn_blocking 写系统代理（注册表直写不能阻塞异步线程）
 → 4. stats.start_session → CONNECTED + 持久化 runtime_state
 → 5. 首次延迟探测（直连 TCP 探测，失败回退经内核 test_node）
 → spawn_monitor(): 启动 45s 周期的健康监督任务
```

**热切换**：CONNECTED 状态收到不同 node/mode 的 connect → Stopping → 停 monitor/TUN/core/统计会话 → Starting → 复用上述流程；系统代理与用户代理备份保持不动。

**内核选择策略**：配置 `core.kind=xray` 则全部走 Xray；否则节点 `transport.type=xhttp` 自动选 Xray（sing-box 无此传输），其余走 sing-box。

**Xray 配置生成要点**（[proxy/xray_builder.rs](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/proxy/xray_builder.rs)）：本地双 inbound（HTTP :2080 + SOCKS :2081，均 127.0.0.1、sniffing 开启）；出站支持 vless/trojan/shadowsocks + tcp/ws/grpc/xhttp + TLS/Reality（uTLS chrome 指纹、SNI 回退 host、allowInsecure 透传）；局域网/环回地址强制 direct；DNS 段最小化（无 geo 资产依赖）。

### 6.3 健康监督与自愈（三态，本轮核心改造）

[proxy/adapter.rs](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/proxy/adapter.rs) 定义三态，[proxy/manager.rs](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/proxy/manager.rs) 实现探测，[health/monitor.rs](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/health/monitor.rs) 驱动决策：

| 状态 | 判定 | 监督反应 |
|---|---|---|
| **Healthy** | 状态 RUNNING 且本地 1s TCP 连通，且经隧道请求 gstatic `generate_204` 返回 200/204（client 8s/总 10s 超时） | 失败计数清零 |
| **LocalDead** | 状态非 RUNNING，或本地 inbound TCP 拒绝/超时 | **立即恢复，不容错**（进程尸体不需要三次确认） |
| **UplinkDead** | 本地端口活着但隧道请求失败（远端抖动） | 连续 **3 次**才恢复 |

监督任务是一个**自循环长任务**：`supervise → tick_phase →（需要恢复时）cm.recover() → 状态仍 CONNECTED 则重新进入新 tick_phase`。恢复后不重新 spawn 任务，杜绝"monitor await recover，recover 又 await spawn monitor"的递归 future（非 Send）死锁。

`recover()` 的升级策略：

1. 第 1..restart_max（默认 3）次：原地 `core.restart()` 并立刻 health_check 验收；
2. 超过次数且当前是 smart 模式：`smart_select(threshold)` 选分差足够大的节点**切换一次**；
3. 全部失败：拆光（停内核 + 清系统代理 + 结束统计）并置 ERROR（错误码 601）——绝不让系统代理指向死端口。

实测：杀掉 xray 子进程后约 **52s（一个巡检 tick）**恢复；旧二态逻辑需要 ~139s（3×45s）。

### 6.4 系统代理备份/还原（[route/manager.rs](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/route/manager.rs)）

- 连接时读取注册表 `HKCU\...\Internet Settings` 的 `ProxyEnable/ProxyServer/ProxyOverride` 三值，写入 `http=127.0.0.1:2080;https=...`，原值快照存内存**并原子落盘** `proxy_backup.json`（幂等，重复连接不覆盖快照）。
- **为什么直写注册表**：`InternetSetOption(INTERNET_OPTION_PROXY)` 在部分 Windows 版本上返回成功却不写注册表；v2rayN/Clash 同样直写后再广播 `INTERNET_OPTION_SETTINGS_CHANGED/REFRESH`。代码同时保留直读注册表（而不是 InternetQueryOption 的进程内视图）。
- 断开时按快照**逐字还原**（原本直连的机器先写回原字符串再关 ProxyEnable），成功后删除备份。
- 孤儿识别用 `references_endpoint()` 按 `;` 分段精确匹配裸端点/`http=`/`https=`/`socks=`，避免 `:2080` 误匹配 `:20800`（有单测）。
- 若注册表残留指向自己的死端口且无备份：连接时把基线记为"直连"，崩溃清理时直接关闭代理，防止断开时"还原"成一个死代理。

### 6.5 订阅刷新管线（[subscription/](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/subscription/manager.rs)）

```
add: URL 经 DPAPI 加密(dpapi: 前缀)落盘
refresh（同一订阅 in_flight 互斥，拒绝并发）:
  DOWNLOAD(reqwest, 连接 10s/总 20s 超时, 必须 2xx 且非空)
  → DECODE(明文 URI 列表直通；否则 url-safe/标准 base64 尝试解码)
  → PARSE(vless/vmess/ss/trojan/socks URI → Node，FNV 稳定 ID)
  → 全部成功后才 replace_subscription_nodes 落盘
```

**先解析后落盘**：下载或解析失败保留旧节点，只发 `subscription.failed` 事件。后台每 10 分钟扫描一次，到期条件：enabled 且（从未更新，或 `now - last_updated ≥ interval_secs.max(60) 秒`）。

### 6.6 测速、评分与智能选路（[nodes/health.rs](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/nodes/health.rs)）

- TCP 探测延迟做 **EWMA 指数平滑**（0.7 旧值 + 0.3 新值），抑制单次尖刺；探测耗时四舍五入并钳到 ≥1ms。
- 百分制加权评分：**延迟 30% + 丢包 25% + 可用性 20% + 吞吐 15% + 成功率 10%**（延迟 ≥300ms 记 0 分，吞吐按 100MB/s 归一）。
- 选路带**反抖动迟滞**：新节点分数必须比当前节点高出 `switch_threshold`（默认 10 分）才切换。

### 6.7 流量统计（[statistics/](file:///c:/Users/haihu/Desktop/vpn/network-client/service/network-service/src/statistics/manager.rs)）

SQLite 四张表：`sessions / traffic_samples / node_health / diagnostics`。后台每 2s 采样增量字节、计算速率、写库并推送 `traffic.updated`；仅在会话活跃且有增量时写库/推送（空闲客户端不被零值事件打扰）。

---

## 7. 核心技术点小结

1. **三态健康模型 + 差异化容错**：本地内核死立即自愈（实测 52s），远端抖动三振出局，兼顾恢复速度与抗误判。
2. **严格的连接状态机**：9 态白名单迁移 + guard 串行化 + 热切换专用迁移，并发连接请求不可能产生撕裂状态。
3. **崩溃自愈体系**：孤儿内核按命令行精确回收、runtime_state 提示恢复、系统代理备份落盘、恢复全程持锁、失败路径必清理。
4. **WinINET 系统代理工程细节**：注册表直写 + SETTINGS_CHANGED 广播、三值快照逐字还原、端点精确匹配防端口前缀误伤。
5. **DPAPI 用户级密文存储**：`CryptProtectData` + 应用熵（`NetworkClient/v1/DPAPI`），订阅 URL 落盘即密文；非 Windows 回退 base64 并显式打 `devb64:` 前缀。
6. **零信任的配置/存储读取**：JSON 原子写（tmp→rename）、UTF-8 BOM 剥离、缺字段 serde default、文件存在但解析失败时 warn 而非静默回空。
7. **协议可扩展架构**：节点协议差异全部收敛进 JSON 段 + parser 子模块 + 内核配置 builder，控制面零改动加协议。
8. **IPC 健壮性**：owner-only SDDL、单写者多路复用（响应+推送共线）、请求级 catch_unwind panic 隔离、统一错误码（404/409/422/500）。
9. **稳定性数据治理**：FNV 稳定节点 ID（刷新不丢历史）、EWMA 延迟、加权评分、迟滞切节点。
10. **GUI/服务解耦的自动拉起**：客户端按 `NC_SERVICE_EXE` → 安装目录两级探测自动启动服务，管道断线每 2s 自动重连，退出时不杀"外部附加"的服务。

---

## 8. 构建与运行（附录）

```powershell
# 服务端（cwd 必须在 service\network-service）
cargo build
cargo test                 # 25 个单元测试（状态机/评分/编解码/端点匹配等）

# 客户端
dotnet build client\WindowsClient\WindowsClient.csproj -c Debug -p:Platform=x64

# 一键启动（自动设置 NC_DATA_DIR / NC_SERVICE_EXE 并拉起 GUI）
powershell -ExecutionPolicy Bypass -File tools\start-client.ps1
```

运行约定：

- 数据根用环境变量 `NC_DATA_DIR` 指定（沙箱/开发环境不用 %LOCALAPPDATA%）；GUI 自动拉起服务时子进程继承该变量。
- 重新 `cargo build` 前必须先退出客户端与服务，否则 Windows 占用 exe 导致链接失败（os error 5）。
- 本机其他代理软件（如 Clash Verge）可能抢占系统代理，联调时需先退出。
- TUN 数据面为预留能力（`tun.enabled=false`），dns/tun/routing 部分代码在当前配置下不参与运行链路。
