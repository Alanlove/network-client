using System.Diagnostics;
using System.IO;
using WindowsClient.Ipc;

namespace WindowsClient.Services;

/// <summary>
/// Owns the service process (optional auto-launch) and the single IPC
/// connection. Reconnects automatically when the pipe drops.
/// </summary>
public sealed class ServiceHost : IDisposable
{
    private readonly object _gate = new();
    private NetworkServiceClient? _client;
    private Process? _serviceProcess;
    private bool _disposed;

    public event EventHandler<ServiceEvent>? EventReceived;
    public event EventHandler<bool>? ConnectionChanged;

    public bool IsConnected => _client?.IsConnected ?? false;

    public NetworkServiceClient Client =>
        _client ?? throw new InvalidOperationException("service not connected");

    public async Task StartAsync()
    {
        if (!await TryEnsurePipeAsync())
        {
            TryLaunchService();
            if (!await TryEnsurePipeAsync())
                throw new InvalidOperationException(
                    "无法连接 network-service，请确认服务已安装并运行。");
        }
    }

    private async Task<bool> TryEnsurePipeAsync()
    {
        lock (_gate)
        {
            if (_client?.IsConnected == true) return true;
            _client?.Dispose();
            _client = new NetworkServiceClient();
        }
        _client.EventReceived += (_, ev) => EventReceived?.Invoke(this, ev);
        _client.Disconnected += async (_, _) =>
        {
            ConnectionChanged?.Invoke(this, false);
            await Task.Delay(1000);
            if (_disposed) return;
            _ = ReconnectLoopAsync();
        };
        var ok = await _client.ConnectWithRetriesAsync(attempts: 3).ConfigureAwait(false);
        if (ok) ConnectionChanged?.Invoke(this, true);
        return ok;
    }

    private async Task ReconnectLoopAsync()
    {
        while (!_disposed)
        {
            try
            {
                if (await TryEnsurePipeAsync().ConfigureAwait(false)) return;
            }
            catch { /* retry */ }
            await Task.Delay(2000).ConfigureAwait(false);
        }
    }

    private void TryLaunchService()
    {
        // Search order:
        // 1. Dev override via NC_SERVICE_EXE env var
        // 2. Installed side-by-side: {app}\bin\network-service.exe
        // 3. Legacy per-user fallback
        var candidates = new[]
        {
            Environment.GetEnvironmentVariable("NC_SERVICE_EXE"),
            Path.Combine(AppContext.BaseDirectory, "bin", "network-service.exe"),
            Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "NetworkClient", "bin", "network-service.exe"),
        };
        var exe = candidates.FirstOrDefault(File.Exists);
        if (exe is null) return;

        var psi = new ProcessStartInfo
        {
            FileName = exe,
            UseShellExecute = false,
            CreateNoWindow = true,
            WorkingDirectory = Path.GetDirectoryName(exe)!,
        };
        _serviceProcess = Process.Start(psi);
    }

    public void Dispose()
    {
        _disposed = true;
        lock (_gate) _client?.Dispose();
        // Do not kill a service we attached to from an external install.
        _serviceProcess?.Dispose();
    }
}
