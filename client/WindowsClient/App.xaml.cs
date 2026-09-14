using Microsoft.UI.Xaml;
using WindowsClient.Services;

namespace WindowsClient;

public partial class App : Application
{
    public static MainWindow MainWindow { get; private set; } = null!;
    public static ServiceHost Host { get; private set; } = null!;

    public enum ThemeMode { System, Light, Dark }
    public static ThemeMode CurrentTheme { get; private set; } = ThemeMode.System;

    public App()
    {
        UnhandledException += (_, e) =>
        {
            Program.Log("XAML UnhandledException", e.Exception);
            e.Handled = true;
        };
        InitializeComponent();
        Host = new ServiceHost();
    }

    public static void ApplyTheme(ThemeMode mode)
    {
        CurrentTheme = mode;
        if (MainWindow?.Content is FrameworkElement root)
        {
            root.RequestedTheme = mode switch
            {
                ThemeMode.Light => ElementTheme.Light,
                ThemeMode.Dark => ElementTheme.Dark,
                _ => ElementTheme.Default,
            };
        }
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        try
        {
            MainWindow = new MainWindow();
            MainWindow.Activate();
        }
        catch (Exception ex)
        {
            Program.Log("OnLaunched failed", ex);
            throw;
        }
    }
}
