using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Microsoft.UI.Dispatching;
using WindowsClient.Ipc;
using WindowsClient.Services;

namespace WindowsClient.ViewModels;

public partial class DiagLine : ObservableObject
{
    [ObservableProperty] private string title = "";
    [ObservableProperty] private string status = "";
    [ObservableProperty] private string detail = "";
    [ObservableProperty] private string glyph = "\uE9CE"; // Clock
}

public partial class DiagnosticsViewModel : ObservableObject
{
    private readonly ServiceHost _host;
    private readonly DispatcherQueue _dq;

    public ObservableCollection<DiagLine> Items { get; } = new();

    [ObservableProperty] private bool busy;
    [ObservableProperty] private string summary = "";
    [ObservableProperty] private string message = "";
    [ObservableProperty] private string logText = "";
    [ObservableProperty] private bool autoScroll = true;

    public DiagnosticsViewModel(ServiceHost host)
    {
        _host = host;
        _dq = DispatcherQueue.GetForCurrentThread();
        _host.EventReceived += OnEvent;
    }

    private void OnEvent(object? sender, ServiceEvent ev)
    {
        if (ev.EventType != "diagnostics.progress" || ev.Payload.Length == 0) return;
        var p = DiagnosticProgress.Decode(ev.Payload);
        RunOnUi(() =>
        {
            var line = Items.FirstOrDefault(i => i.Title == p.Step) ?? AddLine(p.Step);
            line.Status = p.Status;
            line.Detail = p.Detail;
            line.Glyph = p.Status switch
            {
                "ok" => "\uE73E", // Checkmark
                "fail" => "\uE783", // Cancel
                "warn" => "\uE7BA", // Warning
                _ => "\uE9CE",
            };
        });
    }

    private DiagLine AddLine(string step)
    {
        var line = new DiagLine { Title = StepLabel(step) };
        Items.Add(line);
        return line;
    }

    private static string StepLabel(string step) => step switch
    {
        "network" => "网络连通性",
        "gateway" => "默认网关",
        "dns" => "DNS 解析",
        "proxy" => "代理链路",
        "packet_loss" => "丢包率",
        "speed" => "下载测速",
        "disconnect" => "断开连接",
        "clean_proxy" => "清理代理",
        "flush_dns" => "刷新 DNS",
        "reconnect" => "重新连接",
        _ => step,
    };

    [RelayCommand]
    private async Task RunAsync()
    {
        Busy = true;
        Summary = "";
        Items.Clear();
        try
        {
            var resp = await _host.Client.InvokeAsync("diagnostics.run");
            if (resp.Ok)
                Summary = "诊断完成，详见各项结果";
            else
                Message = resp.Message;
        }
        catch (Exception ex) { Message = ex.Message; }
        finally { Busy = false; }
    }

    private void RunOnUi(Action a)
    {
        if (_dq.HasThreadAccess) a();
        else _dq.TryEnqueue(() => a());
    }

    [RelayCommand]
    private async Task RepairAsync()
    {
        Busy = true;
        Summary = "";
        Message = "";
        Items.Clear();
        var steps = new[] { "disconnect", "clean_proxy", "flush_dns", "reconnect" };
        foreach (var s in steps)
        {
            var line = AddLine(s);
            line.Status = "running";
            line.Glyph = "\uE9CE";
        }
        try
        {
            // 1. Disconnect
            UpdateStep("disconnect", "running", "正在断开…");
            var resp = await _host.Client.InvokeAsync("connection.disconnect");
            UpdateStep("disconnect", resp.Ok ? "ok" : "warn", resp.Ok ? "已断开" : resp.Message);
            await Task.Delay(1000);

            // 2. Clean system proxy (IPC: routing.setSystemProxy with enabled=false)
            UpdateStep("clean_proxy", "running", "清理系统代理…");
            try
            {
                // Direct registry write as a fallback if IPC doesn't support it
                var key = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(
                    @"Software\Microsoft\Windows\CurrentVersion\Internet Settings", true);
                if (key != null)
                {
                    key.SetValue("ProxyEnable", 0, Microsoft.Win32.RegistryValueKind.DWord);
                    key.DeleteValue("ProxyServer", false);
                    key.DeleteValue("ProxyOverride", false);
                    key.Close();
                }
                UpdateStep("clean_proxy", "ok", "注册表代理已清理");
            }
            catch (Exception ex)
            {
                UpdateStep("clean_proxy", "warn", ex.Message);
            }
            await Task.Delay(500);

            // 3. Flush DNS
            UpdateStep("flush_dns", "running", "刷新 DNS 缓存…");
            try
            {
                var psi = new System.Diagnostics.ProcessStartInfo
                {
                    FileName = "ipconfig",
                    Arguments = "/flushdns",
                    UseShellExecute = false,
                    CreateNoWindow = true,
                    RedirectStandardOutput = true,
                };
                var p = System.Diagnostics.Process.Start(psi);
                if (p != null)
                {
                    await p.WaitForExitAsync();
                    UpdateStep("flush_dns", "ok", "DNS 缓存已刷新");
                }
            }
            catch (Exception ex)
            {
                UpdateStep("flush_dns", "warn", ex.Message);
            }
            await Task.Delay(500);

            // 4. Reconnect (use last node+mode)
            UpdateStep("reconnect", "running", "正在重连…");
            resp = await _host.Client.InvokeAsync("connection.reconnect",
                ProtoWriter.ConnectRequest("", ""));
            if (resp.Ok)
                UpdateStep("reconnect", "ok", "已重连");
            else
                UpdateStep("reconnect", "fail", resp.Message);

            // 5. Quick connectivity check
            var ok = false;
            try
            {
                using var c = new System.Net.Http.HttpClient { Timeout = TimeSpan.FromSeconds(10) };
                var r = await c.GetAsync("https://www.gstatic.com/generate_204");
                ok = r.IsSuccessStatusCode;
            }
            catch { }

            Summary = ok ? "修复完成，网络连通正常" : "修复完成，但仍无法访问外网";
        }
        catch (Exception ex)
        {
            Message = ex.Message;
        }
        finally
        {
            Busy = false;
        }
    }

    private void UpdateStep(string step, string status, string detail)
    {
        RunOnUi(() =>
        {
            var line = Items.FirstOrDefault(i => i.Title == StepLabel(step));
            if (line != null)
            {
                line.Status = status;
                line.Detail = detail;
                line.Glyph = status switch
                {
                    "ok" => "\uE73E",
                    "fail" => "\uE783",
                    "warn" => "\uE7BA",
                    _ => "\uE9CE",
                };
            }
        });
    }

    [RelayCommand]
    private async Task LoadLogsAsync()
    {
        try
        {
            var resp = await _host.Client.InvokeAsync("logs.tail",
                System.Text.Encoding.UTF8.GetBytes("200"));
            if (resp.Ok && resp.Payload.Length > 0)
            {
                var json = System.Text.Encoding.UTF8.GetString(resp.Payload);
                var doc = System.Text.Json.JsonDocument.Parse(json);
                if (doc.RootElement.TryGetProperty("lines", out var lines))
                {
                    var sb = new System.Text.StringBuilder();
                    foreach (var line in lines.EnumerateArray())
                        sb.AppendLine(line.GetString());
                    LogText = sb.ToString();
                }
            }
        }
        catch { }
    }
}
