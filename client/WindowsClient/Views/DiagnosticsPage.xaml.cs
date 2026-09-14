using Microsoft.UI.Xaml.Controls;
using WindowsClient.ViewModels;

namespace WindowsClient.Views;

public sealed partial class DiagnosticsPage : Page
{
    public DiagnosticsViewModel Vm { get; }

    public DiagnosticsPage()
    {
        InitializeComponent();
        Vm = new DiagnosticsViewModel(App.Host);
    }
}
