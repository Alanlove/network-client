using System.IO.Pipes;
using System.Text;

namespace WindowsClient.Ipc;

/// Event pushed by the service (Event envelope).
public sealed record ServiceEvent(string EventType, byte[] Payload, long Timestamp)
{
    public string? Text => Payload.Length == 0 ? null : ProtoReader.From(Payload).ReadStringField(1);
}

/// <summary>
/// Single persistent Named Pipe connection to network-service.
/// Frame layout: u32-LE length + one protobuf Request | Response | Event.
/// </summary>
public sealed class NetworkServiceClient : IDisposable
{
    public const string PipeName = "NetworkClient";
    private const uint ProtocolVersion = 1;
    private const int MaxFrame = 16 * 1024 * 1024;

    private NamedPipeClientStream? _pipe;
    private readonly SemaphoreSlim _writeLock = new(1, 1);
    private readonly Dictionary<string, TaskCompletionSource<IpcResponse>> _pending = new();
    private readonly object _pendingGate = new();
    private CancellationTokenSource? _receiveCts;
    private int _nextId;

    public bool IsConnected => _pipe?.IsConnected ?? false;

    /// Raised on a background thread for every server-pushed event.
    public event EventHandler<ServiceEvent>? EventReceived;

    /// Raised (background thread) when the pipe drops.
    public event EventHandler? Disconnected;

