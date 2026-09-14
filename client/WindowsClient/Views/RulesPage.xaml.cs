using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using WindowsClient.ViewModels;

namespace WindowsClient.Views;

public sealed partial class RulesPage : Page
{
    public RulesViewModel Vm { get; }

    public RulesPage()
    {
        InitializeComponent();
        Vm = new RulesViewModel(App.Host);
    }

    private void Mode_Click(object sender, RoutedEventArgs e)
    {
        if (sender is Button { Tag: string mode })
            _ = Vm.SetModeCommand.ExecuteAsync(mode);
    }
}
