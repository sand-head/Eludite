namespace Eludite.Testing;

/// <summary>
/// Wall-clock time in tests, in one place for every .NET test project (linked by <c>dotnet/tests/Directory.Build.props</c>
/// and the Mono adapter's test project).
/// <para>
/// A timing budget is a performance claim, not a correctness one: it is asserted on a developer machine and reported, never
/// asserted, on a CI run, whose hosted runners are shared VMs as fast as free compute makes them. A test checks behavior
/// with ordinary assertions and hands only the time to <see cref="Assert(string, TimeSpan, TimeSpan)"/>.
/// <c>ELUDITE_BUDGETS=assert</c> or <c>report</c> overrides the choice either way.
/// </para>
/// <para>
/// A bound that only exists to turn a hang into a failure (a wait for a process, a build, an adapter) goes through
/// <see cref="Hang"/>, which gives it room on CI: such a bound fails a test only when something is really stuck.
/// </para>
/// </summary>
internal static class Budget
{
    /// <summary>A CI run: <c>CI</c> (GitHub Actions and most others), <c>GITHUB_ACTIONS</c> or <c>TF_BUILD</c> (Azure Pipelines).</summary>
    public static bool HostedRunner { get; } =
        Set("CI") || Set("GITHUB_ACTIONS") || Set("TF_BUILD");

    /// <summary>Whether <see cref="Assert(string, TimeSpan, TimeSpan)"/> asserts (true) or only reports (false).</summary>
    public static bool Enforced { get; } = Environment.GetEnvironmentVariable("ELUDITE_BUDGETS") switch
    {
        "assert" => true,
        "report" => false,
        _ => !HostedRunner,
    };

    /// <summary>How much longer than on a developer machine a hang bound is on CI; <c>ELUDITE_TEST_TIMEOUT_SCALE</c> overrides it.</summary>
    public static double TimeoutScale { get; } =
        double.TryParse(Environment.GetEnvironmentVariable("ELUDITE_TEST_TIMEOUT_SCALE"), System.Globalization.NumberStyles.Float,
            System.Globalization.CultureInfo.InvariantCulture, out var scale) && scale >= 1
            ? scale
            : HostedRunner ? 3 : 1;

    /// <summary><paramref name="measured"/> under <paramref name="limit"/>: asserted when <see cref="Enforced"/>, reported always.</summary>
    public static void Assert(string what, TimeSpan measured, TimeSpan limit)
    {
        var line = $"timing: {what} {measured.TotalMilliseconds:F1} ms (budget {limit.TotalMilliseconds:F0} ms)";
        if (!Enforced)
        {
            Report($"{line}, not asserted: a CI run, not a reference machine");
            return;
        }
        Report(line);
        Xunit.Assert.True(measured < limit, $"{what} took {measured.TotalMilliseconds:F1} ms (budget {limit.TotalMilliseconds:F0} ms)");
    }

    /// <summary><see cref="Assert(string, TimeSpan, TimeSpan)"/> for times kept in milliseconds.</summary>
    public static void Assert(string what, double measuredMs, double limitMs) =>
        Assert(what, TimeSpan.FromMilliseconds(measuredMs), TimeSpan.FromMilliseconds(limitMs));

    /// <summary>A bound that only catches a hang, scaled by <see cref="TimeoutScale"/>.</summary>
    public static TimeSpan Hang(TimeSpan bound) => bound * TimeoutScale;

    private static void Report(string line)
    {
        TestContext.Current.TestOutputHelper?.WriteLine(line);
        Console.Error.WriteLine(line);
    }

    private static bool Set(string name) => !string.IsNullOrEmpty(Environment.GetEnvironmentVariable(name));
}
