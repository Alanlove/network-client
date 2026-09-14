namespace WindowsClient.Ipc;

public sealed record IpcResponse(int Code, string Message, byte[] Payload)
{
    public bool Ok => Code == 0;
    public string? Text => Payload.Length == 0 ? null : ProtoReader.From(Payload).ReadStringField(1);
}

public sealed record ConnectionStateInfo(
    string State,
    string NodeId,
    string NodeName,
    string Mode,
    long LatencyMs,
    string ErrorCode,
    string ErrorMessage,
    long Timestamp)
{
    public static readonly ConnectionStateInfo Stopped =
        new("STOPPED", "", "", "", 0, "", "", 0);

    public static ConnectionStateInfo Decode(byte[] payload)
    {
        var r = ProtoReader.From(payload);
        return new ConnectionStateInfo(
            r.ReadStringField(1) ?? "STOPPED",
            r.ReadStringField(2) ?? "",
            r.ReadStringField(3) ?? "",
            r.ReadStringField(4) ?? "",
            r.ReadInt64Field(5),
            r.ReadStringField(6) ?? "",
            r.ReadStringField(7) ?? "",
            r.ReadInt64Field(8));
    }
}

public sealed record TrafficInfo(
    ulong UpRate, ulong DownRate,
    ulong UpTotal, ulong DownTotal,
    ulong UpToday, ulong DownToday)
{
    public static TrafficInfo Decode(byte[] payload)
    {
        var r = ProtoReader.From(payload);
        return new TrafficInfo(
            r.ReadUInt64Field(1), r.ReadUInt64Field(2),
            r.ReadUInt64Field(3), r.ReadUInt64Field(4),
            r.ReadUInt64Field(5), r.ReadUInt64Field(6));
    }
}

public sealed record VersionInfo(uint ProtocolVersion, string ServiceVersion, string CoreVersion)
{
    public static VersionInfo Decode(byte[] payload)
    {
        var r = ProtoReader.From(payload);
        return new VersionInfo(
            (uint)r.ReadInt32Field(1),
            r.ReadStringField(2) ?? "",
            r.ReadStringField(3) ?? "");
    }
}

public sealed record DiagnosticProgress(
    string Step, uint Index, uint Total, string Status, string Detail)
{
    public static DiagnosticProgress Decode(byte[] payload)
    {
        var r = ProtoReader.From(payload);
        return new DiagnosticProgress(
            r.ReadStringField(1) ?? "",
            (uint)r.ReadInt32Field(2),
            (uint)r.ReadInt32Field(3),
            r.ReadStringField(4) ?? "",
            r.ReadStringField(5) ?? "");
    }
}
