using System.Text;

namespace Eludite.Host.Build;

/// <summary>
/// The output chunks of a running build, kept for <c>eludite/build/status</c> (brief 0020): the last
/// <see cref="MaxBytes"/> bytes (UTF-8) of whole chunks, in order. Older chunks are dropped whole, so the text always
/// starts at a chunk (and so a line) boundary. Thread-safe.
/// </summary>
public sealed class OutputHistory
{
    /// <summary>How much output a running build keeps: 8 MiB.</summary>
    public const long DefaultMaxBytes = 8L * 1024 * 1024;

    private readonly Lock _lock = new();
    private readonly Queue<(long Seq, string Text, long Bytes)> _chunks = new();
    private long _bytes;
    private long _nextSeq;
    private bool _truncated;

    public OutputHistory(long maxBytes = DefaultMaxBytes) => MaxBytes = maxBytes;

    public long MaxBytes { get; }

    /// <summary>Records chunk <paramref name="seq"/> (sent in order, from 0).</summary>
    public void Add(long seq, string text)
    {
        var bytes = Encoding.UTF8.GetByteCount(text);
        lock (_lock)
        {
            _chunks.Enqueue((seq, text, bytes));
            _bytes += bytes;
            _nextSeq = seq + 1;
            while (_bytes > MaxBytes && _chunks.Count > 1)
            {
                var (_, _, dropped) = _chunks.Dequeue();
                _bytes -= dropped;
                _truncated = true;
            }
        }
    }

    /// <summary>The kept chunks, concatenated.</summary>
    public BuildStatusOutput Snapshot()
    {
        lock (_lock)
        {
            var text = new StringBuilder((int)Math.Min(_bytes, int.MaxValue));
            foreach (var (_, t, _) in _chunks)
            {
                text.Append(t);
            }

            var first = _chunks.Count > 0 ? _chunks.Peek().Seq : _nextSeq;
            return new BuildStatusOutput(first, _nextSeq, text.ToString(), _truncated);
        }
    }
}
