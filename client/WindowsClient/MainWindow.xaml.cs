using Microsoft.UI;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.Graphics;
using WindowsClient.Ipc;
using WindowsClient.Services;
using WindowsClient.Views;
using WinRT.Interop;

namespace WindowsClient;

public sealed partial class MainWindow : Window
{
    private AppWindow? _appWindow;
    private TrayIconHost? _tray;
    private bool _exitRequested;

    public MainWindow()
    {
        InitializeComponent();

        // Window.Width/Height only exist in WinAppSDK 1.7+; on 1.6 size the
        // window through AppWindow.
        var hWnd = WindowNative.GetWindowHandle(this);
        var windowId = Win32Interop.GetWindowIdFromWindow(hWnd);
        if (AppWindow.GetFromWindowId(windowId) is { } appWindow)
        {
            _appWindow = appWindow;
            appWindow.Resize(new SizeInt32(960, 680));
            // Close button minimizes to tray instead of exiting.
            appWindow.Closing += (s, e) =>
            {
                if (!_exitRequested)
                {
                    e.Cancel = true;
                    s.Hide();
                }
            };
        }

        InitTray();
        Nav.SelectedItem = Nav.MenuItems[0];
    }

    private void InitTray()
    {
        try
        {
            _tray = new TrayIconHost(
                DispatcherQueue.GetForCurrentThread(),
                showWindow: ShowMainWindow,
                connect: () => _ = InvokeAsync("connection.reconnect", ProtoWriter.ConnectRequest("", "")),
                disconnect: () => _ = InvokeAsync("connection.disconnect", null),
                exit: RequestExit);
            App.Host.EventReceived += OnEventForTray;
            _ = RefreshTrayStateAsync();
        }
        catch (Exception ex)
        {
            // Tray is auxiliary — never block the main window.
            _tray = null;
            Program.Log("InitTray failed", ex);
        }
    }

    private void OnEventForTray(object? sender, ServiceEvent ev)
    {
        if (ev.EventType == "connection.stateChanged")
            _tray?.SetConnected(ConnectionStateInfo.Decode(ev.Payload).State == "CONNECTED");
    }

    private async Task RefreshTrayStateAsync()
    {
        try
        {
            var r = await App.Host.Client.InvokeAsync("connection.getStatus");
            if (r.Ok)
                _tray?.SetConnected(ConnectionStateInfo.Decode(r.Payload).State == "CONNECTED");
        }
        catch { /* service not up yet; events will update later */ }
    }

    private Task InvokeAsync(string method, byte[]? payload) => App.Host.Client.InvokeAsync(method, payload);

    private void ShowMainWindow()
    {
        _appWindow?.Show();
        Activate();
    }

    private void RequestExit()
    {
        _exitRequested = true;
        _tray?.Dispose();
        Close();
    }

    private void Nav_SelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItem is not NavigationViewItem item || item.Tag is not string tag)
            return;

        ContentFrame.Navigate(tag switch
        {
            "home" => typeof(HomePage),
            "nodes" => typeof(NodesPage),
            "rules" => typeof(RulesPage),
            "diag" => typeof(DiagnosticsPage),
            "settings" => typeof(SettingsPage),
            _ => typeof(HomePage),
        });
    }
}
