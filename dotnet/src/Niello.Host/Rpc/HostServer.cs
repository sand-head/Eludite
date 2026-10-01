using System.Text.Json;
using StreamJsonRpc;

namespace Niello.Host.Rpc;

/// <summary>Wires a <see cref="HostRpcTarget"/> to a pair of streams using LSP-style Content-Length framing.</summary>
public static class HostServer
{
    public static JsonSerializerOptions SerializerOptions { get; } = new(JsonSerializerDefaults.Web);

    /// <summary>Creates the JSON-RPC connection used by both the host and its clients.</summary>
    public static JsonRpc CreateConnection(Stream sendingStream, Stream receivingStream)
    {
        var formatter = new SystemTextJsonFormatter { JsonSerializerOptions = SerializerOptions };
        var handler = new HeaderDelimitedMessageHandler(sendingStream, receivingStream, formatter);
        return new JsonRpc(handler);
    }

    /// <summary>
    /// Serves <paramref name="target"/> until the client sends <c>exit</c> or disconnects.
    /// Returns the process exit code: 0 after a clean shutdown/exit sequence, 1 otherwise.
    /// </summary>
    public static async Task<int> RunAsync(Stream output, Stream input, HostRpcTarget target)
    {
        ArgumentNullException.ThrowIfNull(target);

        using var rpc = CreateConnection(output, input);
        rpc.AddLocalRpcTarget(target);
        rpc.StartListening();

        await Task.WhenAny(target.ExitRequested, rpc.Completion).ConfigureAwait(false);
        return target.ShutdownRequested && target.ExitRequested.IsCompleted ? 0 : 1;
    }
}
