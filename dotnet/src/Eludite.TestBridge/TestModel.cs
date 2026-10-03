using System.Text.Json;

namespace Eludite.TestBridge;

/// <summary>A trait of a test (xunit <c>[Trait]</c>, MSTest <c>[TestCategory]</c> and NUnit <c>[Category]</c> as Category).</summary>
public sealed record TestTrait(string Name, string Value);

/// <summary>
/// A discovered test, one model for both protocols (protocol/schemas/host-rpc.md, "Tests", the mapping table).
/// </summary>
/// <param name="Id">The runner's id: MTP's node <c>uid</c>, VSTest's TestCase <c>Id</c>.</param>
/// <param name="DisplayName">As the framework names it (a data row with its arguments).</param>
/// <param name="FullyQualifiedName"><c>Namespace.Class.Method</c>, without a data row's arguments.</param>
public sealed record TestItem(string Id, string DisplayName, string FullyQualifiedName)
{
    public string? Namespace { get; init; }

    public string? ClassName { get; init; }

    public string? Method { get; init; }

    /// <summary>Absolute path of the declaring source file, when the framework reports it.</summary>
    public string? Source { get; init; }

    /// <summary>1-based line the framework reports.</summary>
    public int? Line { get; init; }

    public IReadOnlyList<TestTrait>? Traits { get; init; }
}

/// <summary>The outcomes of <see cref="TestResult.Outcome"/> (host-rpc.md: running, passed, failed, skipped, notRun).</summary>
public static class TestOutcomes
{
    public const string Running = "running";
    public const string Passed = "passed";
    public const string Failed = "failed";
    public const string Skipped = "skipped";
    public const string NotRun = "notRun";
}

/// <summary>A test's state in a run.</summary>
/// <param name="Id">The test's discovered id (or the runner's id of a test discovery did not list).</param>
/// <param name="Outcome">One of <see cref="TestOutcomes"/>.</param>
public sealed record TestResult(string Id, string Outcome)
{
    public double? DurationMs { get; init; }

    /// <summary>The failure message, or the skip reason.</summary>
    public string? Message { get; init; }

    public string? StackTrace { get; init; }

    /// <summary>Standard output, then standard error.</summary>
    public string? Output { get; init; }

    /// <summary>Only for a test discovery did not list.</summary>
    public string? DisplayName { get; init; }

    /// <summary>Only for a test discovery did not list.</summary>
    public string? FullyQualifiedName { get; init; }
}

/// <summary>A discovered test with the runner's own object for it (the MTP node, the VSTest TestCase), sent back as is to run it.</summary>
public sealed record DiscoveredTest(TestItem Item, JsonElement Native);

/// <summary>Splits fully qualified names into the model's parts.</summary>
public static class TestNames
{
    /// <summary>
    /// <c>Namespace.Class.Method</c> into its namespace, class and method. A nested class keeps its <c>+</c> path; a
    /// method's parameter list (<c>Method(System.Int32)</c>) is dropped first.
    /// </summary>
    public static (string? Namespace, string? ClassName, string? Method) Split(string? type, string? method)
    {
        method = StripParameters(method);
        if (string.IsNullOrEmpty(type))
        {
            return (null, null, method);
        }

        var dot = type.LastIndexOf('.');
        return dot < 0 ? (null, type, method) : (type[..dot], type[(dot + 1)..], method);
    }

    /// <summary><c>Method(System.Int32)</c> to <c>Method</c>.</summary>
    public static string? StripParameters(string? method)
    {
        if (method is null)
        {
            return null;
        }

        var paren = method.IndexOf('(', StringComparison.Ordinal);
        return paren < 0 ? method : method[..paren];
    }

    /// <summary>Type and method joined, or the fallback (the runner's fully qualified name, without arguments).</summary>
    public static string FullName(string? type, string? method, string fallback)
    {
        var m = StripParameters(method);
        if (!string.IsNullOrEmpty(type) && !string.IsNullOrEmpty(m))
        {
            return type + "." + m;
        }

        return StripParameters(fallback) ?? fallback;
    }
}
