using StreamJsonRpc;

namespace Niello.Host.Rpc;

/// <summary>Error codes and error responses of the host contract (protocol/schemas/host-rpc.md, "Error codes").</summary>
public static class HostErrors
{
    public const int InvalidParams = -32602;

    /// <summary>LSP ServerNotInitialized: <c>niello/host/initialize</c> has not been received.</summary>
    public const int ServerNotInitialized = -32002;

    /// <summary>LSP RequestCancelled.</summary>
    public const int RequestCancelled = -32800;

    /// <summary>LSP ContentModified: stale <c>nielloGeneration</c>.</summary>
    public const int ContentModified = -32801;

    /// <summary>LSP RequestFailed: the language server is unavailable.</summary>
    public const int RequestFailed = -32803;

    public static LocalRpcException NotInitialized() =>
        new("niello/host/initialize has not been received") { ErrorCode = ServerNotInitialized };

    public static LocalRpcException BadParams(string message) => new(message) { ErrorCode = InvalidParams };

    public static LocalRpcException Stale(long requested, long current) =>
        new($"solution generation {requested} is stale (current {current})")
        {
            ErrorCode = ContentModified,
            ErrorData = new { requestedGeneration = requested, currentGeneration = current },
        };

    public static LocalRpcException LanguageServerUnavailable(string why) =>
        new($"language server unavailable: {why}")
        {
            ErrorCode = RequestFailed,
            ErrorData = new { reason = "languageServerUnavailable" },
        };
}
