using System.Text;
using System.Text.Json;
using System.Threading.Channels;

namespace Eludite.TestBridge;

/// <summary>
/// vstest.console's design-mode connection (the translation-layer protocol): JSON messages
/// <c>{ MessageType, Version, Payload }</c>, each a string in BinaryWriter's format (a 7-bit-encoded byte length, then
/// UTF-8), over the TCP connection vstest.console opens to its client.
/// </summary>
internal sealed class VsTestConnection : IAsyncDisposable
{
    /// <summary>The protocol version Eludite asks for (vstest.console 17 and 18 answer 7).</summary>
    public const int ProtocolVersion = 7;

    /// <summary>
    /// Indented: vstest.console 18.6 does not find a run's sources in its TestCases when the JSON has no space after
    /// the colons (the run aborts with ArgumentNullException in TestRequestManager.RunTests); indented JSON has them.
    /// </summary>
    private static readonly JsonSerializerOptions Options = new() { PropertyNamingPolicy = null, WriteIndented = true };

    private readonly Stream _stream;
    private readonly BufferedStream _input;
    private readonly SemaphoreSlim _write = new(1, 1);
    private readonly Channel<(string Type, JsonElement Payload)> _messages = Channel.CreateUnbounded<(string, JsonElement)>();
    private readonly Task _reader;

    public VsTestConnection(Stream stream)
    {
        _stream = stream;
        _input = new BufferedStream(stream, 64 * 1024);
        _reader = Task.Run(ReadLoopAsync);
    }

    public ChannelReader<(string Type, JsonElement Payload)> Messages => _messages.Reader;

    public async Task SendAsync(string messageType, object? payload, bool versioned = true)
    {
        var message = new Dictionary<string, object?> { ["MessageType"] = messageType };
        if (versioned)
        {
            message["Version"] = ProtocolVersion;
        }

        message["Payload"] = payload;
        var body = JsonSerializer.SerializeToUtf8Bytes(message, Options);
        var prefix = new List<byte>(5);
        var n = (uint)body.Length;
        while (n >= 0x80)
        {
            prefix.Add((byte)(n | 0x80));
            n >>= 7;
        }

        prefix.Add((byte)n);
        await _write.WaitAsync().ConfigureAwait(false);
        try
        {
            await _stream.WriteAsync(prefix.ToArray()).ConfigureAwait(false);
            await _stream.WriteAsync(body).ConfigureAwait(false);
            await _stream.FlushAsync().ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is IOException or ObjectDisposedException)
        {
            throw new TestRunnerException("vstest.console closed the connection", ex);
        }
        finally
        {
            _write.Release();
        }
    }

    /// <summary>The next message, failing when vstest.console closes the connection.</summary>
    public async Task<(string Type, JsonElement Payload)> ReceiveAsync(CancellationToken cancellationToken)
    {
        try
        {
            return await _messages.Reader.ReadAsync(cancellationToken).ConfigureAwait(false);
        }
        catch (ChannelClosedException ex)
        {
            throw new TestRunnerException("vstest.console closed the connection", ex);
        }
    }

    private async Task ReadLoopAsync()
    {
        try
        {
            var one = new byte[1];
            while (true)
            {
                var length = 0;
                var shift = 0;
                while (true)
                {
                    if (await _input.ReadAsync(one).ConfigureAwait(false) == 0)
                    {
                        return;
                    }

                    length |= (one[0] & 0x7f) << shift;
                    shift += 7;
                    if ((one[0] & 0x80) == 0)
                    {
                        break;
                    }
                }

                var body = new byte[length];
                await _input.ReadExactlyAsync(body).ConfigureAwait(false);
                using var doc = JsonDocument.Parse(Encoding.UTF8.GetString(body));
                var root = doc.RootElement;
                var type = root.TryGetProperty("MessageType", out var t) ? t.GetString() ?? string.Empty : string.Empty;
                var payload = root.TryGetProperty("Payload", out var p) ? p.Clone() : default;
                await _messages.Writer.WriteAsync((type, payload)).ConfigureAwait(false);
            }
        }
        catch (Exception ex) when (ex is IOException or ObjectDisposedException or JsonException or EndOfStreamException)
        {
        }
        finally
        {
            _messages.Writer.TryComplete();
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
