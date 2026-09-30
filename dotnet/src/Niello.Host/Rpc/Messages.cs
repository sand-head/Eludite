namespace Niello.Host.Rpc;

// Wire shapes for the niello-host JSON-RPC contract shared with the Rust shell.
// Property names are serialized camelCase; keep them in sync with protocol/.

public sealed record InitializeParams(string ClientName, string ClientVersion, string? SolutionPath = null);

public sealed record InitializeResult(string HostName, string HostVersion, HostCapabilities Capabilities);

/// <summary>Capabilities advertised by the host. Empty until features land.</summary>
public sealed record HostCapabilities;

public sealed record PingResult(bool Pong, string Timestamp);

public sealed record HostInfoResult(IReadOnlyList<SdkInfo> DotnetSdks, string Runtime, string Os);

public sealed record SdkInfo(string Version, string Path);
