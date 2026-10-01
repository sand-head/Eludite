using System.Globalization;

namespace Eludite.Host.Lsp;

/// <summary>
/// Schedules the host's warming <c>textDocument/diagnostic</c> pulls (protocol/schemas/host-rpc.md, "Semantics
/// warming"). Roslyn's LSP completion runs on frozen-partial semantics and the server never compiles dependencies on
/// its own, so without a pull, member completion on types from referenced projects stays empty (brief 0002 report).
/// </summary>
/// <remarks>
/// A document is pulled at once when opened, <see cref="Debounce"/> after its last change, and on demand (solution
/// loaded, <c>workspace/diagnostic/refresh</c>). A newer pull for a document cancels the older one. Pulls run on the
/// thread pool and never block the caller.
/// </remarks>
public sealed class DiagnosticsWarmer : IDisposable
{
    /// <summary>Default debounce after a change: 150 ms. <c>ELUDITE_DIAGNOSTICS_DEBOUNCE_MS</c> overrides it.</summary>
    public static readonly TimeSpan DefaultDebounce = TimeSpan.FromMilliseconds(150);

    private readonly Func<string, CancellationToken, Task> _pull;
    private readonly TimeProvider _time;
    private readonly TextWriter _log;
    private readonly Lock _lock = new();
    private readonly Dictionary<string, Entry> _entries = new(StringComparer.Ordinal);
    private bool _disposed;
    private long _started;

    /// <param name="pull">Pulls diagnostics for one document; must honor the token.</param>
    public DiagnosticsWarmer(Func<string, CancellationToken, Task> pull, TextWriter log, TimeSpan? debounce = null, TimeProvider? time = null)
    {
        _pull = pull;
        _log = log;
        _time = time ?? TimeProvider.System;
        Debounce = debounce ?? DefaultDebounce;
    }

    public TimeSpan Debounce { get; }

    /// <summary>Number of pulls started so far.</summary>
    public long PullsStarted => Interlocked.Read(ref _started);

    /// <summary>Reads <c>ELUDITE_DIAGNOSTICS_DEBOUNCE_MS</c>; null when unset or invalid.</summary>
    public static TimeSpan? DebounceFromEnvironment() =>
        int.TryParse(Environment.GetEnvironmentVariable("ELUDITE_DIAGNOSTICS_DEBOUNCE_MS"), NumberStyles.Integer, CultureInfo.InvariantCulture, out var ms) && ms >= 0
            ? TimeSpan.FromMilliseconds(ms)
            : null;

    /// <summary>The document was opened: pull now.</summary>
    public void Opened(string uri) => PullNow(uri);

    /// <summary>The document changed: pull after <see cref="Debounce"/> unless it changes again first.</summary>
    public void Changed(string uri)
    {
        lock (_lock)
        {
            if (_disposed)
            {
                return;
            }

            var entry = GetEntry(uri);
            entry.Timer?.Dispose();
            entry.Timer = _time.CreateTimer(_ => PullNow(uri), null, Debounce, Timeout.InfiniteTimeSpan);
        }
    }

    /// <summary>The document was closed: cancel its timer and any pull in flight.</summary>
    public void Closed(string uri)
    {
        Entry? entry;
        lock (_lock)
        {
            if (_entries.Remove(uri, out entry))
            {
                entry.Timer?.Dispose();
            }
        }

        entry?.Cancel();
    }

    /// <summary>Pulls every given document now (solution loaded, diagnostic refresh).</summary>
    public void PullAll(IEnumerable<string> uris)
    {
        ArgumentNullException.ThrowIfNull(uris);
        foreach (var uri in uris)
        {
            PullNow(uri);
        }
    }

    /// <summary>Starts a pull for <paramref name="uri"/>, canceling the previous one.</summary>
    public void PullNow(string uri)
    {
        CancellationTokenSource cts;
        CancellationTokenSource? previous;
        lock (_lock)
        {
            if (_disposed)
            {
                return;
            }

            var entry = GetEntry(uri);
            entry.Timer?.Dispose();
            entry.Timer = null;
            previous = entry.InFlight;
            cts = new CancellationTokenSource();
            entry.InFlight = cts;
            Interlocked.Increment(ref _started);
        }

        previous?.Cancel();
        _ = Task.Run(() => RunAsync(uri, cts));
    }

    private async Task RunAsync(string uri, CancellationTokenSource cts)
    {
        try
        {
            await _pull(uri, cts.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            // Superseded by a newer pull, or the document closed.
        }
        catch (Exception ex) when (ex is not OutOfMemoryException)
        {
            await _log.WriteLineAsync($"warming pull for {uri} failed: {ex.GetBaseException().Message}").ConfigureAwait(false);
        }
        finally
        {
            lock (_lock)
            {
                if (_entries.TryGetValue(uri, out var entry) && ReferenceEquals(entry.InFlight, cts))
                {
                    entry.InFlight = null;
                }
            }

            // Not disposed: a newer pull may still call Cancel on it; it holds no timer or wait handle.
        }
    }

    private Entry GetEntry(string uri)
    {
        if (!_entries.TryGetValue(uri, out var entry))
        {
            entry = new Entry();
            _entries[uri] = entry;
        }

        return entry;
    }

    public void Dispose()
    {
        List<Entry> entries;
        lock (_lock)
        {
            _disposed = true;
            entries = [.. _entries.Values];
            _entries.Clear();
        }

        foreach (var e in entries)
        {
            e.Timer?.Dispose();
            e.Cancel();
        }
    }

    private sealed class Entry
    {
        public ITimer? Timer { get; set; }

        public CancellationTokenSource? InFlight { get; set; }

        public void Cancel()
        {
            InFlight?.Cancel();
        }
    }
}
