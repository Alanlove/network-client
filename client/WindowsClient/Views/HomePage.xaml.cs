using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using WindowsClient.ViewModels;

namespace WindowsClient.Views;

public sealed partial class HomePage : Page
{
    public HomeViewModel Vm { get; }

    public HomePage()
    {
        InitializeComponent();
        Vm = new HomeViewModel(App.Host);
    }

    private void Page_Loaded(object sender, Microsoft.UI.Xaml.RoutedEventArgs e)
    {
        var upNotify = (System.Collections.Specialized.INotifyCollectionChanged)Vm.UpHistory;
        var downNotify = (System.Collections.Specialized.INotifyCollectionChanged)Vm.DownHistory;
        upNotify.CollectionChanged += (_, _) => DrawChart();
        downNotify.CollectionChanged += (_, _) => DrawChart();
        RateChart.SizeChanged += (_, _) => DrawChart();
        DrawChart();
    }

    private void DrawChart()
    {
        if (RateChart is null) return;
        var w = RateChart.ActualWidth;
        var h = RateChart.ActualHeight;
        if (w <= 0 || h <= 0) return;

        // Scale: at least 10 KB/s baseline so idle lines stay visible
        var maxRate = 10.0;
        foreach (var v in Vm.UpHistory) maxRate = Math.Max(maxRate, v);
        foreach (var v in Vm.DownHistory) maxRate = Math.Max(maxRate, v);

        RateChart.Children.Clear();
        DrawSeries(Vm.DownHistory, maxRate, w, h, 0x21, 0x96, 0xF3); // blue
        DrawSeries(Vm.UpHistory, maxRate, w, h, 0x4C, 0xAF, 0x50);   // green
    }

    private void DrawSeries(System.Collections.Generic.IList<double> data, double maxRate, double w, double h,
        byte r, byte g, byte b)
    {
        if (data.Count < 2) return;
        var pts = new PointCollection();
        var n = data.Count;
        for (int i = 0; i < n; i++)
        {
            var x = i / (double)Math.Max(n - 1, 1) * w;
            var y = h - data[i] / maxRate * h * 0.9;
            pts.Add(new Windows.Foundation.Point(x, y));
        }
        RateChart.Children.Add(new Polyline
        {
            Points = pts,
            Stroke = new SolidColorBrush(Windows.UI.Color.FromArgb(0xFF, r, g, b)),
            StrokeThickness = 1.5,
            StrokeLineJoin = PenLineJoin.Round,
        });
    }

    private void ModeSelector_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if ((sender as RadioButtons)?.SelectedItem is RadioButton rb && rb.Tag is string mode)
            Vm.SelectedMode = mode;
    }
}
