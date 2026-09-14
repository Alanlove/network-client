using System.Text;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using WinRT;

namespace WindowsClient;

/// <summary>
/// Custom entry point (replaces the XAML-generated Main) so that startup
/// XAML/activation failures can be logged instead of surfacing only as a
/// native 0xC000027B stowed exception.
/// </summary>
public static class Program
{
    private static readonly string LogPath =
        System.IO.Path.Combine(
            AppContext.BaseDirectory, "client-startup.log");

    [STAThread]
    public static void Main(string[] args)
    {
        AppDomain.CurrentDomain.UnhandledException += (_, e) =>
            Log("AppDomain unhandled", e.ExceptionObject as Exception);
        System.Threading.Tasks.TaskScheduler.UnobservedTaskException += (_, e) =>
        {
            Log("UnobservedTask", e.Exception);
            e.SetObserved();
        };

        try
        {
            ComWrappersSupport.InitializeComWrappers();
            Application.Start(p =>
            {
                var context = new DispatcherQueueSynchronizationContext(
                    DispatcherQueue.GetForCurrentThread());
                SynchronizationContext.SetSynchronizationContext(context);
                try
                {
                    _ = new App();
                }
                catch (Exception ex)
                {
                    Log("App construction failed", ex);
                    throw;
                }
            });
        }
        catch (Exception ex)
        {
            Log("Application.Start failed", ex);
            throw;
        }
    }

    public static void Log(string stage, Exception? ex)
    {
        try
        {
            var dir = System.IO.Path.GetDirectoryName(LogPath);
            if (!string.IsNullOrEmpty(dir)) System.IO.Directory.CreateDirectory(dir);
            var sb = new StringBuilder();
            sb.AppendLine($"[{DateTimeOffset.Now:O}] {stage}");
            for (var e = ex; e is not null; e = e.InnerException)
            {
                sb.AppendLine($"  {e.GetType().FullName}: {e.Message}")
                  .AppendLine($"  HResult=0x{e.HResult:X8}")
                  .AppendLine(e.StackTrace);
            }
            System.IO.File.AppendAllText(LogPath, sb + Environment.NewLine);
        }
        catch
        {
            // never throw from the crash logger
        }
    }
}
