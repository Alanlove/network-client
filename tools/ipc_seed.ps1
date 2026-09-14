# Seeds two nodes and one routing rule through the Named Pipe IPC for
# end-to-end UI verification. Frame: u32-LE length + protobuf Request.
#   Request: field1=request_id, field2=method, field3=TextPayload(text=field1),
#            field4=protocol_version(varint)
param(
    [string]$PipeName = "NetworkClient"
)

function New-Varint([ulong]$v) {
    $bytes = New-Object System.Collections.Generic.List[byte]
    while ($v -ge 0x80) { $bytes.Add([byte]($v -bor 0x80)); $v = $v -shr 7 }
    $bytes.Add([byte]$v)
    return $bytes.ToArray()
}

function New-LenField([int]$field, [byte[]]$data) {
    $tag = ($field -shl 3) -bor 2
    $out = New-Object System.Collections.Generic.List[byte]
    $out.AddRange([byte[]](New-Varint $tag))
    $out.AddRange([byte[]](New-Varint ([ulong]$data.Length)))
    $out.AddRange([byte[]]$data)
    return $out.ToArray()
}

function Build-Frame([string]$id, [string]$method, [string]$text) {
    $body = New-Object System.Collections.Generic.List[byte]
    $body.AddRange([byte[]](New-LenField 1 ([Text.Encoding]::UTF8.GetBytes($id))))
    $body.AddRange([byte[]](New-LenField 2 ([Text.Encoding]::UTF8.GetBytes($method))))
    $tp = New-LenField 1 ([Text.Encoding]::UTF8.GetBytes($text))
    $body.AddRange([byte[]](New-LenField 3 $tp))
    # field 4 varint, tag = (4<<3)|0 = 0x20, value = 1
    $body.Add(0x20); $body.Add(0x01)
    $frame = New-Object System.Collections.Generic.List[byte]
    $frame.AddRange([byte[]]([BitConverter]::GetBytes([uint32]$body.Count)))
    $frame.AddRange([byte[]]$body.ToArray())
    return $frame.ToArray()
}

function Read-Frame($stream) {
    $lenBuf = New-Object byte[] 4
    $read = 0
    while ($read -lt 4) {
        $n = $stream.Read($lenBuf, $read, 4 - $read)
        if ($n -eq 0) { return $null }
        $read += $n
    }
    $len = [BitConverter]::ToUInt32($lenBuf, 0)
    $buf = New-Object byte[] $len
    $read = 0
    while ($read -lt $len) { $read += $stream.Read($buf, $read, $len - $read) }
    return $buf
}

# Minimal protobuf reader for the fields we care about (response field3
# message, field4 payload -> TextPayload field1).
function Get-LenField([byte[]]$buf, [int]$target) {
    $pos = 0
    while ($pos -lt $buf.Length) {
        [ulong]$tag = 0; $shift = 0
        do { $b = $buf[$pos++]; $tag = $tag -bor ([ulong]($b -band 0x7F) -shl $shift); $shift += 7 } while ($b -band 0x80)
        $field = [int]($tag -shr 3); $wire = $tag -band 7
        if ($wire -eq 2) {
            [ulong]$l = 0; $shift = 0
            do { $b = $buf[$pos++]; $l = $l -bor ([ulong]($b -band 0x7F) -shl $shift); $shift += 7 } while ($b -band 0x80)
            if ($field -eq $target) {
                $out = New-Object byte[] $l
                [Array]::Copy($buf, $pos, $out, 0, $l)
                return $out
            }
            $pos += [int]$l
        } elseif ($wire -eq 0) {
            do { $b = $buf[$pos++] } while ($b -band 0x80)
        } else { break }
    }
    return $null
}

$pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", $PipeName,
    [System.IO.Pipes.PipeDirection]::InOut, [System.IO.Pipes.PipeOptions]::None)
$pipe.Connect(5000)

$seq = 0
function Invoke-Ipc([string]$method, [string]$json) {
    $script:seq++
    $id = "seed$script:seq"
    $frame = Build-Frame $id $method $json
    $pipe.Write($frame, 0, $frame.Length)
    $resp = Read-Frame $pipe
    $msgBytes = Get-LenField $resp 3
    $payBytes = Get-LenField $resp 4
    $inner = if ($payBytes) { Get-LenField $payBytes 1 } else { $null }
    $msg = if ($msgBytes) { [Text.Encoding]::UTF8.GetString($msgBytes) } else { "" }
    $text = if ($inner) { [Text.Encoding]::UTF8.GetString($inner) } else { "" }
    Write-Output "$method -> code-frame msg='$msg' text=$text"
}

$node1 = @'
{"name":"Tokyo-Shadowsocks","protocol":"shadowsocks","endpoint":{"host":"tk1.example.com","port":8388},"authentication":{"password":"demo"},"enabled":true}
'@
$node2 = @'
{"name":"Seattle-VMess","protocol":"vmess","endpoint":{"host":"sea2.example.com","port":443},"tls":{"enabled":true,"server_name":"sea2.example.com"},"enabled":true}
'@
Invoke-Ipc "node.add" $node1
Invoke-Ipc "node.add" $node2
$rule = @'
{"kind":"domain_suffix","value":"example.org","action":"DIRECT","enabled":true,"priority":100}
'@
Invoke-Ipc "routing.addRule" $rule
Invoke-Ipc "node.list" "{}"
Invoke-Ipc "routing.listRules" "{}"

$pipe.Dispose()
