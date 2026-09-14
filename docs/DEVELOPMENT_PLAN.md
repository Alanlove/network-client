# Windows 网络代理客户端 — 开发计划

> 依据：`Windows_网络代理客户端——技术架构与开发设计文档_v1.0.md` 与 Technical Design v1.1（`说明.md`）
> 仓库根：`network-client/`
> 制定日期：2026-09-12

---

## 1. 技术栈冻结

| 层级 | 选型 | 备注 |
|---|---|---|
| UI | C# + WinUI 3 / .NET 9 | NavigationView + MVVM（CommunityToolkit.Mvvm） |
| IPC | Named Pipe `\\.\pipe\NetworkClient` + Protobuf | 长度前缀帧（u32 LE），Request/Response/Event |
| Service | Rust（edition 2021, tokio） | 控制台形态先行，Windows Service 注册随后 |
| TUN | Wintun（运行时加载 wintun.dll） | 服务通过 libloading 动态加载，缺失时优雅报错 |
| DNS | 自研 Resolver Layer（UDP + DoH/8484）+ TTL Cache | V1 不做 Fake-IP |
| Routing | 自研 Rule Engine（Domain Trie / Bit-Trie CIDR / Process / GeoIP RuleSet） | DIRECT / PROXY / BLOCK |
| Proxy Core | Core Adapter 接口 + 外部 Core 进程（sing-box 配置优先） | 产品不内置线路；Core 二进制与节点均由用户提供 |
| Storage | JSON（原子写）+ SQLite（rusqlite bundled） | |
| 敏感数据 | Windows DPAPI（CryptProtectData） | 订阅 URL / UUID / Password |
| 日志 | tracing + 文件轮转（20MB × 5） | 脱敏 |

## 2. 目录结构（对齐 v1.1 §2）

```
network-client/
├── client/WindowsClient/          # WinUI 3
├── service/network-service/       # Rust bin crate
│   └── src/{app,ipc,config,nodes,subscription,routing,dns,tun,route,proxy,diagnostics,statistics,health}/
├── shared/proto/network.proto     # IPC 契约（冻结）
├── shared/schemas/*.json
├── installer/ updater/ tests/ docs/
```

## 3. 里程碑（对齐 v1.1 §67 EPIC M1–M10）

| 里程碑 | 内容 | 状态 |
|---|---|---|
| M1 基础工程 | 仓库、Rust Service、IPC、proto、WinUI 骨架 | 本次交付 |
| M2 TUN | Wintun 生命周期、packet 解析（Flow 抽象）、MTU | 生命周期+Flow 解析本次交付；用户态协议栈（smoltcp）紧随其后 |
| M3 Route | RouteManager、备份/恢复、Proxy Bypass、DNS Route、防环 | 本次交付（route.exe 实现，IP Helper API 随后替换） |
| M4 Core | Adapter trait、进程管理、sing-box ConfigBuilder、健康探测 | 本次交付 |
| M5 Node | 统一 Node Model、CRUD、RuntimeStats、评分、Smart Select（滞回阈值） | 本次交付 |
| M6 Subscription | download / base64 decode / ss·vmess·trojan·vless·socks parser / 校验 / 原子更新 | 本次交付 |
| M7 Routing | Rule Model、Domain Trie、CIDR Bit-Trie、Process、GeoIP RuleSet、Decision | 本次交付 |
| M8 DNS | Manager、UDP/DoH Resolver、TTL Cache、策略路由 | 本次交付 |
| M9 UI | Home / Nodes / Rules / Diagnostics / Settings / Tray | 骨架 + 可交互页面本次交付 |
| M10 稳定性 | 网络切换、睡眠唤醒、Core/TUN/Service 恢复、崩溃状态 | 框架本次交付，用例随后 |

补充：Statistics（内存计数→2s flush→SQLite）、Diagnostics（网关/DNS/代理/丢包/测速）、DPAPI 加密、日志轮转随 M1 一并落地。

## 4. 本次迭代验收（冒烟链路）

1. **链路一（System Proxy 模式，真实可用）**：启动 Service → 拉起外部 Core（sing-box，mixed 入站）→ 设置系统代理 → 经 Core 访问 Internet → 断开 → 解除代理，幂等可重复。
2. **链路二（订阅）**：订阅 URL → 下载 → base64 解码 → 多协议解析 → Node 列表 → 选节点 → 连接。
3. **链路三（TUN 控制面）**：Wintun 适配器创建/销毁、地址/MTU 配置、路由备份/添加/Bypass/恢复、Flow 解析 → Rule → Decision 流水线打通。
4. **UI ↔ Service**：Named Pipe + Protobuf 双向通信，事件驱动状态刷新。
5. `cargo test` 全绿；`cargo clippy` 无警告；`dotnet build` 通过。

## 5. 明确的后续任务（不在本次范围）

- smoltcp 用户态 TCP/IP 栈接入 TUN，完成 TUN 全流量数据面（当前 TUN 完成生命周期 + Flow 分类）。
- Windows Service（SCM）注册入口、MSIX/WiX 安装器、独立 Updater。
- Kill Switch、Fake-IP、mmdb GeoIP、Reality/QUIC 等传输、IPv6 增强。
- 进程→连接映射（WFP / IP Helper TCP 表）。

## 6. 关键工程原则（摘自设计文档，执行中遵守）

- UI 不碰 TUN/Route/Core；唯一通道是 IPC；UI 状态以 Service 事件为准。
- `disconnect()` 必须幂等；所有配置修改 Validate → tmp → rename 原子写。
- Proxy Server IP 必须走物理网卡（Bypass），杜绝 TUN 回环。
- NodeConfig 与 NodeRuntimeStats 分离；测速结果不写用户配置。
- 订阅更新失败保留旧配置；运行态 `runtime_state.json` 仅作恢复提示，不盲目重连。
- 最小权限：UI 普通用户，Service 承载高权限操作；Pipe ACL 限制访问主体。
