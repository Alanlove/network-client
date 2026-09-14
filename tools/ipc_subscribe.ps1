# Minimal IPC client for network-service named pipe `\\.\pipe\NetworkClient`.
# Frame: u32-LE length + protobuf body. Frozen contract: networkclient.v1.
# Usage examples:
#   .\ipc_subscribe.ps1 add        -Name smallstrawberry -Url '<url>'
#   .\ipc_subscribe.ps1 refresh    -Id <subscription-id>
#   .\ipc_subscribe.ps1 list
#   .\ipc_subscribe.ps1 batchTest
#   .\ipc_subscribe.ps1 connect    -NodeId <node-id> [-Mode smart]
#   .\ipc_subscribe.ps1 disconnect
#   .\ipc_subscribe.ps1 status
param(
    [Parameter(Position = 0)][string]$Action = "list",
    [string]$Name = "smallstrawberry",
    [string]$Url = "",
    [string]$Id = "",
    [string]$NodeId = "",
    [string]$Mode = "smart",
    [string]$Payload = ""
)

$ErrorActionPreference = "Stop"

function Write-Varint([System.Collections.Generic.List[byte]]$bytes, [long]$value) {
    while ($value -gt 0x7F) {
        [void]$bytes.Add([byte](($value -band 0x7F) -bor 0x80))
        $value = [math]::Floor($value / 128)
    }
    [void]$bytes.Add([byte]$value)
}

function Add-StringField([System.Collections.Generic.List[byte]]$body, [int]$field, [string]$s) {
    [void]$body.Add([byte](($field -shl 3) -bor 2))
    $utf8 = [Text.Encoding]::UTF8.GetBytes($s)
    Write-Varint $body $utf8.Length
    $body.AddRange($utf8)
}

function Add-BytesField([System.Collections.Generic.List[byte]]$body, [int]$field, [byte[]]$payload) {
    [void]$body.Add([byte](($field -shl 3) -bor 2))
    Write-Varint $body $payload.Length
    $body.AddRange($payload)
}

function Add-VarintField([System.Collections.Generic.List[byte]]$body, [int]$field, [long]$value) {
    [void]$body.Add([byte]($field -shl 3))
    Write-Varint $body $value
}

function Read-VarintAt([byte[]]$b, [ref]$pos) {
    [long]$result = 0; [int]$shift = 0; $p = $pos.Value
    while ($true) {
        $byte = $b[$p]; $p++
        $result = $result -bor (($byte -band 0x7F) -shl $shift)
        if (($byte -band 0x80) -eq 0) { break }
        $shift += 7
    }
    $pos.Value = $p
    return $result
}

# Parses a protobuf message into an ordered list of [field,wire,bytes/value] items.
function Read-Message([byte[]]$b) {
    $items = @()
    $p = 0
    while ($p -lt $b.Length) {
        $pos = [ref]$p
        $tag = Read-VarintAt $b $pos
        $p = $pos.Value
        $field = [int]($tag -shr 3); $wire = $tag -band 7
        switch ($wire) {
            0 { $pos = [ref]$p; $v = Read-VarintAt $b $pos; $p = $pos.Value; $items += ,@($field, 0, $v) }
            2 {
                $pos = [ref]$p; $len = [int](Read-VarintAt $b $pos); $p = $pos.Value
                $chunk = New-Object byte[] $len
                [Array]::Copy($b, $p, $chunk, 0, $len); $p += $len
                $items += ,@($field, 2, $chunk)
            }
            default { throw "wire type $wire not supported" }
        }
    }
    return ,$items
}

function Read-Frame($pipe) {
    $lenBuf = New-Object byte[] 4
    $off = 0
    while ($off -lt 4) {
        $n = $pipe.Read($lenBuf, $off, 4 - $off)
        if ($n -le 0) { throw "pipe closed before frame length" }
        $off += $n
    }
    $len = [BitConverter]::ToUInt32($lenBuf, 0)
    $buf = New-Object byte[] $len
    $off = 0
    while ($off -lt $len) {
        $n = $pipe.Read($buf, $off, [int]$len - $off)
        if ($n -le 0) { throw "pipe closed mid-frame" }
        $off += $n
    }
    return ,$buf
}

