using System.Collections.ObjectModel;
using System.Text.Json;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Microsoft.UI.Dispatching;
using WindowsClient.Models;
using WindowsClient.Services;

namespace WindowsClient.ViewModels;

public partial class NodeRow : ObservableObject
{
    public Node Node { get; }
    public NodeRow(Node node) => Node = node;

    public string Id => Node.Id;
    public string Name => Node.Name;
    public string Endpoint => $"{Node.Protocol}  {Node.Endpoint.Host}:{Node.Endpoint.Port}";
    public string Source => string.IsNullOrEmpty(Node.SubscriptionId) ? "手动" : "订阅";

    [ObservableProperty] private string latency = "—";
    [ObservableProperty] private string score = "—";
}

public partial class NodesViewModel : ObservableObject
{
    private readonly ServiceHost _host;
    private readonly DispatcherQueue _dq;
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web);

    public ObservableCollection<NodeRow> Nodes { get; } = new();

    [ObservableProperty] private bool busy;
    [ObservableProperty] private string message = "";

    public NodesViewModel(ServiceHost host)
    {
        _host = host;
        _dq = DispatcherQueue.GetForCurrentThread();
        _ = LoadAsync();
    }

    [RelayCommand]
    public async Task LoadAsync()
    {
        Busy = true;
        try
        {
            var resp = await _host.Client.InvokeAsync("node.list");
            if (!resp.Ok) { Message = resp.Message; return; }
            var nodes = JsonSerializer.Deserialize<List<Node>>(resp.Text ?? "[]", Json) ?? new();
            RunOnUi(() =>
            {
                Nodes.Clear();
                foreach (var n in nodes) Nodes.Add(new NodeRow(n));
                Message = $"{nodes.Count} 个节点";
            });
        }
        catch (Exception ex) { Message = ex.Message; }
        finally { Busy = false; }
    }

    [RelayCommand]
    private async Task BatchTestAsync()
    {
        Busy = true;
        Message = "测速中…";
        try
        {
            var resp = await _host.Client.InvokeAsync("node.batchTest");
            if (!resp.Ok) { Message = resp.Message; return; }
            using var doc = JsonDocument.Parse(resp.Text ?? "{}");
            RunOnUi(() =>
            {
                foreach (var row in Nodes)
                {
                    if (!doc.RootElement.TryGetProperty(row.Id, out var el)) continue;
                    var latency = el.TryGetProperty("latency_ms", out var l) && l.ValueKind == JsonValueKind.Number
                        ? l.GetInt64() : 0;
                    row.Latency = latency > 0 ? $"{latency} ms" : "超时";
                    if (el.TryGetProperty("score", out var sc) && sc.ValueKind == JsonValueKind.Number)
                        row.Score = $"{sc.GetDouble():0}";
                }
                Message = "测速完成";
            });
        }
        catch (Exception ex) { Message = ex.Message; }
        finally { Busy = false; }
    }

    [RelayCommand]
    private async Task SelectAsync(NodeRow? row)
    {
        if (row is null) return;
        var resp = await _host.Client.InvokeTextAsync("node.select", row.Id);
        Message = resp.Ok ? $"已选择 {row.Name}" : resp.Message;
    }

    [RelayCommand]
    private async Task DeleteAsync(NodeRow? row)
    {
        if (row is null) return;
        var resp = await _host.Client.InvokeTextAsync("node.delete", row.Id);
        if (resp.Ok) await LoadAsync();
        else Message = resp.Message;
    }

    private void RunOnUi(Action a)
    {
        if (_dq.HasThreadAccess) a();
        else _dq.TryEnqueue(() => a());
    }
}
