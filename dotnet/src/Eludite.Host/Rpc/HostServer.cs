using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.VisualStudio.Threading;
using StreamJsonRpc;

namespace Eludite.Host.Rpc;

/// <summary>Wires a <see cref="HostRpcTarget"/> to a pair of streams using LSP-style Content-Length framing.</summary>
public static class HostServer
{
    /// <summary>camelCase, null members omitted (protocol/schemas/host-rpc.md).</summary>
    public static JsonSerializerOptions SerializerOptions { get; } = new(JsonSerializerDefaults.Web)
    {
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
    };

    /// <summary>Creates the JSON-RPC connection used by both the host and its clients.</summary>
    public static JsonRpc CreateConnection(Stream sendingStream, Stream receivingStream)
    {
        var formatter = new SystemTextJsonFormatter { JsonSerializerOptions = SerializerOptions };
        var handler = new HeaderDelimitedMessageHandler(sendingStream, receivingStream, formatter);
        return new JsonRpc(handler);
    }

    /// <summary>
    /// Serves <paramref name="target"/> until the client sends <c>eludite/host/exit</c> or disconnects.
    /// Returns the process exit code: 0 after a clean shutdown/exit sequence, 1 otherwise.
    /// </summary>
    public static async Task<int> RunAsync(Stream output, Stream input, HostRpcTarget target)
    {
        ArgumentNullException.ThrowIfNull(target);

        using var rpc = CreateConnection(output, input);
        // Handlers start one at a time in arrival order, so didOpen/didChange/completion keep the shell's order.
        // Every handler leaves this context at its first await (ConfigureAwait(false)), so they still run concurrently.
        rpc.SynchronizationContext = new NonConcurrentSynchronizationContext(sticky: false);
        rpc.AddLocalRpcTarget(target);
        target.LanguageServer.Attach(rpc);
        target.Build.Attach(rpc);
        rpc.StartListening();

        await Task.WhenAny(target.ExitRequested, rpc.Completion).ConfigureAwait(false);
        await target.Build.DisposeAsync().ConfigureAwait(false);
        await target.LanguageServer.DisposeAsync().ConfigureAwait(false);

        return target.ShutdownRequested && target.ExitRequested.IsCompleted ? 0 : 1;
    }
}
