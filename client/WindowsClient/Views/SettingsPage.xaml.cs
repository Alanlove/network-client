using Microsoft.UI.Xaml.Controls;
using WindowsClient.ViewModels;

namespace WindowsClient.Views;

public sealed partial class SettingsPage : Page
{
    public SettingsViewModel Vm { get; }

    public SettingsPage()
    {
        InitializeComponent();
        Vm = new SettingsViewModel(App.Host);
    }

    private void Theme_Click(object sender, Microsoft.UI.Xaml.RoutedEventArgs e)
    {
        if (sender is RadioButton rb && rb.Tag is string tag)
            Vm.SetTheme(tag);
    }
}
