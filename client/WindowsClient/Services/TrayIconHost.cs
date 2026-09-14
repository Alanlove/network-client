using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;

namespace WindowsClient.Services;

/// <summary>
/// System tray icon via Win32 Shell_NotifyIcon on a dedicated hidden-window
/// thread. Icons are generated in memory (32bpp ARGB circles), so no .ico
/// asset is required. Menu actions are marshalled back to the UI thread.
/// </summary>
public sealed class TrayIconHost : IDisposable
{
    private const uint WM_APP_TRAY = 0x8000;          // WM_APP
    private const uint WM_LBUTTONDBLCLK = 0x0203;
    private const uint WM_RBUTTONUP = 0x0205;
    private const int NIM_ADD = 0, NIM_MODIFY = 1, NIM_DELETE = 2;
    private const uint NIF_MESSAGE = 0x1, NIF_ICON = 0x2, NIF_TIP = 0x4;
    private const uint MF_STRING = 0x0, MF_SEPARATOR = 0x800, MF_GRAYED = 0x1;
    private const uint TPM_RETURNCMD = 0x100, TPM_RIGHTBUTTON = 0x2;
    private const uint WM_CLOSE = 0x0010, WM_COMMAND = 0x0111, WM_QUIT = 0x0012;
    private const int MENU_OPEN = 1, MENU_CONNECT = 2, MENU_DISCONNECT = 3, MENU_EXIT = 4;

    private delegate IntPtr WndProcDelegate(IntPtr hwnd, uint msg, IntPtr wParam, IntPtr lParam);

    private readonly DispatcherQueue _dq;
    private readonly Action _showWindow;
    private readonly Action _connect;
    private readonly Action _disconnect;
    private readonly Action _exit;
    private readonly ManualResetEvent _ready = new(false);

    private WndProcDelegate? _wndProcRef;   // GC keep-alive
    private IntPtr _hwnd;
    private IntPtr _hIconOn, _hIconOff;
    private volatile bool _connected;
    private bool _disposed;

    public TrayIconHost(DispatcherQueue dq, Action showWindow, Action connect, Action disconnect, Action exit)
    {
        _dq = dq;
        _showWindow = showWindow;
        _connect = connect;
        _disconnect = disconnect;
        _exit = exit;

        _hIconOn = CreateCircleIcon(0x2E, 0xB8, 0x57);   // green
        _hIconOff = CreateCircleIcon(0x9E, 0x9E, 0x9E);  // gray

        var worker = new Thread(Run)
        {
            IsBackground = true,
            Name = "TrayIconThread",
        };
        worker.SetApartmentState(ApartmentState.STA);
        worker.Start();
        _ready.WaitOne(TimeSpan.FromSeconds(3));

        var nid = NewBase();
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = _hIconOff;
        nid.szTip = "网络代理客户端 — 未连接";
        var ok = Shell_NotifyIconW(NIM_ADD, ref nid);
        if (!ok)
            Program.Log("TrayIconHost", new Exception($"Shell_NotifyIconW(ADD) failed"));
    }

    /// <summary>Update tray icon and tooltip. Thread-safe (shell call only).</summary>
    public void SetConnected(bool on)
    {
        if (_disposed) return;
        _connected = on;
        var nid = NewBase();
        nid.uFlags = NIF_ICON | NIF_TIP;
        nid.hIcon = on ? _hIconOn : _hIconOff;
        nid.szTip = on ? "网络代理客户端 — 已连接" : "网络代理客户端 — 未连接";
        Shell_NotifyIconW(NIM_MODIFY, ref nid);
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        var nid = NewBase();
        Shell_NotifyIconW(NIM_DELETE, ref nid);
        if (_hwnd != IntPtr.Zero) PostMessageW(_hwnd, WM_CLOSE, IntPtr.Zero, IntPtr.Zero);
        if (_hIconOn != IntPtr.Zero) DestroyIcon(_hIconOn);
        if (_hIconOff != IntPtr.Zero) DestroyIcon(_hIconOff);
    }

    // ---- worker thread: hidden window + message pump --------------------

    private void Run()
    {
        _wndProcRef = WndProc;
        var hInstance = GetModuleHandleW(null);
        var className = Marshal.StringToHGlobalUni("NC_TrayWnd");
        var wc = new WNDCLASS
        {
            lpfnWndProc = Marshal.GetFunctionPointerForDelegate(_wndProcRef),
            lpszClassName = className,
            hInstance = hInstance,
        };
        var atom = RegisterClassW(ref wc);
        const uint WS_OVERLAPPEDWINDOW = 0x00CF0000;
        _hwnd = CreateWindowExW(0x80, className, IntPtr.Zero, WS_OVERLAPPEDWINDOW,
            0, 0, 0, 0, IntPtr.Zero, IntPtr.Zero, hInstance, IntPtr.Zero);
        Marshal.FreeHGlobal(className);
        if (_hwnd == IntPtr.Zero)
        {
            var err = System.Runtime.InteropServices.Marshal.GetLastWin32Error();
            Program.Log("TrayIconHost", new Exception($"CreateWindowExW failed lastError={err}"));
        }
        _ready.Set();

        while (GetMessageW(out var msg, IntPtr.Zero, 0, 0) > 0)
        {
            TranslateMessage(ref msg);
            DispatchMessageW(ref msg);
        }
    }