    public async Task<bool> TryConnectAsync(CancellationToken ct = default)
    {
        var pipe = new NamedPipeClientStream(".", PipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
        try
        {
            await pipe.ConnectAsync(3000, ct).ConfigureAwait(false);
        }
        catch (TimeoutException)
        {
            pipe.Dispose();
            return false;
        }
        catch (IOException)
        {
            pipe.Dispose();
            return false;
        }
        _pipe = pipe;
        _receiveCts = new CancellationTokenSource();
        _ = Task.Run(() => ReceiveLoopAsync(_receiveCts.Token));
        return true;
    }

    /// Connect with retries; returns false if the service never answered.
    public async Task<bool> ConnectWithRetriesAsync(int attempts = 20, CancellationToken ct = default)
    {
        for (var i = 0; i < attempts; i++)
        {
            if (await TryConnectAsync(ct).ConfigureAwait(false))
                return true;
            try { await Task.Delay(500, ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { return false; }
        }
        return false;
    }

    public async Task<IpcResponse> InvokeAsync(string method, byte[]? payload = null, CancellationToken ct = default)
    {
        var pipe = _pipe ?? throw new InvalidOperationException("not connected");
        var id = Interlocked.Increment(ref _nextId).ToString();
        var tcs = new TaskCompletionSource<IpcResponse>(TaskCreationOptions.RunContinuationsAsynchronously);
        lock (_pendingGate) _pending[id] = tcs;

        var frame = ProtoWriter.BuildRequest(id, method, payload ?? Array.Empty<byte>(), ProtocolVersion);
        try
        {
            await _writeLock.WaitAsync(ct).ConfigureAwait(false);
            try
            {
                await WriteFrameAsync(pipe, frame, ct).ConfigureAwait(false);
            }
            finally
            {
                _writeLock.Release();
            }
        }
        catch (Exception ex)
        {
            lock (_pendingGate) _pending.Remove(id);
            throw new IOException($"IPC request '{method}' failed: {ex.Message}", ex);
        }

        var completed = await Task.WhenAny(tcs.Task, Task.Delay(Timeout.Infinite, ct)).ConfigureAwait(false);
        if (completed != tcs.Task)
        {
            lock (_pendingGate) _pending.Remove(id);
            throw new OperationCanceledException(ct);
        }
        return await tcs.Task.ConfigureAwait(false);
    }

    public Task<IpcResponse> InvokeTextAsync(string method, string text, CancellationToken ct = default)
        => InvokeAsync(method, ProtoWriter.Text(text), ct);

    private async Task ReceiveLoopAsync(CancellationToken token)
    {
        try
        {
            while (!token.IsCancellationRequested)
            {
                var frame = await ReadFrameAsync(token).ConfigureAwait(false);
                // Distinguish Response vs Event by the wire type of field 2:
                // Response.code is varint (wire 0); Event.payload is
                // length-delimited (wire 2).
                if (TryDecodeResponse(frame, out var response))
                {
                    var requestId = ProtoReader.From(frame).ReadStringField(1) ?? "";
                    lock (_pendingGate)
                    {
                        if (_pending.Remove(requestId, out var tcs))
                        {
                            tcs!.TrySetResult(response!);
                        }
                    }
                }
                else
                {
                    var ev = new ServiceEvent(
                        ProtoReader.From(frame).ReadStringField(1) ?? "",
                        ProtoReader.From(frame).ReadBytesField(2) ?? Array.Empty<byte>(),
                        ProtoReader.From(frame).ReadInt64Field(3));
                    EventReceived?.Invoke(this, ev);
                }
            }
        }
        catch (OperationCanceledException) { }
        catch { /* pipe dropped */ }
        finally
        {
            Disconnected?.Invoke(this, EventArgs.Empty);
        }
    }

    private static bool TryDecodeResponse(byte[] frame, out IpcResponse? response)
    {
        // proto3 omits fields equal to their default, so a successful Response
        // (code = 0) carries NO field 2 at all — requiring it would misclassify
        // every success frame as an Event and hang the pending TCS forever.
        // Discriminate on fields that are always populated instead:
        //   Response: field 2 code  is varint(wire 0) when non-zero,
        //             field 3 message "ok" is length-delimited(wire 2);
        //   Event:    field 2 payload is wire 2, field 3 timestamp is wire 0.
        var isResponse = false;
        var pos = 0;
        while (pos < frame.Length)
        {
            var (field, wire) = ReadTag(frame, ref pos);
            if (field == 2 && wire == 0) { isResponse = true; break; } // non-zero code
            if (field == 3 && wire == 2) { isResponse = true; break; } // message "ok"/error
            SkipField(frame, ref pos, wire);
        }
        if (!isResponse)
        {
            response = null;
            return false;
        }
        var reader = ProtoReader.From(frame);
        var rCode = reader.ReadInt32Field(2);
        var rMsg = reader.ReadStringField(3) ?? "";
        var rPay = reader.ReadBytesField(4) ?? Array.Empty<byte>();
        response = new IpcResponse(rCode, rMsg, rPay);
        return true;
    }

    private static (int field, uint wire) ReadTag(byte[] buf, ref int pos)
    {
        ulong tag = 0;
        int shift = 0;
        while (true)
        {
            var b = buf[pos++];
            tag |= (ulong)(b & 0x7F) << shift;
            if ((b & 0x80) == 0) break;
            shift += 7;
        }
        return ((int)(tag >> 3), (uint)(tag & 0x7));
    }

    private static void SkipField(byte[] buf, ref int pos, uint wire)
    {
        switch (wire)
        {
            case 0:
                while ((buf[pos++] & 0x80) != 0) { }
                break;
            case 1: pos += 8; break;
            case 2:
            {
                int len = 0, shift = 0;
                while (true)
                {
                    var b = buf[pos++];
                    len |= (b & 0x7F) << shift;
                    if ((b & 0x80) == 0) break;
                    shift += 7;
                }
                pos += len;
                break;
            }
            case 5: pos += 4; break;
        }
    }

    private static async Task WriteFrameAsync(PipeStream pipe, byte[] payload, CancellationToken ct)
    {
        var lenBuf = BitConverter.GetBytes((uint)payload.Length);
        if (!BitConverter.IsLittleEndian) Array.Reverse(lenBuf);
        await pipe.WriteAsync(lenBuf, ct).ConfigureAwait(false);
        await pipe.WriteAsync(payload, ct).ConfigureAwait(false);
        await pipe.FlushAsync(ct).ConfigureAwait(false);
    }

    private async Task<byte[]> ReadFrameAsync(CancellationToken token)
    {
        var pipe = _pipe!;
        var lenBuf = new byte[4];
        await ReadExactAsync(pipe, lenBuf, token).ConfigureAwait(false);
        var len = BitConverter.ToUInt32(lenBuf, 0);
        if (!BitConverter.IsLittleEndian) len = BinaryReverse(len);
        if (len is 0 or > MaxFrame) throw new IOException($"invalid frame length {len}");
        var buf = new byte[len];
        await ReadExactAsync(pipe, buf, token).ConfigureAwait(false);
        return buf;
    }

    private static uint BinaryReverse(uint v) =>
        ((v & 0xFF) << 24) | ((v & 0xFF00) << 8) | ((v >> 8) & 0xFF00) | (v >> 24);

    private static async Task ReadExactAsync(PipeStream pipe, byte[] buf, CancellationToken ct)
    {
        var read = 0;
        while (read < buf.Length)
        {
            var n = await pipe.ReadAsync(buf.AsMemory(read), ct).ConfigureAwait(false);
            if (n == 0) throw new EndOfStreamException("pipe closed");
            read += n;
        }
    }

    public void Dispose()
    {
        _receiveCts?.Cancel();
        _pipe?.Dispose();
        _writeLock.Dispose();
        _receiveCts?.Dispose();
    }
}
