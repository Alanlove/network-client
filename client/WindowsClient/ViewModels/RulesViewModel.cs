using System.Collections.ObjectModel;
using System.Text.Json;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Microsoft.UI.Dispatching;
using WindowsClient.Models;
using WindowsClient.Services;

namespace WindowsClient.ViewModels;

public partial class RulesViewModel : ObservableObject
{
    private readonly ServiceHost _host;
    private readonly DispatcherQueue _dq;
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web);

    public ObservableCollection<Rule> Rules { get; } = new();
    public string[] Kinds { get; } =
        { "domain", "domain_suffix", "domain_keyword", "ip_cidr", "port", "process", "geoip" };
    public string[] Actions { get; } = { "DIRECT", "PROXY", "BLOCK" };

    [ObservableProperty] private string selectedKind = "domain_suffix";
    [ObservableProperty] private string selectedAction = "DIRECT";
    [ObservableProperty] private string newValue = "";
    [ObservableProperty] private string message = "";
    [ObservableProperty] private string mode = "smart";
    [ObservableProperty] private string testInput = "";
    [ObservableProperty] private string testResult = "";

    public RulesViewModel(ServiceHost host)
    {
        _host = host;
        _dq = DispatcherQueue.GetForCurrentThread();
        _ = LoadAsync();
    }

    [RelayCommand]
    public async Task LoadAsync()
    {
        try
        {
            var rulesResp = await _host.Client.InvokeAsync("routing.listRules");
            var rules = rulesResp.Ok
                ? JsonSerializer.Deserialize<List<Rule>>(rulesResp.Text ?? "[]", Json) ?? new()
                : new List<Rule>();

            var cfgResp = await _host.Client.InvokeAsync("routing.getConfig");
            var mode = "smart";
            if (cfgResp.Ok)
            {
                using var doc = JsonDocument.Parse(cfgResp.Text ?? "{}");
                if (doc.RootElement.TryGetProperty("mode", out var m)) mode = m.GetString() ?? "smart";
            }

            RunOnUi(() =>
            {
                Rules.Clear();
                foreach (var r in rules.OrderByDescending(r => r.Priority)) Rules.Add(r);
                Mode = mode;
            });
        }
        catch (Exception ex) { Message = ex.Message; }
    }

    [RelayCommand]
    private async Task AddAsync()
    {
        if (string.IsNullOrWhiteSpace(NewValue)) return;
        var rule = new Rule
        {
            Id = Guid.NewGuid().ToString("N"),
            Kind = SelectedKind,
            Value = NewValue.Trim(),
            Action = SelectedAction,
            Enabled = true,
        };
        var resp = await _host.Client.InvokeTextAsync("routing.addRule", JsonSerializer.Serialize(rule, Json));
        if (resp.Ok) { NewValue = ""; await LoadAsync(); }
        else Message = resp.Message;
    }

    [RelayCommand]
    private async Task DeleteAsync(Rule? rule)
    {
        if (rule is null) return;
        var resp = await _host.Client.InvokeTextAsync("routing.deleteRule", rule.Id);
        if (resp.Ok) await LoadAsync();
        else Message = resp.Message;
    }

    [RelayCommand]
    private async Task SetModeAsync(string? mode)
    {
        if (string.IsNullOrEmpty(mode)) return;
        var resp = await _host.Client.InvokeTextAsync(
            "routing.setMode", JsonSerializer.Serialize(new { mode }, Json));
        if (resp.Ok) Mode = mode;
        else Message = resp.Message;
    }

    [RelayCommand]
    private async Task TestRuleAsync()
    {
        if (string.IsNullOrWhiteSpace(TestInput)) return;
        var resp = await _host.Client.InvokeTextAsync("routing.test", TestInput.Trim());
        if (resp.Ok && !string.IsNullOrEmpty(resp.Text))
        {
            using var doc = JsonDocument.Parse(resp.Text);
            var root = doc.RootElement;
            var action = root.GetProperty("action").GetString() ?? "?";
            var reason = root.TryGetProperty("reason", out var r) ? r.GetString() ?? "" : "";
            var ruleId = root.TryGetProperty("matched_rule_id", out var ri) ? ri.GetString() : null;
            var mode = root.TryGetProperty("mode", out var m) ? m.GetString() ?? "" : "";
            var arrow = action == "direct" ? "→ 直连" : action == "proxy" ? "→ 代理" : action == "block" ? "→ 阻断" : $"→ {action}";
            var match = ruleId != null ? $"（匹配规则 {ruleId}）" : "（默认策略）";
            RunOnUi(() => TestResult = $"{TestInput} {arrow} {match}\n原因: {reason}\n模式: {mode}");
        }
        else
        {
            RunOnUi(() => TestResult = resp.Message);
        }
    }

    private void RunOnUi(Action a)
    {
        if (_dq.HasThreadAccess) a();
        else _dq.TryEnqueue(() => a());
    }
}