    private IntPtr WndProc(IntPtr hwnd, uint msg, IntPtr wParam, IntPtr lParam)
    {
        switch (msg)
        {
            case WM_APP_TRAY:
                var mouse = (uint)(lParam.ToInt64() & 0xFFFF);
                if (mouse == WM_LBUTTONDBLCLK)
                    _dq.TryEnqueue(new DispatcherQueueHandler(_showWindow));
                else if (mouse == WM_RBUTTONUP)
                    ShowContextMenu();
                return IntPtr.Zero;

            case WM_COMMAND:
                switch ((uint)(wParam.ToInt64() & 0xFFFF))
                {
                    case MENU_OPEN: _dq.TryEnqueue(new DispatcherQueueHandler(_showWindow)); break;
                    case MENU_CONNECT: _dq.TryEnqueue(new DispatcherQueueHandler(_connect)); break;
                    case MENU_DISCONNECT: _dq.TryEnqueue(new DispatcherQueueHandler(_disconnect)); break;
                    case MENU_EXIT: _dq.TryEnqueue(new DispatcherQueueHandler(_exit)); break;
                }
                return IntPtr.Zero;

            case WM_CLOSE:
                DestroyWindow(hwnd);
                PostQuitMessage(0);
                return IntPtr.Zero;
        }
        return DefWindowProcW(hwnd, msg, wParam, lParam);
    }

