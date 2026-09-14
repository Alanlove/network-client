using System.Collections.ObjectModel;
using System.Text.Json;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Microsoft.UI.Dispatching;
using WindowsClient.Ipc;
using WindowsClient.Models;
using WindowsClient.Services;

namespace WindowsClient.ViewModels;

public partial class SettingsViewModel : ObservableObject
{
    private readonly ServiceHost _host;
    private readonly DispatcherQueue _dq;
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web);

    public ObservableCollection<SubscriptionView> Subscriptions { get; } = new();

    [ObservableProperty] private string newName = "";
    [ObservableProperty] private string newUrl = "";
    [ObservableProperty] private string versionText = "—";
    [ObservableProperty] private string message = "";
    [ObservableProperty] private bool busy;

    [ObservableProperty] private bool isThemeSystem = true;
    [ObservableProperty] private bool isThemeLight;
    [ObservableProperty] private bool isThemeDark;

    private App.ThemeMode _theme = App.ThemeMode.System;

    public SettingsViewModel(ServiceHost host)
    {
        _host = host;
        _dq = DispatcherQueue.GetForCurrentThread();
        _host.EventReceived += OnEvent;
        _ = LoadAllAsync();
    }

    public void SetTheme(string tag)
    {
        _theme = tag switch
        {
            "Light" => App.ThemeMode.Light,
            "Dark" => App.ThemeMode.Dark,
            _ => App.ThemeMode.System,
        };
        IsThemeSystem = _theme == App.ThemeMode.System;
        IsThemeLight = _theme == App.ThemeMode.Light;
        IsThemeDark = _theme == App.ThemeMode.Dark;
        App.ApplyTheme(_theme);
    }

    private void OnEvent(object? sender, ServiceEvent ev)
    {
        switch (ev.EventType)
        {
            case "subscription.updated":
            case "subscription.failed":
                RunOnUi(async () => await LoadSubscriptionsAsync());
                break;
        }
    }

    private async Task LoadAllAsync()
    {
        try
        {
            var ver = await _host.Client.InvokeAsync("system.getVersion");
            if (ver.Ok)
            {
                var v = VersionInfo.Decode(ver.Payload);
                RunOnUi(() => VersionText =
                    $"协议 v{v.ProtocolVersion}  ·  服务 {v.ServiceVersion}  ·  内核 {v.CoreVersion}");
            }
            await LoadSubscriptionsAsync();
        }
        catch (Exception ex) { Message = ex.Message; }
    }

    [RelayCommand]
    public async Task LoadSubscriptionsAsync()
    {
        var resp = await _host.Client.InvokeAsync("subscription.list");
        if (!resp.Ok) return;
        var items = JsonSerializer.Deserialize<List<SubscriptionView>>(resp.Text ?? "[]", Json) ?? new();
        RunOnUi(() =>
        {
            Subscriptions.Clear();
            foreach (var s in items) Subscriptions.Add(s);
        });
    }

    [RelayCommand]
    private async Task AddSubscriptionAsync()
    {
        if (string.IsNullOrWhiteSpace(NewName) || string.IsNullOrWhiteSpace(NewUrl)) return;
        Busy = true;
        var input = new SubscriptionInput { Name = NewName.Trim(), Url = NewUrl.Trim() };
        var resp = await _host.Client.InvokeTextAsync(
            "subscription.add", JsonSerializer.Serialize(input, Json));
        Busy = false;
        if (resp.Ok)
        {
            NewName = "";
            NewUrl = "";
            Message = "订阅已添加，正在同步节点…";
            await LoadSubscriptionsAsync();
        }
        else Message = resp.Message;
    }

    [RelayCommand]
    private async Task RefreshAsync(SubscriptionView? sub)
    {
        if (sub is null) return;
        Message = $"刷新订阅 {sub.Name} …";
        var resp = await _host.Client.InvokeTextAsync("subscription.refresh", sub.Id);
        if (!resp.Ok) Message = resp.Message;
    }

    [RelayCommand]
    private async Task DeleteAsync(SubscriptionView? sub)
    {
        if (sub is null) return;
        var resp = await _host.Client.InvokeTextAsync("subscription.delete", sub.Id);
        if (resp.Ok) await LoadSubscriptionsAsync();
        else Message = resp.Message;
    }

    private void RunOnUi(Action a)
    {
        if (_dq.HasThreadAccess) a();
        else _dq.TryEnqueue(() => a());
    }
}
