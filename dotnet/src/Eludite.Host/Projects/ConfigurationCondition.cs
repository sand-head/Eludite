using System.Text;

namespace Eludite.Host.Projects;

/// <summary>
/// The configuration conditions Visual Studio writes on property groups (brief 0049, the condition rule):
/// <c>'$(Configuration)|$(Platform)'=='Debug|AnyCPU'</c>, <c>'$(Configuration)'=='Release'</c>,
/// <c>'$(TargetFramework)'=='net8.0'</c> and <c>'$(Configuration)|$(TargetFramework)|$(Platform)'=='Debug|net8.0|AnyCPU'</c>.
/// Two conditions match when they name the same properties with the same values, whatever their spacing, order and
/// case.
/// </summary>
public static class ConfigurationCondition
{
    /// <summary>The properties a configuration condition may test.</summary>
    public static readonly string[] Dimensions = ["Configuration", "TargetFramework", "Platform"];

    /// <summary>The project platform MSBuild uses for the solution's <c>Any CPU</c>.</summary>
    public static string ProjectPlatform(string platform) =>
        string.Equals(platform.Replace(" ", string.Empty, StringComparison.Ordinal), "AnyCPU", StringComparison.OrdinalIgnoreCase)
            ? "AnyCPU"
            : platform;

    /// <summary>
    /// The properties and values <paramref name="condition"/> tests, or null when it is not a configuration condition of
    /// the form <c>'$(A)|$(B)'=='a|b'</c> over <see cref="Dimensions"/>.
    /// </summary>
    public static Dictionary<string, string>? Parse(string? condition)
    {
        if (string.IsNullOrWhiteSpace(condition))
        {
            return null;
        }

        var compact = OutsideQuotesWithoutSpaces(condition);
        var eq = compact.IndexOf("==", StringComparison.Ordinal);
        if (eq < 0)
        {
            return null;
        }

        var left = Unquote(compact[..eq]);
        var right = Unquote(compact[(eq + 2)..]);
        if (left is null || right is null)
        {
            return null;
        }

        var names = left.Split('|');
        var values = right.Split('|');
        if (names.Length != values.Length)
        {
            return null;
        }

        var result = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        for (var i = 0; i < names.Length; i++)
        {
            var n = names[i].Trim();
            if (!n.StartsWith("$(", StringComparison.Ordinal) || !n.EndsWith(')'))
            {
                return null;
            }

            var name = n[2..^1];
            var dimension = Dimensions.FirstOrDefault(d => string.Equals(d, name, StringComparison.OrdinalIgnoreCase));
            if (dimension is null || !result.TryAdd(dimension, values[i].Trim()))
            {
                return null;
            }
        }

        return result;
    }

    /// <summary>True when <paramref name="condition"/> is a configuration condition (<see cref="Parse"/>).</summary>
    public static bool IsConfigurationCondition(string? condition) => Parse(condition) is not null;

    /// <summary>The wanted dimensions of an edit; empty for the unconditioned value.</summary>
    public static Dictionary<string, string> Key(string? configuration, string? platform, string? framework)
    {
        var key = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        if (!string.IsNullOrEmpty(configuration))
        {
            key["Configuration"] = configuration;
        }

        if (!string.IsNullOrEmpty(framework))
        {
            key["TargetFramework"] = framework;
        }

        if (!string.IsNullOrEmpty(platform))
        {
            key["Platform"] = ProjectPlatform(platform);
        }

        return key;
    }

    /// <summary>
    /// The condition Visual Studio writes for <paramref name="key"/>: <c>'$(Configuration)|$(Platform)'=='Debug|AnyCPU'</c>
    /// (the dimensions in Configuration, TargetFramework, Platform order); null for an empty key.
    /// </summary>
    public static string? Build(IReadOnlyDictionary<string, string> key)
    {
        ArgumentNullException.ThrowIfNull(key);
        var dims = Dimensions.Where(key.ContainsKey).ToList();
        if (dims.Count == 0)
        {
            return null;
        }

        var left = string.Join('|', dims.Select(d => $"$({d})"));
        var right = string.Join('|', dims.Select(d => key[d]));
        return $"'{left}'=='{right}'";
    }

    /// <summary>True when <paramref name="condition"/> tests exactly the dimensions and values of <paramref name="key"/>.</summary>
    public static bool Matches(string? condition, IReadOnlyDictionary<string, string> key)
    {
        ArgumentNullException.ThrowIfNull(key);
        var parsed = Parse(condition);
        if (parsed is null || parsed.Count != key.Count)
        {
            return false;
        }

        foreach (var (name, value) in key)
        {
            if (!parsed.TryGetValue(name, out var v) || !Same(name, v, value))
            {
                return false;
            }
        }

        return true;
    }

    private static bool Same(string dimension, string a, string b) =>
        dimension.Equals("Platform", StringComparison.OrdinalIgnoreCase)
            ? string.Equals(ProjectPlatform(a), ProjectPlatform(b), StringComparison.OrdinalIgnoreCase)
            : string.Equals(a, b, StringComparison.OrdinalIgnoreCase);

    private static string? Unquote(string s) =>
        s.Length >= 2 && s[0] == '\'' && s[^1] == '\'' ? s[1..^1] : null;

    private static string OutsideQuotesWithoutSpaces(string s)
    {
        var sb = new StringBuilder(s.Length);
        var quoted = false;
        foreach (var c in s)
        {
            if (c == '\'')
            {
                quoted = !quoted;
            }

            if (!quoted && char.IsWhiteSpace(c))
            {
                continue;
            }

            sb.Append(c);
        }

        return sb.ToString();
    }
}
