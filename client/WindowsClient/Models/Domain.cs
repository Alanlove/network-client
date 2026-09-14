using System.Text.Json.Serialization;

namespace WindowsClient.Models;

public sealed record Endpoint
{
    public string Host { get; init; } = "";
    public ushort Port { get; init; }
}

public sealed record Node
{
    public string Id { get; init; } = "";
    public string Name { get; init; } = "";
    public string Protocol { get; init; } = "";
    public Endpoint Endpoint { get; init; } = new();
    [JsonPropertyName("subscription_id")]
    public string SubscriptionId { get; init; } = "";
    public bool Enabled { get; init; } = true;
    [JsonPropertyName("created_at")]
    public long CreatedAt { get; init; }

    public string Display => $"{Name}  ·  {Protocol} {Endpoint.Host}:{Endpoint.Port}";
}

public sealed record SubscriptionView
{
    public string Id { get; init; } = "";
    public string Name { get; init; } = "";
    public string Url { get; init; } = "";
    public bool Enabled { get; init; } = true;
    [JsonPropertyName("node_count")]
    public int NodeCount { get; init; }
    [JsonPropertyName("last_refresh")]
    public long LastRefresh { get; init; }
}

public sealed record SubscriptionInput
{
    public string Name { get; init; } = "";
    public string Url { get; init; } = "";
    public bool Enabled { get; init; } = true;
    [JsonPropertyName("interval_secs")]
    public long IntervalSecs { get; init; } = 86400;
}

public sealed record Rule
{
    public string Id { get; init; } = "";
    public string Kind { get; init; } = "domain_suffix";
    public string Value { get; init; } = "";
    public string Action { get; init; } = "DIRECT";
    public bool Enabled { get; init; } = true;
    public int Priority { get; init; }
}

public sealed record RoutingConfig
{
    public string Mode { get; init; } = "smart";
}

public sealed record NodeRuntimeStats
{
    [JsonPropertyName("latency_ms")]
    public long LatencyMs { get; init; }
    [JsonPropertyName("packet_loss")]
    public double PacketLoss { get; init; }
    public double Score { get; init; }
    public double Availability { get; init; }
}
