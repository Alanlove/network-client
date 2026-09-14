# 一键启动图形客户端（开发模式）
#
# 启动链：WindowsClient.exe
#          └─ 发现命名管道 \\.\pipe\NetworkClient 不存在时
#             按 NC_SERVICE_EXE 自动拉起 network-service.exe
#                └─ 继承 NC_DATA_DIR，读取 data\ 下的配置/节点/内核
#
# 若管道已存在（服务已在跑），客户端直接附加，不会重复拉起。
[CmdletBinding()]
param(
    [string]$Configuration = "Debug"
)

$repo = Split-Path -Parent $PSScriptRoot
$serviceExe = Join-Path $repo "service\network-service\target\$Configuration\network-service.exe"
$clientExe  = Join-Path $repo "client\WindowsClient\bin\x64\$Configuration\net9.0-windows10.0.19041.0\WindowsClient.exe"
$dataDir    = Join-Path $repo "data"

if (-not (Test-Path $serviceExe)) { throw "service exe 不存在，请先 cargo build：$serviceExe" }
if (-not (Test-Path $clientExe))  { throw "client exe 不存在，请先 dotnet build：$clientExe" }
if (-not (Test-Path (Join-Path $dataDir "bin\xray.exe"))) { throw "data\bin\xray.exe 缺失" }

$env:NC_DATA_DIR    = $dataDir
$env:NC_SERVICE_EXE = $serviceExe

Start-Process -FilePath $clientExe -WorkingDirectory (Split-Path $clientExe)
"已启动客户端 (NC_DATA_DIR=$dataDir)"
