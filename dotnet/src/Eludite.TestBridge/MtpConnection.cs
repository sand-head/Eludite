using System.Collections.Concurrent;
using System.Globalization;
using System.Text;
using System.Text.Json;

namespace Eludite.TestBridge;

/// <summary>
/// JSON-RPC 2.0 with <c>Content-Length</c> framing over the TCP connection a Microsoft.Testing.Platform test application
/// opens to its client. Notifications are handed to the callback in arrival order, on the reader's thread, before the
/// response that follows them completes; requests from the application are answered MethodNotFound.
/// </summary>
internal sealed class MtpConnection : IAsyncDisposable
{
    private static readonly JsonSerializerOptions Options = new(JsonSerializerDefaults.Web);

    private readonly Stream _stream;
    private readonly BufferedStream _input;
    private readonly Action<string, JsonElement> _onNotification;
    private readonly SemaphoreSlim _write = new(1, 1);
    private readonly ConcurrentDictionary<long, TaskCompletionSource<JsonElement>> _pending = new();
    private readonly Task _reader;
    private long _nextId;

    public MtpConnection(Stream stream, Action<string, JsonElement> onNotification)
    {
        _stream = stream;
        _input = new BufferedStream(stream, 64 * 1024);
        _onNotification = onNotification;
        _reader = Task.Run(ReadLoopAsync);
    }

    /// <summary>Completes when the application closes the connection (or it breaks).</summary>
    public Task Completion => _reader;

    /// <summary>Sends a request; the task completes with its result, or throws <see cref="TestRunnerException"/> with its error.</summary>
    public async Task<(long Id, Task<JsonElement> Response)> RequestAsync(string method, object? parameters)
    {
        var id = Interlocked.Increment(ref _nextId);
        var tcs = new TaskCompletionSource<JsonElement>(TaskCreationOptions.RunContinuationsAsynchronously);
        _pending[id] = tcs;
        await WriteAsync(new { jsonrpc = "2.0", id, method, @params = parameters }).ConfigureAwait(false);
        return (id, tcs.Task);
    }

    public Task NotifyAsync(string method, object? parameters) =>
        WriteAsync(new { jsonrpc = "2.0", method, @params = parameters });

    private async Task WriteAsync(object message)
    {
        var body = JsonSerializer.SerializeToUtf8Bytes(message, Options);
        var header = Encoding.ASCII.GetBytes(string.Create(CultureInfo.InvariantCulture, $"Content-Length: {body.Length}\r\n\r\n"));
        await _write.WaitAsync().ConfigureAwait(false);
        try
        {
            await _stream.WriteAsync(header).ConfigureAwait(false);
            await _stream.WriteAsync(body).ConfigureAwait(false);
            await _stream.FlushAsync().ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is IOException or ObjectDisposedException)
        {
            throw new TestRunnerException("the test application closed the connection", ex);
        }
        finally
        {
            _write.Release();
        }
    }

    private async Task ReadLoopAsync()
    {
        try
        {
            while (await ReadMessageAsync().ConfigureAwait(false) is { } body)
            {
                using var doc = JsonDocument.Parse(body);
                var root = doc.RootElement;
                var hasMethod = root.TryGetProperty("method", out var method);
                var hasId = root.TryGetProperty("id", out var id);
                if (hasMethod && hasId)
                {
                    var answer = new { jsonrpc = "2.0", id = id.Clone(), error = new { code = -32601, message = $"{method.GetString()} is not supported by Eludite" } };
                    _ = WriteAsync(answer).ContinueWith(t => _ = t.Exception, TaskScheduler.Default);
                }
                else if (hasMethod)
                {
                    var parameters = root.TryGetProperty("params", out var p) ? p.Clone() : default;
                    _onNotification(method.GetString() ?? string.Empty, parameters);
                }
                else if (hasId && id.TryGetInt64(out var n) && _pending.TryRemove(n, out var tcs))
                {
                    if (root.TryGetProperty("error", out var error))
                    {
                        var message = error.TryGetProperty("message", out var m) ? m.GetString() : error.ToString();
                        tcs.TrySetException(new TestRunnerException(message ?? "error"));
                    }
                    else
                    {
                        tcs.TrySetResult(root.TryGetProperty("result", out var r) ? r.Clone() : default);
                    }
                }
            }
        }
        catch (Exception ex) when (ex is IOException or ObjectDisposedException or JsonException or InvalidDataException)
        {
        }
        finally
        {
            foreach (var (_, tcs) in _pending)
            {
                tcs.TrySetException(new TestRunnerException("the test application closed the connection"));
            }
        }
    }

    private async Task<byte[]?> ReadMessageAsync()
    {
        var length = -1;
        while (true)
        {
            var line = await ReadLineAsync().ConfigureAwait(false);
            if (line is null)
            {
                return null;
            }

            if (line.Length == 0)
            {
                break;
            }

            var colon = line.IndexOf(':', StringComparison.Ordinal);
            if (colon > 0 && line[..colon].Trim().Equals("Content-Length", StringComparison.OrdinalIgnoreCase))
            {
                length = int.Parse(line[(colon + 1)..].Trim(), CultureInfo.InvariantCulture);
            }
        }

        if (length < 0)
        {
            throw new InvalidDataException("a message without Content-Length");
        }

        var body = new byte[length];
        await _input.ReadExactlyAsync(body).ConfigureAwait(false);
        return body;
    }

    private async Task<string?> ReadLineAsync()
    {
        var bytes = new List<byte>(64);
        var one = new byte[1];
        while (true)
        {
            var read = await _input.ReadAsync(one).ConfigureAwait(false);
            if (read == 0)
            {
                return null;
            }

            if (one[0] == (byte)'\n')
            {
                if (bytes.Count > 0 && bytes[^1] == (byte)'\r')
                {
                    bytes.RemoveAt(bytes.Count - 1);
                }

                return Encoding.ASCII.GetString(bytes.ToArray());
            }

            bytes.Add(one[0]);
        }
    }

    public async ValueTask DisposeAsync()
    {
        await _stream.DisposeAsync().ConfigureAwait(false);
        try
        {
            await _reader.ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is IOException or ObjectDisposedException)
        {
        }

        _write.Dispose();
    }
}