function Invoke-Ipc([string]$method, [byte[]]$payload) {
    $pipe = New-Object IO.Pipes.NamedPipeClientStream(".", "NetworkClient",
        [IO.Pipes.PipeDirection]::InOut, [IO.Pipes.PipeOptions]::None)
    $pipe.Connect(5000)
    try {
        $body = New-Object System.Collections.Generic.List[byte]
        $reqId = [guid]::NewGuid().ToString("N")
        Add-StringField $body 1 $reqId
        Add-StringField $body 2 $method
        if ($payload -and $payload.Length -gt 0) { Add-BytesField $body 3 $payload }
        Add-VarintField $body 4 1

        $frame = New-Object System.Collections.Generic.List[byte]
        $frame.AddRange([BitConverter]::GetBytes([uint32]$body.Count))
        $frame.AddRange($body.ToArray())
        $pipe.Write($frame.ToArray(), 0, $frame.Count)
        $pipe.Flush()

        # The server may broadcast EVENT frames on the same pipe (e.g. latency
        # updates during batchTest). Read frames until the response whose
        # field 1 (request_id) matches ours.
        $deadline = (Get-Date).AddSeconds(120)
        while ((Get-Date) -lt $deadline) {
            $respBuf = Read-Frame $pipe
            $fields = Read-Message $respBuf
            $respId = ""; $code = 0; $msg = ""; $respPayload = $null
            foreach ($f in $fields) {
                switch ($f[0]) {
                    1 { if ($f[1] -eq 2) { $respId = [Text.Encoding]::UTF8.GetString($f[2]) } }
                    2 { if ($f[1] -eq 0) { $code = [long]$f[2] } }
                    3 { if ($f[1] -eq 2) { $msg = [Text.Encoding]::UTF8.GetString($f[2]) } }
                    4 { if ($f[1] -eq 2) { $respPayload = $f[2] } }
                }
            }
            if ($respId -ne $reqId) { continue }
            return [pscustomobject]@{ Code = $code; Message = $msg; Payload = $respPayload }
        }
        throw "timed out waiting for response to $method (id=$reqId)"
    } finally {
        $pipe.Dispose()
    }
}

function New-TextPayload([string]$text) {
    $body = New-Object System.Collections.Generic.List[byte]
    Add-StringField $body 1 $text
    return $body.ToArray()
}

function Get-InnerText([byte[]]$payload) {
    if (-not $payload) { return "" }
    foreach ($f in (Read-Message $payload)) {
        if ($f[0] -eq 1 -and $f[1] -eq 2) { return [Text.Encoding]::UTF8.GetString($f[2]) }
    }
    return ""
}

switch ($Action) {
    "add" {
        $json = @{ name = $Name; url = $Url; enabled = $true } | ConvertTo-Json -Compress
        $r = Invoke-Ipc "subscription.add" (New-TextPayload $json)
    }
    "refresh" { $r = Invoke-Ipc "subscription.refresh" (New-TextPayload $Id) }
    "list" { $r = Invoke-Ipc "node.list" $null }
    "subs" { $r = Invoke-Ipc "subscription.list" $null }
    "batchTest" { $r = Invoke-Ipc "node.batchTest" $null }
    "status" { $r = Invoke-Ipc "connection.getStatus" $null }
    "disconnect" { $r = Invoke-Ipc "connection.disconnect" $null }
    "connect" {
        $p = New-Object System.Collections.Generic.List[byte]
        Add-StringField $p 1 $NodeId
        Add-StringField $p 2 $Mode
        $r = Invoke-Ipc "connection.connect" $p.ToArray()
    }
    # Generic escape hatch: call -Id '<frozen.method.name>' [-Payload 'text']
    "call" {
        $p = if ($Payload) { New-TextPayload $Payload } else { $null }
        $r = Invoke-Ipc $Id $p
    }
    default { throw "unknown action $Action" }
}

"code=$($r.Code) message=$($r.Message)"
if ($r.Payload) {
    $text = Get-InnerText $r.Payload
    if ($text) { $text } else { "($($r.Payload.Length) bytes binary payload)" }
}
