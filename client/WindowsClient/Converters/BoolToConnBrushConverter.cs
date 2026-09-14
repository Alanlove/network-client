using Microsoft.UI;
using Microsoft.UI.Xaml.Data;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace WindowsClient.Converters;

/// <summary>
/// Bool → SolidColorBrush converter for connection status indicator.
/// True = success brush, False = neutral brush.
/// </summary>
public partial class BoolToConnBrushConverter : IValueConverter
{
    private static readonly Color Success = Color.FromArgb(0xFF, 0x10, 0xB9, 0x81); // #10B981
    private static readonly Color Neutral = Color.FromArgb(0xFF, 0x9C, 0xA3, 0xAF); // #9CA3AF

    public object Convert(object value, Type targetType, object parameter, string language)
    {
        var isConnected = value is bool b && b;
        return new SolidColorBrush(isConnected ? Success : Neutral);
    }

    public object ConvertBack(object value, Type targetType, object parameter, string language)
        => throw new NotSupportedException();
}