    private void ShowContextMenu()
    {
        var menu = CreatePopupMenu();
        AppendMenuW(menu, MF_STRING, MENU_OPEN, "打开主窗口");
        AppendMenuW(menu, MF_STRING | (_connected ? MF_GRAYED : 0), MENU_CONNECT, "连接");
        AppendMenuW(menu, MF_STRING | (_connected ? 0 : MF_GRAYED), MENU_DISCONNECT, "断开");
        AppendMenuW(menu, MF_SEPARATOR, 0, null);
        AppendMenuW(menu, MF_STRING, MENU_EXIT, "退出");
        SetForegroundWindow(_hwnd); // required so the menu dismisses on outside click
        GetCursorPos(out var p);
        var id = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.X, p.Y, 0, _hwnd, IntPtr.Zero);
        DestroyMenu(menu);
        if (id != 0)
            PostMessageW(_hwnd, WM_COMMAND, (IntPtr)(long)id, IntPtr.Zero);
    }

    private NOTIFYICONDATA NewBase() => new()
    {
        cbSize = Marshal.SizeOf<NOTIFYICONDATA>(),
        hWnd = _hwnd,
        uID = 1,
        szTip = string.Empty,
        szInfo = string.Empty,
        szInfoTitle = string.Empty,
    };

    // ---- in-memory icon: solid ARGB circle ------------------------------

    private static IntPtr CreateCircleIcon(byte r, byte g, byte b)
    {
        const int size = 16;
        var bmi = new BITMAPINFO
        {
            bmiHeader = new BITMAPINFOHEADER
            {
                biSize = (uint)Marshal.SizeOf<BITMAPINFOHEADER>(),
                biWidth = size,
                biHeight = -size, // top-down
                biPlanes = 1,
                biBitCount = 32,
                biCompression = 0, // BI_RGB
            },
        };
        IntPtr hdc = GetDC(IntPtr.Zero);
        IntPtr hColor = CreateDIBSection(hdc, ref bmi, 0 /*DIB_RGB_COLORS*/, out IntPtr bits, IntPtr.Zero, 0);
        ReleaseDC(IntPtr.Zero, hdc);

        var px = new byte[size * size * 4];
        double cx = (size - 1) / 2.0, cy = (size - 1) / 2.0, rad = size / 2.0 - 1.0;
        for (int y = 0; y < size; y++)
        {
            for (int x = 0; x < size; x++)
            {
                double dx = x - cx, dy = y - cy;
                if (dx * dx + dy * dy <= rad * rad)
                {
                    int i = (y * size + x) * 4;
                    px[i] = b; px[i + 1] = g; px[i + 2] = r; px[i + 3] = 0xFF;
                }
            }
        }
        if (hColor != IntPtr.Zero && bits != IntPtr.Zero)
            Marshal.Copy(px, 0, bits, px.Length);

        // All-zero 1bpp mask: for 32bpp color icons this defers to alpha.
        var maskBytes = new byte[((size + 15) / 16) * 2 * size];
        IntPtr hMask = CreateBitmap(size, size, 1, 1, maskBytes);

        var ii = new ICONINFO { fIcon = 1, hbmMask = hMask, hbmColor = hColor };
        IntPtr hIcon = CreateIconIndirect(ref ii);
        DeleteObject(hMask);
        DeleteObject(hColor);
        return hIcon;
    }

    // ---- Win32 -----------------------------------------------------------

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct NOTIFYICONDATA
    {
        public int cbSize;
        public IntPtr hWnd;
        public uint uID;
        public uint uFlags;
        public uint uCallbackMessage;
        public IntPtr hIcon;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string szTip;
        public uint dwState;
        public uint dwStateMask;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)] public string szInfo;
        public uint uVersion;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)] public string szInfoTitle;
        public uint dwInfoFlags;
        public Guid guidItem;
        public IntPtr hBalloonIcon;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ICONINFO
    {
        public int fIcon;
        public int xHotspot;
        public int yHotspot;
        public IntPtr hbmMask;
        public IntPtr hbmColor;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BITMAPINFOHEADER
    {
        public uint biSize;
        public int biWidth;
        public int biHeight;
        public ushort biPlanes;
        public ushort biBitCount;
        public uint biCompression;
        public uint biSizeImage;
        public int biXPelsPerMeter;
        public int biYPelsPerMeter;
        public uint biClrUsed;
        public uint biClrImportant;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BITMAPINFO
    {
        public BITMAPINFOHEADER bmiHeader;
        public uint bmiColors;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct WNDCLASS
    {
        public uint style;
        public IntPtr lpfnWndProc;
        public int cbClsExtra;
        public int cbWndExtra;
        public IntPtr hInstance;
        public IntPtr hIcon;
        public IntPtr hCursor;
        public IntPtr hbrBackground;
        public IntPtr lpszMenuName;
        public IntPtr lpszClassName;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MSG
    {
        public IntPtr hwnd;
        public uint message;
        public IntPtr wParam;
        public IntPtr lParam;
        public uint time;
        public int ptX;
        public int ptY;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct POINT { public int X; public int Y; }

    [DllImport("shell32.dll", EntryPoint = "Shell_NotifyIconW", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool Shell_NotifyIconW(int msg, ref NOTIFYICONDATA data);

    [DllImport("user32.dll", EntryPoint = "RegisterClassW", SetLastError = true)]
    private static extern ushort RegisterClassW(ref WNDCLASS wc);

    [DllImport("user32.dll", EntryPoint = "CreateWindowExW", SetLastError = true)]
    private static extern IntPtr CreateWindowExW(uint exStyle, IntPtr cls, IntPtr name, uint style,
        int x, int y, int w, int h, IntPtr parent, IntPtr menu, IntPtr inst, IntPtr param);

    [DllImport("user32.dll")]
    private static extern IntPtr DefWindowProcW(IntPtr hwnd, uint msg, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DestroyWindow(IntPtr hwnd);

    [DllImport("user32.dll")]
    private static extern int GetMessageW(out MSG msg, IntPtr hWnd, uint min, uint max);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool TranslateMessage(ref MSG msg);

    [DllImport("user32.dll")]
    private static extern IntPtr DispatchMessageW(ref MSG msg);

    [DllImport("user32.dll")]
    private static extern void PostQuitMessage(int code);

    [DllImport("user32.dll", EntryPoint = "PostMessageW")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool PostMessageW(IntPtr hwnd, uint msg, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetForegroundWindow(IntPtr hwnd);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetCursorPos(out POINT p);

    [DllImport("user32.dll")]
    private static extern IntPtr CreatePopupMenu();

    [DllImport("user32.dll", EntryPoint = "AppendMenuW", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool AppendMenuW(IntPtr menu, uint flags, uint id, string? text);

    [DllImport("user32.dll")]
    private static extern int TrackPopupMenu(IntPtr menu, uint flags, int x, int y, int reserved, IntPtr hwnd, IntPtr rect);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DestroyMenu(IntPtr menu);

    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern IntPtr CreateDIBSection(IntPtr hdc, ref BITMAPINFO bmi, uint usage,
        out IntPtr bits, IntPtr section, uint offset);

    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern IntPtr CreateBitmap(int w, int h, uint planes, uint bitsPerPixel, byte[]? data);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr CreateIconIndirect(ref ICONINFO info);

    [DllImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DeleteObject(IntPtr obj);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DestroyIcon(IntPtr hIcon);

    [DllImport("user32.dll")]
    private static extern IntPtr GetDC(IntPtr hwnd);

    [DllImport("user32.dll")]
    private static extern int ReleaseDC(IntPtr hwnd, IntPtr hdc);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr GetModuleHandleW(string? name);
}
