# Network Client

> 一个从零开始写的 Windows 代理客户端，Rust 服务端 + WinUI 3 现代化界面。
> 自研内核适配层，已接入 **Xray 25.12.8** 和 sing-box 1.14.0，系统代理/TUN 双模式切换。

[![Status](https://img.shields.io/badge/status-beta-orange.svg)]()
[![Rust](https://img.shields.io/badge/rust-1.82+-orange.svg)]()
[![WinUI](https://img.shields.io/badge/WinUI-3-blue.svg)]()
[![License](https://img.shields.io/badge/license-MIT-green.svg)]()

---

## ✨ 亮点

- **🎯 系统代理零残留** — 断开时自动还原注册表（ProxyEnable / ProxyServer / ProxyOverride 三值逐字），备份写 `data/backup_proxy.json`，崩溃重启时先清理再重连。拒绝把死代理遗留给用户。

- **⚡ 三态健康模型** — `Healthy / LocalDead / UplinkDead`，LocalDead（内核本地 TCP 不通）**立即触发恢复**（不再等 3 个 45s 巡检周期），实测恢复时间 ~52s。

- **🔥 热切换连接** — `CONNECTED → CONNECTED(不同节点/模式)` 无缝迁移：停 monitor / 停内核 / 启动新内核 / 重建 monitor，全程持锁，无竞态。

- **🩹 崩溃自愈** — 启动时先 `reap_stale_cores`（按命令行精确匹配 kill 孤儿内核），再看 `runtime_state.last_state == CONNECTED` + `restore_previous_connection` 决定是否自动重连。服务被强杀重启也能恢复到工作状态。

- **📊 流量统计真实可用** — 直接读 xray 进程的 `GetProcessIoCounters`（Win32 API），避开 Xray 25.x gRPC stats 静默返回空的坑。每 2s 差分进 SQLite，客户端 HomePage 实时折线图。

- **🔔 Win32 P/Invoke 系统托盘** — 纯 Win32 Shell_NotifyIcon，内存自绘圆点图标（绿/灰），双击唤窗，右键菜单，关闭主窗口隐藏到托盘。零 NuGet 依赖。

- **🛠️ 一键修复** — 诊断页：断开 → 清注册表代理 → `ipconfig /flushdns` → reconnect → gstatic 204 验证。

- **💎 订阅管线完整** — 下载 → base64/URL 解码 → VLESS/VMess/Trojan/SS/SOCKS 解析 → 全部成功才落盘。DPAPI + 应用熵加密订阅 URL。

## 🏗️ 架构

```
┌─────────────────────────────┐
│   WindowsClient.exe          │  WinUI 3 · .NET 9 · x64
│   (GUI + 托盘)               │
└────────────┬────────────────┘
             │ Named Pipe \\.\pipe\NetworkClient
             │ 帧格式: u32-LE(len) + protobuf
┌────────────▼────────────────┐
│   network-service.exe        │  Rust · tokio · prost
│   ├─ AppContext (全局状态)   │
│   ├─ ConnectionManager       │  状态机 + 持锁串行化
│   ├─ RouteManager            │  注册表代理 + 备份
│   ├─ HealthMonitor           │  supervise 自循环
│   ├─ ProxyManager            │  内核适配器（Xray/sing-box）
│   ├─ XrayStatsPoller         │  2s 差分进 StatsManager
│   ├─ NodeManager             │  EWMA 延迟 + 加权评分
│   ├─ SubscriptionManager     │  下载→解码→解析→落盘
│   ├─ RoutingEngine           │  规则匹配（TUN 模式用）
│   ├─ DiagnosticEngine        │  ping / DNS / proxy 链路 / speed
│   └─ SQLite (traffic_samples) │
└─────────────────────────────┘
             │
    ┌────────┼────────┐
    ▼        ▼        ▼
  Xray    sing-box   注册表代理
 (可选)   (可选)
```

更详细的设计：见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
下一轮开发计划：见 [docs/DEVELOPMENT-PLAN.md](docs/DEVELOPMENT-PLAN.md)

## 🛠️ 技术栈

| 层 | 技术 |
|---|---|
| GUI | WinUI 3 / .NET 9 / CommunityToolkit.Mvvm |
| 服务端 | Rust 1.82+ / tokio / prost / reqwest / rusqlite / windows-rs 0.58 / winreg |
| IPC | Named Pipe + protobuf (v0.13) |
| 内核 | Xray 25.12.8（vless + xhttp + tls）/ sing-box 1.14.0 |
| 系统代理 | Win32 Registry (HKCU\...\Internet Settings) |
| 托盘 | Shell_NotifyIconW + 内存自绘 ARGB 圆点 |
| DNS | DoH + UDP + wire 格式解析 + TTL 缓存 + LRU 驱逐 |

## 🚀 快速开始

### 前置条件
- Windows 10 22H2+ / Windows 11
- Visual Studio 2022 17.8+ 或 .NET 9 SDK（csproj 是 `net9.0-windows10.0.19041.0`）
- Rust 1.82+
- Windows App Runtime 1.6（随 WinUI 项目自动下载）

### 构建

```powershell
# 1. 构建服务端
cd service\network-service
cargo build --release

# 2. 构建客户端
cd ..\..
dotnet build client\WindowsClient\WindowsClient.csproj -c Release -p:Platform=x64
```

### 运行

```powershell
# 启动脚本（自动处理 NC_DATA_DIR / NC_SERVICE_EXE）
powershell -ExecutionPolicy Bypass -File tools\start-client.ps1
```

客户端会自动发现服务：管道存在 → 直接附加；不存在 → 按 `NC_SERVICE_EXE` 自行拉起子进程（继承环境变量）。

### 手动 IPC 调试

```powershell
# status / connect / disconnect / stats / diagnostics / logs.tail 全部支持
powershell -ExecutionPolicy Bypass -File tools\ipc_subscribe.ps1 status
powershell -ExecutionPolicy Bypass -File tools\ipc_subscribe.ps1 connect -NodeId node_xxx -Mode global
powershell -ExecutionPolicy Bypass -File tools\ipc_subscribe.ps1 call -Id statistics.getRange -Payload "0"
powershell -ExecutionPolicy Bypass -File tools\ipc_subscribe.ps1 call -Id logs.tail -Payload "200"
```

## 📂 目录结构

```
network-client/
├── client/WindowsClient/     # WinUI 3 客户端
│   ├── Ipc/                   # Named Pipe 客户端 + protobuf codec
│   ├── Models/ViewModels/     # MVVM 层
│   ├── Services/              # ServiceHost（管道自动重连）+ TrayIconHost
│   └── Views/                 # 5 个页面：Home / Nodes / Rules / Diagnostics / Settings
├── service/network-service/  # Rust 常驻服务
│   ├── src/app/               # 生命周期 + 状态机 + 全局上下文
│   ├── src/proxy/             # 内核适配（xray_builder / xray_stats）
│   ├── src/route/             # WinINET 代理读写 + 备份/还原
│   ├── src/health/            # supervise 自循环 + 三态模型
│   ├── src/ipc/               # Named Pipe 帧 + protobuf dispatch
│   ├── src/nodes/             # EWMA 延迟 + 加权评分
│   ├── src/subscription/      # VLESS/VMess/Trojan/SS/SOCKS 五协议解析
│   ├── src/dns/               # DoH + wire + LRU 缓存
│   ├── src/diagnostics/       # ping / proxy / speed / packetLoss
│   ├── src/statistics/        # SQLite 流量样本 + 速率差分
│   └── src/routing/           # Rule Engine + GeoIP（TUN 模式用）
├── shared/proto/             # .proto 定义（服务端 build.rs 生成）
├── tools/                    # PowerShell 脚本：启动 / IPC 调试
├── docs/                     # ARCHITECTURE.md / DEVELOPMENT-PLAN.md
└── data/                     # ⚠️ 运行时数据（gitignore，不提交）
    ├── bin/                  # xray.exe / sing-box.exe 内核二进制
    ├── config/               # app.json / nodes.json / subscriptions.json
    ├── logs/                 # service.log.YYYY-MM-DD
    └── db/                   # statistics.db (WAL 模式)
```

## 🧪 已实测场景

| 场景 | 结果 |
|---|---|
| 杀 xray 子进程 | **~52s 自动恢复**（三态 LocalDead 快速触发） |
| 强杀服务重启（restore=true） | 孤儿 reap + 自动重连 + 恢复代理 |
| 强杀服务重启（restore=false） | 孤儿 reap + 注册表还原 + 不重连 |
| 热切换 HK → JP → US | 204，PID 变更，无缝迁移 |
| disconnect | ProxyEnable=0 / ProxyServer=清空 / 备份删除 / xray=0 |
| 连续 2 次杀 xray | 第 1 次恢复 52s，第 2 次恢复 48s |
| batchTest 18 节点 | 成功 42–1039ms，无 0 值 |
| gstatic generate_204 | 代理模式 204（41–115ms） |
| 托盘 | 启动 CLEAN 无错误，HWND 正确，绿/灰图标切换 |
| 流量统计 getRange | 返回非空 samples（2s 差分，up/down 非零） |
| 一键修复 | disconnect → 清代理 → flushdns → reconnect → 204 ✅ |

## 📋 Phase 1 进度

- ✅ 主题切换（跟随系统 / 浅色 / 深色）
- ✅ 规则测试（`routing.test` 域名/IP → 代理/直连）
- ✅ 系统托盘（Shell_NotifyIconW + 内存自绘图标）
- ✅ 流量图表（Canvas/Polyline 实时折线，速率 + 今日流量）
- ✅ 流量统计接线（GetProcessIoCounters → StatsManager → SQLite → IPC）
- ✅ 一键修复（disconnect → 清代理 → flushdns → reconnect）
- ✅ 日志查看器（`logs.tail` IPC + Expander 折叠面板）
- ✅ UI 统一设计语言（Padding 28 / Spacing 18 / Ellipse 状态指示灯）

下一步：Phase 2 诊断升级（IP 信誉 / 全球延迟 / 网站连通性 · 参考 one-ip）、Phase 3 TUN 启用（Wintun + Bypass Manager + IPv6）。

## 🔐 安全

- 订阅 URL 用 **DPAPI + 应用熵** 加密，每次解密前先校验熵。
- 注册表代理写入走 `HKCU` 当前用户，提权不是必需。
- Named Pipe ACL 设 owner-only SDDL，拒绝其他用户连接。
- IPC handler 包了一层 `AssertUnwindSafe + catch_unwind`，单个 handler panic 回 500 不拖死整个管道。

## 📝 License

MIT © Network Client contributors
