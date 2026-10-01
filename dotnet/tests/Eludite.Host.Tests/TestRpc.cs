using System.Text.Json;
using Niello.Host.Rpc;
using StreamJsonRpc;
using StreamJsonRpc.Protocol;

namespace Niello.Host.Tests;

/// <summary>Test-side JSON-RPC client helpers.</summary>
internal static class TestRpc
{
    /// <summary>A client connection whose remote errors keep <c>error.data</c> as raw JSON.</summary>
    public static JsonRpc Create(Stream stream) => Create(stream, stream);

    /// <summary>As <see cref="Create(Stream)"/>, over separate streams (a child process's stdin and stdout).</summary>
    public static JsonRpc Create(Stream sendingStream, Stream receivingStream)
    {
        var formatter = new SystemTextJsonFormatter { JsonSerializerOptions = HostServer.SerializerOptions };
        return new RawErrorDataJsonRpc(new HeaderDelimitedMessageHandler(sendingStream, receivingStream, formatter));
    }

    /// <summary>Registers a single-object notification handler (call before StartListening).</summary>
    public static void On(JsonRpc rpc, string method, Action<JsonElement> handler) =>
        rpc.AddLocalRpcMethod(handler.Method, handler.Target, new JsonRpcMethodAttribute(method) { UseSingleObjectParameterDeserialization = true });

    /// <summary>Awaits a call expected to fail and returns its <c>error.code</c> and <c>error.data</c>.</summary>
    /// <remarks>StreamJsonRpc surfaces -32601 and -32602 as RemoteMethodNotFoundException, others as RemoteInvocationException.</remarks>
    public static async Task<(int Code, JsonElement? Data)> ErrorOfAsync(Func<Task> call)
    {
        var ex = await Assert.ThrowsAnyAsync<RemoteRpcException>(call);
        return ((int)(ex.ErrorCode ?? 0), ex.ErrorData is JsonElement e ? e : null);
    }

    private sealed class RawErrorDataJsonRpc(IJsonRpcMessageHandler handler) : JsonRpc(handler)
    {
        protected override Type? GetErrorDetailsDataType(JsonRpcError error) => typeof(JsonElement);
    }
}
