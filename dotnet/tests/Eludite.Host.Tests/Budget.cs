namespace Eludite.Host.Tests;

/// <summary>
/// Timing budgets: asserted on a developer machine, printed under CI (<c>CI</c> set), whose hosted runners are only as
/// fast as free compute makes them, not a reference machine.
/// </summary>
internal static class Budget
{
    public static bool HostedRunner => Environment.GetEnvironmentVariable("CI") is not null;

    public static void Assert(string what, TimeSpan measured, TimeSpan limit)
    {
        if (HostedRunner)
        {
            Console.Error.WriteLine(
                $"timing: {what} {measured.TotalMilliseconds:F1} ms not asserted against {limit.TotalMilliseconds:F0} ms: a CI run, not a reference machine");
            return;
        }
        Xunit.Assert.True(measured < limit, $"{what} took {measured.TotalMilliseconds:F1} ms (budget {limit.TotalMilliseconds:F0} ms)");
    }
}
