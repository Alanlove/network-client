using System.Collections.ObjectModel;
using System.Text.Json;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using WindowsClient.Ipc;
using WindowsClient.Models;
using WindowsClient.Services;

namespace WindowsClient.ViewModels;

public partial class HomeViewModel : ObservableObject
{
    private readonly ServiceHost _host;
    private readonly DispatcherQueue _dq;
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web);

    public ObservableCollection<Node> Nodes { get; } = new();

    // Rolling 60-point rate history for chart (60s window, 1s per sample)
    private readonly ObservableCollection<double> _upHistory = new();
    private readonly ObservableCollection<double> _downHistory = new();
    public ReadOnlyObservableCollection<double> UpHistory { get; }
    public ReadOnlyObservableCollection<double> DownHistory { get; }

    [ObservableProperty] private Node? selectedNode;
    [ObservableProperty] private string selectedMode = "smart";
    [ObservableProperty] private string stateText = "未连接";
    [ObservableProperty] private string stateGlyph = "\uE774"; // PlugDisconnected
    [ObservableProperty] private bool isConnected;
    [ObservableProperty] private bool busy;
    [ObservableProperty] private string nodeName = "—";
    [ObservableProperty] private string latencyText = "—";
    [ObservableProperty] private string upRateText = "0 KB/s";
    [ObservableProperty] private string downRateText = "0 KB/s";
    [ObservableProperty] private string todayText = "今日 ↑ 0 B  ↓ 0 B";
    [ObservableProperty] private string serviceStatus = "正在连接服务…";
    [ObservableProperty] private string statusMessage = "";

    public HomeViewModel(ServiceHost host)
    {
        _host = host;
        _dq = DispatcherQueue.GetForCurrentThread();
        UpHistory = new(_upHistory);
        DownHistory = new(_downHistory);
        _host.EventReceived += OnEvent;
        _host.ConnectionChanged += (_, ok) => RunOnUi(() => ServiceStatus = ok ? "服务已连接" : "服务连接中断，重连中…");
        _ = InitializeAsync();
    }

    private async Task InitializeAsync()
    {
        try
        {
            await _host.StartAsync();
            RunOnUi(() => ServiceStatus = "服务已连接");
            var state = await GetStateAsync();
            ApplyState(state);
            await LoadNodesAsync();
            await LoadTrafficAsync();
        }
        catch (Exception ex)
        {
            RunOnUi(() => ServiceStatus = ex.Message);
        }
    }

    private async Task<ConnectionStateInfo> GetStateAsync()
    {
        var resp = await _host.Client.InvokeAsync("connection.getStatus");
        if (!resp.Ok) return ConnectionStateInfo.Stopped;
        return ConnectionStateInfo.Decode(resp.Payload);
    }

    private async Task LoadTrafficAsync()
    {
        var resp = await _host.Client.InvokeAsync("statistics.get");
        if (resp.Ok) ApplyTraffic(TrafficInfo.Decode(resp.Payload));
    }

    public async Task LoadNodesAsync()
    {
        var resp = await _host.Client.InvokeAsync("node.list");
        if (!resp.Ok) return;
        var nodes = JsonSerializer.Deserialize<List<Node>>(resp.Text ?? "[]", Json) ?? new();
        RunOnUi(() =>
        {
            var keepId = SelectedNode?.Id;
            Nodes.Clear();
            foreach (var n in nodes.Where(n => n.Enabled)) Nodes.Add(n);
            SelectedNode = Nodes.FirstOrDefault(n => n.Id == keepId)
                ?? Nodes.FirstOrDefault(n => n.Id == StateNodeId)
                ?? Nodes.FirstOrDefault();
        });
    }

    private string StateNodeId { get; set; } = "";

    private void OnEvent(object? sender, ServiceEvent ev)
    {
        switch (ev.EventType)
        {
            case "connection.stateChanged":
                ApplyState(ConnectionStateInfo.Decode(ev.Payload));
                break;
            case "traffic.updated":
                ApplyTraffic(TrafficInfo.Decode(ev.Payload));
                break;
            case "subscription.updated":
                RunOnUi(async () => await LoadNodesAsync());
                break;
        }
    }

    private void ApplyState(ConnectionStateInfo s)
    {
        RunOnUi(() =>
        {
            StateNodeId = s.NodeId;
            StateText = s.State switch
            {
                "CONNECTED" => "已连接",
                "CONNECTING" => "连接中…",
                "STARTING" => "启动中…",
                "RECONNECTING" => "重连中…",
                "STOPPING" => "断开中…",
                "ERROR" => "连接错误",
                "READY" => "就绪",
                _ => "未连接",
            };
            IsConnected = s.State == "CONNECTED";
            StateGlyph = IsConnected ? "\uE930" : "\uE774";
            Busy = s.State is "CONNECTING" or "STARTING" or "RECONNECTING" or "STOPPING";
            NodeName = string.IsNullOrEmpty(s.NodeName) ? "—" : s.NodeName;
            LatencyText = s.LatencyMs > 0 ? $"{s.LatencyMs} ms" : "—";
            StatusMessage = s.State == "ERROR" ? s.ErrorMessage : "";
            if (!string.IsNullOrEmpty(s.NodeId))
                SelectedNode = Nodes.FirstOrDefault(n => n.Id == s.NodeId) ?? SelectedNode;
        });
    }

    private void ApplyTraffic(TrafficInfo t)
    {
        RunOnUi(() =>
        {
            UpRateText = FormatRate(t.UpRate);
            DownRateText = FormatRate(t.DownRate);
            TodayText = $"今日 ↑ {FormatBytes(t.UpToday)}  ↓ {FormatBytes(t.DownToday)}";
            // Push to chart history (KB/s, capped 60 points)
            _upHistory.Add(t.UpRate / 1000.0);
            _downHistory.Add(t.DownRate / 1000.0);
            while (_upHistory.Count > 60) _upHistory.RemoveAt(0);
            while (_downHistory.Count > 60) _downHistory.RemoveAt(0);
        });
    }

    [RelayCommand]
    private async Task ConnectAsync()
    {
        if (SelectedNode is null) return;
        await InvokeStateChanging("connection.connect", SelectedNode.Id);
    }

    [RelayCommand]
    private async Task DisconnectAsync() => await InvokeStateChanging("connection.disconnect", "");

    private async Task InvokeStateChanging(string method, string nodeId)
    {
        Busy = true;
        try
        {
            var payload = method == "connection.disconnect"
                ? null
                : ProtoWriter.ConnectRequest(nodeId, SelectedMode);
            var resp = await _host.Client.InvokeAsync(method, payload);
            if (!resp.Ok) StatusMessage = resp.Message;
        }
        catch (Exception ex)
        {
            StatusMessage = ex.Message;
        }
        finally
        {
            Busy = false;
        }
    }

    private void RunOnUi(Action a)
    {
        if (_dq.HasThreadAccess) a();
        else _dq.TryEnqueue(() => a());
    }

    public static string FormatRate(ulong bytesPerSec) =>
        bytesPerSec switch
        {
            >= 1_000_000 => $"{bytesPerSec / 1_000_000.0:0.0} MB/s",
            >= 1_000 => $"{bytesPerSec / 1_000.0:0.0} KB/s",
            _ => $"{bytesPerSec} B/s",
        };

    public static string FormatBytes(ulong b) =>
        b switch
        {
            >= 1UL << 30 => $"{b / (1UL << 30):0.00} GB",
            >= 1UL << 20 => $"{b / (1UL << 20):0.0} MB",
            >= 1UL << 10 => $"{b / (1UL << 10):0.0} KB",
            _ => $"{b} B",
        };
}
