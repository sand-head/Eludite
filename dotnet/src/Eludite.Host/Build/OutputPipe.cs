using System.Diagnostics;
using System.Text;
using System.Threading.Channels;

namespace Eludite.Host.Build;

/// <summary>
/// Turns the build's lines into ordered <c>eludite/build/output</c> chunks (host-rpc.md, "Build").
/// <list type="bullet">
/// <item>A chunk holds whole lines. It is sent when it reaches <see cref="ChunkBytes"/> (16 KiB), or
/// <see cref="FlushDelay"/> (16 ms) after its first line, whichever comes first.</item>
/// <item>One sender awaits each send. While it waits (the shell or the pipe is slow), new lines queue up and the next
/// chunk takes everything queued, up to <see cref="MaxChunkBytes"/>: fewer, larger messages, never dropped lines.</item>
/// <item>The queue holds at most <see cref="QueueLines"/> lines (about 8 MiB of typical MSBuild output). When it is
/// full, <see cref="WriteLineAsync"/> waits, so the reader stops reading MSBuild's output and MSBuild pauses on its
/// own console write instead of the host's memory growing.</item>
/// </list>
/// </summary>
public sealed class OutputPipe
{
    public const int ChunkBytes = 16 * 1024;
    public const int MaxChunkBytes = 1024 * 1024;
    public const int QueueLines = 64 * 1024;
    public static readonly TimeSpan FlushDelay = TimeSpan.FromMilliseconds(16);

    private readonly Channel<string> _lines;
    private readonly Func<long, string, Task> _send;
    private readonly Task _sender;
    private long _sent;

    /// <param name="send">Sends chunk <c>seq</c> (from 0) with its text; awaited before the next chunk is formed.</param>
    public OutputPipe(Func<long, string, Task> send)
    {
        _send = send;
        _lines = Channel.CreateBounded<string>(new BoundedChannelOptions(QueueLines)
        {
            FullMode = BoundedChannelFullMode.Wait,
            SingleReader = true,
        });
        _sender = Task.Run(SendLoopAsync);
    }

    /// <summary>Chunks sent so far.</summary>
    public long ChunksSent => Interlocked.Read(ref _sent);

    /// <summary>Queues one line (without its line break). Waits while the queue is full.</summary>
    public ValueTask WriteLineAsync(string line, CancellationToken cancellationToken = default) =>
        _lines.Writer.WriteAsync(line, cancellationToken);

    /// <summary>Sends what is queued and stops. Completes when the last chunk was sent.</summary>
    public Task CompleteAsync()
    {
        _lines.Writer.TryComplete();
        return _sender;
    }

    private async Task SendLoopAsync()
    {
        var reader = _lines.Reader;
        var chunk = new StringBuilder();
        long seq = 0;
        while (await reader.WaitToReadAsync().ConfigureAwait(false))
        {
            chunk.Clear();
            var first = Stopwatch.StartNew();
            var open = true;
            while (true)
            {
                while (chunk.Length < MaxChunkBytes && reader.TryRead(out var line))
                {
                    chunk.Append(line).Append('\n');
                }

                var left = FlushDelay - first.Elapsed;
                if (!open || chunk.Length >= ChunkBytes || left <= TimeSpan.Zero)
                {
                    break;
                }

                var more = reader.WaitToReadAsync().AsTask();
                if (await Task.WhenAny(more, Task.Delay(left)).ConfigureAwait(false) != more)
                {
                    break;
                }

                open = await more.ConfigureAwait(false);
            }

            if (chunk.Length > 0)
            {
                try
                {
                    await _send(seq++, chunk.ToString()).ConfigureAwait(false);
                }
                catch (Exception ex) when (ex is IOException or ObjectDisposedException or StreamJsonRpc.ConnectionLostException)
                {
                    // The shell is gone; keep draining so writers never block on a dead connection.
                }

                Interlocked.Increment(ref _sent);
            }
        }
    }
}
