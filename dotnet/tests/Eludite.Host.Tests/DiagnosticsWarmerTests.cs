using System.Collections.Concurrent;
using Microsoft.Extensions.Time.Testing;
using Eludite.Host.Lsp;

namespace Eludite.Host.Tests;

/// <summary>The warming scheduler: immediate pulls on open, debounced pulls on change, newest pull wins.</summary>
public sealed class DiagnosticsWarmerTests : IDisposable
{
    private readonly FakeTimeProvider _time = new();
    private readonly ConcurrentQueue<string> _pulls = new();
    private readonly ConcurrentQueue<string> _canceled = new();
    private readonly DiagnosticsWarmer _warmer;
    private TaskCompletionSource? _gate;

    public DiagnosticsWarmerTests()
    {
        _warmer = new DiagnosticsWarmer(PullAsync, TextWriter.Null, TimeSpan.FromMilliseconds(150), _time);
    }

    private async Task PullAsync(string uri, CancellationToken ct)
    {
        _pulls.Enqueue(uri);
        if (_gate is { } gate)
        {
            try
            {
                await gate.Task.WaitAsync(ct);
            }
            catch (OperationCanceledException)
            {
                _canceled.Enqueue(uri);
                throw;
            }
        }
    }

    private static async Task Eventually(Func<bool> condition)
    {
        for (var i = 0; i < 500 && !condition(); i++)
        {
            await Task.Delay(5, TestContext.Current.CancellationToken);
        }

        Assert.True(condition());
    }

    [Fact]
    public async Task Opened_PullsAtOnce()
    {
        _warmer.Opened("a");

        await Eventually(() => _pulls.Count == 1);
        Assert.Equal(1, _warmer.PullsStarted);
    }

    [Fact]
    public async Task Changes_AreDebouncedIntoOnePull()
    {
        for (var i = 0; i < 5; i++)
        {
            _warmer.Changed("a");
            _time.Advance(TimeSpan.FromMilliseconds(100));
        }

        await Task.Delay(50, TestContext.Current.CancellationToken);
        Assert.Empty(_pulls);

        _time.Advance(TimeSpan.FromMilliseconds(49));
        await Task.Delay(50, TestContext.Current.CancellationToken);
        Assert.Empty(_pulls);

        _time.Advance(TimeSpan.FromMilliseconds(1));
        await Eventually(() => _pulls.Count == 1);
        await Task.Delay(50, TestContext.Current.CancellationToken);
        Assert.Single(_pulls);
    }

    [Fact]
    public async Task Debounce_IsPerDocument()
    {
        _warmer.Changed("a");
        _time.Advance(TimeSpan.FromMilliseconds(100));
        _warmer.Changed("b");
        _time.Advance(TimeSpan.FromMilliseconds(50));

        await Eventually(() => _pulls.Count == 1);
        Assert.Equal(["a"], _pulls);

        _time.Advance(TimeSpan.FromMilliseconds(100));
        await Eventually(() => _pulls.Count == 2);
        Assert.Equal(["a", "b"], _pulls);
    }

    [Fact]
    public async Task NewerPull_CancelsTheOlderOne()
    {
        _gate = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        _warmer.Opened("a");
        await Eventually(() => _pulls.Count == 1);

        _warmer.PullNow("a");

        await Eventually(() => _canceled.Count == 1);
        await Eventually(() => _pulls.Count == 2);
        _gate.TrySetResult();
    }

    [Fact]
    public async Task Closed_CancelsPendingTimerAndPullInFlight()
    {
        _gate = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        _warmer.Opened("a");
        await Eventually(() => _pulls.Count == 1);
        _warmer.Changed("a");

        _warmer.Closed("a");
        _time.Advance(TimeSpan.FromSeconds(1));

        await Eventually(() => _canceled.Count == 1);
        await Task.Delay(50, TestContext.Current.CancellationToken);
        Assert.Single(_pulls);
    }

    [Fact]
    public void Debounce_DefaultsTo150ms()
    {
        Assert.Equal(TimeSpan.FromMilliseconds(150), DiagnosticsWarmer.DefaultDebounce);
        using var warmer = new DiagnosticsWarmer((_, _) => Task.CompletedTask, TextWriter.Null);
        Assert.Equal(DiagnosticsWarmer.DefaultDebounce, warmer.Debounce);
    }

    public void Dispose() => _warmer.Dispose();
}
