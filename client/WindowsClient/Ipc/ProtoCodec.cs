using System.Buffers.Binary;
using System.Text;

namespace WindowsClient.Ipc;

// Minimal protobuf wire codec for the handful of IPC messages declared in
// shared/proto/network.proto. The contract intentionally carries complex
// domain objects as JSON inside TextPayload, so this codec stays tiny.

internal static class ProtoWriter
{
    public static byte[] BuildRequest(string requestId, string method, byte[] payload, uint protocolVersion)
    {
        using var ms = new MemoryStream();
        WriteString(ms, 1, requestId);
        WriteString(ms, 2, method);
        WriteBytes(ms, 3, payload);
        WriteVarint(ms, Tag(4, WireType.Varint), protocolVersion);
        return ms.ToArray();
    }

    public static byte[] Text(string text)
    {
        using var ms = new MemoryStream();
        WriteString(ms, 1, text);
        return ms.ToArray();
    }

    public static byte[] ConnectRequest(string nodeId, string mode)
    {
        using var ms = new MemoryStream();
        WriteString(ms, 1, nodeId);
        WriteString(ms, 2, mode);
        return ms.ToArray();
    }

    private enum WireType : uint
    {
        Varint = 0,
        Fixed64 = 1,
        LengthDelimited = 2,
        Fixed32 = 5,
    }

    private static ulong Tag(int field, WireType wire) => ((ulong)field << 3) | (ulong)wire;

    public static void WriteVarint(Stream s, ulong value)
    {
        while (value >= 0x80)
        {
            s.WriteByte((byte)(value | 0x80));
            value >>= 7;
        }
        s.WriteByte((byte)value);
    }

    private static void WriteVarint(Stream s, ulong tag, uint value)
    {
        WriteVarint(s, tag);
        WriteVarint(s, value);
    }

    public static void WriteString(Stream s, int field, string value)
    {
        if (string.IsNullOrEmpty(value)) return;
        var bytes = Encoding.UTF8.GetBytes(value);
        WriteBytes(s, field, bytes);
    }

    public static void WriteBytes(Stream s, int field, ReadOnlySpan<byte> bytes)
    {
        WriteVarint(s, Tag(field, WireType.LengthDelimited));
        WriteVarint(s, (ulong)bytes.Length);
        s.Write(bytes);
    }
}

internal sealed class ProtoReader
{
    private readonly byte[] _buf;
    private int _pos;

    private ProtoReader(byte[] buf) { _buf = buf; }

    public static ProtoReader From(byte[] buf) => new(buf);

    public string? ReadStringField(int target)
    {
        var b = ReadBytesField(target);
        return b is null ? null : Encoding.UTF8.GetString(b);
    }

    public byte[]? ReadBytesField(int target)
    {
        Reset();
        while (TryReadField(out var field, out var wire))
        {
            if (field == target && wire == 2)
                return ReadLengthDelimited();
            Skip(wire);
        }
        return null;
    }

    public long ReadInt64Field(int target)
    {
        Reset();
        while (TryReadField(out var field, out var wire))
        {
            if (field == target && wire == 0)
                return (long)ReadVarint();
            Skip(wire);
        }
        return 0;
    }

    public int ReadInt32Field(int target)
    {
        Reset();
        while (TryReadField(out var field, out var wire))
        {
            if (field == target && wire == 0)
                return (int)ReadVarint();
            Skip(wire);
        }
        return 0;
    }

    public ulong ReadUInt64Field(int target)
    {
        Reset();
        while (TryReadField(out var field, out var wire))
        {
            if (field == target && wire == 0)
                return ReadVarint();
            Skip(wire);
        }
        return 0;
    }

    public double ReadDoubleField(int target)
    {
        Reset();
        while (TryReadField(out var field, out var wire))
        {
            if (field == target && wire == 1)
            {
                var v = BinaryPrimitives.ReadUInt64LittleEndian(_buf.AsSpan(_pos));
                _pos += 8;
                return BitConverter.Int64BitsToDouble((long)v);
            }
            Skip(wire);
        }
        return 0;
    }

    private void Reset() => _pos = 0;

    private bool TryReadField(out int field, out uint wire)
    {
        field = 0; wire = 0;
        if (_pos >= _buf.Length) return false;
        var tag = ReadVarint();
        field = (int)(tag >> 3);
        wire = (uint)(tag & 0x7);
        return field > 0;
    }

    private ulong ReadVarint()
    {
        ulong result = 0;
        int shift = 0;
        while (true)
        {
            var b = _buf[_pos++];
            result |= (ulong)(b & 0x7F) << shift;
            if ((b & 0x80) == 0) break;
            shift += 7;
        }
        return result;
    }

    private byte[] ReadLengthDelimited()
    {
        var len = (int)ReadVarint();
        var bytes = new byte[len];
        Array.Copy(_buf, _pos, bytes, 0, len);
        _pos += len;
        return bytes;
    }

    private void Skip(uint wire)
    {
        switch (wire)
        {
            case 0: ReadVarint(); break;
            case 1: _pos += 8; break;
            // NOTE: do not write `_pos += (int)ReadVarint()` — in a compound
            // assignment the LHS (_pos) is evaluated before ReadVarint() runs,
            // so its internal advance is discarded and field skipping desyncs.
            case 2:
                var len = (int)ReadVarint();
                _pos += len;
                break;
            case 5: _pos += 4; break;
            default: _pos = _buf.Length; break;
        }
    }
}
