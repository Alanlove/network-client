using Microsoft.UI.Xaml.Controls;
using WindowsClient.ViewModels;

namespace WindowsClient.Views;

public sealed partial class NodesPage : Page
{
    public NodesViewModel Vm { get; }

    public NodesPage()
    {
        InitializeComponent();
        Vm = new NodesViewModel(App.Host);
    }
}
