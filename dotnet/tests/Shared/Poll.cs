using System.Diagnostics;

namespace Eludite.Testing;

/// <summary>
/// Waiting for a condition another thread or process makes true. The bound only turns a hang into a failure, so it is
/// generous and scaled for CI (<see cref="Budget.Hang"/>). Waiting for a condition instead of sleeping a fixed time keeps a
/// test fast on a quiet machine and correct on a slow one.
/// </summary>
internal static class Poll
{
    /// <summary>The default hang bound before scaling.</summary>
    public static readonly TimeSpan DefaultBound = TimeSpan.FromSeconds(30);

    /// <summary>Waits until <paramref name="condition"/> holds; fails naming <paramref name="what"/> after the bound.</summary>
    public static async Task UntilAsync(Func<bool> condition, string what, TimeSpan? bound = null, CancellationToken cancellationToken = default)
    {
        var limit = Budget.Hang(bound ?? DefaultBound);
        var clock = Stopwatch.StartNew();
        if (!cancellationToken.CanBeCanceled)
        {
            cancellationToken = TestContext.Current.CancellationToken;
        }

        while (!condition())
        {
            if (clock.Elapsed > limit)
            {
                Assert.Fail($"timed out after {limit.TotalSeconds:F0} s waiting for {what}");
            }

            await Task.Delay(5, cancellationToken);
        }
    }
}
