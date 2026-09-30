using System.Collections.Concurrent;
using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace Bench.Driver;

/// <summary>Minimal Content-Length framed JSON-RPC client over a child process's stdio.</summary>
internal sealed class LspClient : IAsyncDisposable
{
    private readonly Stream _out;
    private readonly Stream _in;
    private readonly SemaphoreSlim _writeLock = new(1, 1);
    private readonly ConcurrentDictionary<long, TaskCompletionSource<JsonObject>> _pending = new();
    private readonly ConcurrentDictionary<string, TaskCompletionSource<JsonNode?>> _notifications = new();
    private readonly Task _reader;
    private long _nextId;

    public LspClient(Stream toServer, Stream fromServer)
    {
        _out = toServer;
        _in = fromServer;
        _reader = Task.Run(ReadLoopAsync);
    }

    /// <summary>Responses whose id was never pending or arrived after the caller stopped waiting.</summary>
    public ConcurrentBag<JsonObject> Unexpected { get; } = new();

    public Task<JsonNode?> WaitForNotification(string method) =>
        _notifications.GetOrAdd(method, _ => new(TaskCreationOptions.RunContinuationsAsynchronously)).Task;

    public (long Id, Task<JsonObject> Response) Send(string method, JsonNode? parameters)
    {
        var id = Interlocked.Increment(ref _nextId);
        var tcs = new TaskCompletionSource<JsonObject>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[id] = tcs;
        var msg = new JsonObject { ["jsonrpc"] = "2.0", ["id"] = id, ["method"] = method, ["params"] = parameters };
        return (id, WriteAsync(msg).ContinueWith(_ => tcs.Task).Unwrap());
    }

    public async Task<JsonObject> RequestAsync(string method, JsonNode? parameters) => await Send(method, parameters).Response;

    public Task NotifyAsync(string method, JsonNode? parameters) =>
        WriteAsync(new JsonObject { ["jsonrpc"] = "2.0", ["method"] = method, ["params"] = parameters });

    private async Task WriteAsync(JsonObject message)
    {
        var body = Encoding.UTF8.GetBytes(message.ToJsonString());
        var header = Encoding.ASCII.GetBytes($"Content-Length: {body.Length}\r\n\r\n");
        await _writeLock.WaitAsync();
        try
        {
            await _out.WriteAsync(header);
            await _out.WriteAsync(body);
            await _out.FlushAsync();
        }
        finally
        {
            _writeLock.Release();
        }
    }

    private async Task ReadLoopAsync()
    {
        var buffer = new byte[64 * 1024];
        var data = new MemoryStream();
        try
        {
            while (true)
            {
                var n = await _in.ReadAsync(buffer);
                if (n == 0)
                {
                    break;
                }

                data.Write(buffer, 0, n);
                while (TryTakeMessage(data, out var message))
                {
                    Dispatch(message);
                }
            }
        }
        catch (IOException)
        {
        }
        catch (ObjectDisposedException)
        {
        }

        foreach (var p in _pending.Values)
        {
            p.TrySetException(new IOException("connection closed"));
        }
    }

    private static bool TryTakeMessage(MemoryStream data, out JsonObject message)
    {
        message = null!;
        var bytes = data.GetBuffer().AsSpan(0, (int)data.Length);
        var headerEnd = bytes.IndexOf("\r\n\r\n"u8);
        if (headerEnd < 0)
        {
            return false;
        }

        var header = Encoding.ASCII.GetString(bytes[..headerEnd]);
        var length = int.Parse(header.Split("\r\n").First(l => l.StartsWith("Content-Length:", StringComparison.OrdinalIgnoreCase))["Content-Length:".Length..].Trim());
        var total = headerEnd + 4 + length;
        if (bytes.Length < total)
        {
            return false;
        }

        message = JsonNode.Parse(bytes.Slice(headerEnd + 4, length))!.AsObject();
        var rest = bytes[total..].ToArray();
        data.SetLength(0);
        data.Write(rest);
        return true;
    }

    private void Dispatch(JsonObject message)
    {
        if (message.TryGetPropertyValue("id", out var idNode) && idNode is not null && !message.ContainsKey("method"))
        {
            var id = idNode.GetValue<long>();
            if (_pending.TryRemove(id, out var tcs))
            {
                tcs.TrySetResult(message);
            }
            else
            {
                Unexpected.Add(message);
            }

            return;
        }

        if (message["method"]?.GetValue<string>() is { } method && !message.ContainsKey("id"))
        {
            _notifications.GetOrAdd(method, _ => new(TaskCreationOptions.RunContinuationsAsynchronously)).TrySetResult(message["params"]?.DeepClone());
        }
    }

    /// <summary>Stops tracking a request (the caller gave up on it); a late response lands in <see cref="Unexpected"/>.</summary>
    public void Forget(long id) => _pending.TryRemove(id, out _);

    public async ValueTask DisposeAsync()
    {
        try
        {
            await _reader.WaitAsync(TimeSpan.FromSeconds(2));
        }
        catch (TimeoutException)
        {
        }
    }

    public static double Ms(long ticks) => ticks * 1000.0 / Stopwatch.Frequency;
}
