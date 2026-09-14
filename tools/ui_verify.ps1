# UI verification helper: finds the WindowsClient window, walks the
# navigation items, and saves screenshots to %TEMP%\gui_<tag>.png.
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

function Find-Window {
    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $cond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ProcessIdProperty, (Get-Process WindowsClient).Id)
    return $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)
}

function Shoot($name) {
    Start-Sleep -Milliseconds 1200
    $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $path = Join-Path $env:TEMP "gui_$name.png"
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    Write-Output "saved $path"
}

$win = Find-Window
if (-not $win) { Write-Error "window not found"; exit 1 }
$walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
$items = $win.FindAll([System.Windows.Automation.TreeScope]::Descendants,
    (New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::ListItem)))

$targets = @('主页','节点','分流规则','网络诊断','设置')
foreach ($t in $targets) {
    $el = $null
    foreach ($i in $items) {
        if ($i.Current.Name -eq $t) { $el = $i; break }
    }
    if ($el) {
        try {
            $inv = $el.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
            $inv.Invoke()
        } catch {
            $sel = $el.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
            $sel.Select()
        }
        Shoot ($t)
    } else { Write-Output "item not found: $t" }
}
