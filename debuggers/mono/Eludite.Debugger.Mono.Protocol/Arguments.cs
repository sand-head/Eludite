using System.Globalization;
using System.Text;
using Newtonsoft.Json.Linq;

namespace Eludite.Debugger.Mono.Protocol;

/// <summary><c>launch</c>'s arguments (protocol/schemas/dap-mono.md).</summary>
public sealed class LaunchArguments
{
    /// <summary>The program's .exe, absolute.</summary>
    public string Program { get; private set; } = string.Empty;

    public IReadOnlyList<string> Args { get; private set; } = Array.Empty<string>();

    /// <summary>The working directory, absolute (default: the program's folder).</summary>
    public string Cwd { get; private set; } = string.Empty;

    public IReadOnlyDictionary<string, string> Env { get; private set; } = new Dictionary<string, string>();

    /// <summary>The mono to run the program with; null: the one running the adapter.</summary>
    public string? RuntimeExecutable { get; private set; }

    public IReadOnlyList<string> RuntimeArgs { get; private set; } = Array.Empty<string>();

    public bool StopAtEntry { get; private set; }

    public bool JustMyCode { get; private set; } = true;

    /// <summary>Parse and check <paramref name="arguments"/>; relative paths resolve against <paramref name="baseDirectory"/>.</summary>
    /// <exception cref="DapException">A required argument is missing or has the wrong type.</exception>
    public static LaunchArguments Parse(JObject arguments, string baseDirectory)
    {
        var program = Json.String(arguments, "program");
        if (string.IsNullOrWhiteSpace(program))
        {
            throw new DapException("launch needs `program`, the .exe to run");
        }

        var cwdArg = Json.String(arguments, "cwd");
        var cwd = string.IsNullOrEmpty(cwdArg) ? null : Path.GetFullPath(Path.Combine(baseDirectory, cwdArg));
        var programPath = Path.GetFullPath(Path.Combine(cwd ?? baseDirectory, program));
        return new LaunchArguments
        {
            Program = programPath,
            Args = Json.Strings(arguments, "args"),
            Cwd = cwd ?? Path.GetDirectoryName(programPath) ?? baseDirectory,
            Env = Json.StringMap(arguments, "env"),
            RuntimeExecutable = Json.String(arguments, "runtimeExecutable") is { Length: > 0 } r ? r : null,
            RuntimeArgs = Json.Strings(arguments, "runtimeArgs"),
            StopAtEntry = Json.Bool(arguments, "stopAtEntry") ?? false,
            JustMyCode = Json.Bool(arguments, "justMyCode") ?? true,
        };
    }
}

/// <summary><c>attach</c>'s arguments: where a program's debugger agent listens.</summary>
public sealed class AttachArguments
{
    public string Address { get; private set; } = "127.0.0.1";

    public int Port { get; private set; }

    /// <exception cref="DapException"><c>port</c> is missing or out of range.</exception>
    public static AttachArguments Parse(JObject arguments)
    {
        var port = arguments["port"];
        if (port is null || port.Type != JTokenType.Integer || (int)port is < 1 or > 65535)
        {
            throw new DapException("attach needs `port`, the port the program's debugger agent listens on (1 to 65535)");
        }

        return new AttachArguments
        {
            Address = Json.String(arguments, "address") is { Length: > 0 } a ? a : "127.0.0.1",
            Port = (int)port,
        };
    }
}

/// <summary>How a hit condition compares the hit count.</summary>
public enum HitConditionKind
{
    EqualTo,
    GreaterThanOrEqualTo,
    GreaterThan,
    LessThan,
    LessThanOrEqualTo,
    MultipleOf,
}

/// <summary>
/// A breakpoint's <c>hitCondition</c> as Eludite's shell sends it (Visual Studio's Hit Count): <c>N</c> breaks on the
/// Nth hit, <c>&gt;=N</c> from the Nth on, <c>%N</c> on every Nth; <c>==N</c>, <c>&gt;N</c>, <c>&lt;N</c> and
/// <c>&lt;=N</c> are accepted too. N is at least 1.
/// </summary>
public readonly struct HitCondition : IEquatable<HitCondition>
{
    public HitCondition(HitConditionKind kind, int count)
    {
        Kind = kind;
        Count = count;
    }

    public HitConditionKind Kind { get; }

    public int Count { get; }

    /// <summary>Parse <paramref name="text"/>; false when it is not a hit condition.</summary>
    public static bool TryParse(string? text, out HitCondition condition)
    {
        condition = default;
        var s = (text ?? string.Empty).Trim();
        var kind = HitConditionKind.EqualTo;
        foreach (var (prefix, k) in Prefixes)
        {
            if (s.StartsWith(prefix, StringComparison.Ordinal))
            {
                kind = k;
                s = s.Substring(prefix.Length).Trim();
                break;
            }
        }

        if (!int.TryParse(s, NumberStyles.None, CultureInfo.InvariantCulture, out var n) || n < 1)
        {
            return false;
        }

        condition = new HitCondition(kind, n);
        return true;
    }

    private static readonly (string Prefix, HitConditionKind Kind)[] Prefixes =
    {
        (">=", HitConditionKind.GreaterThanOrEqualTo),
        ("<=", HitConditionKind.LessThanOrEqualTo),
        ("==", HitConditionKind.EqualTo),
        (">", HitConditionKind.GreaterThan),
        ("<", HitConditionKind.LessThan),
        ("%", HitConditionKind.MultipleOf),
        ("=", HitConditionKind.EqualTo),
    };

    /// <summary>Whether the <paramref name="hits"/>th hit breaks (what the debugger library decides; for tests).</summary>
    public bool BreaksOn(int hits) => Kind switch
    {
        HitConditionKind.EqualTo => hits == Count,
        HitConditionKind.GreaterThanOrEqualTo => hits >= Count,
        HitConditionKind.GreaterThan => hits > Count,
        HitConditionKind.LessThan => hits < Count,
        HitConditionKind.LessThanOrEqualTo => hits <= Count,
        HitConditionKind.MultipleOf => hits % Count == 0,
        _ => false,
    };

    public bool Equals(HitCondition other) => Kind == other.Kind && Count == other.Count;

    public override bool Equals(object? obj) => obj is HitCondition h && Equals(h);

    public override int GetHashCode() => ((int)Kind * 397) ^ Count;

    public static bool operator ==(HitCondition left, HitCondition right) => left.Equals(right);

    public static bool operator !=(HitCondition left, HitCondition right) => !left.Equals(right);
}

/// <summary>One piece of a log point's message: literal text or an expression to evaluate.</summary>
public readonly struct LogSegment
{
    public LogSegment(string text, bool isExpression)
    {
        Text = text;
        IsExpression = isExpression;
    }

    public string Text { get; }

    public bool IsExpression { get; }
}

/// <summary>
/// A log point's <c>logMessage</c>: text with <c>{expression}</c> parts evaluated in the hit frame. <c>{{</c> and
/// <c>}}</c> are literal braces; an unclosed <c>{</c> is literal text.
/// </summary>
public static class LogMessage
{
    public static IReadOnlyList<LogSegment> Parse(string message)
    {
        var segments = new List<LogSegment>();
        var text = new StringBuilder();
        var i = 0;
        while (i < message.Length)
        {
            var c = message[i];
            if (c == '{' && i + 1 < message.Length && message[i + 1] == '{')
            {
                text.Append('{');
                i += 2;
                continue;
            }

            if (c == '}' && i + 1 < message.Length && message[i + 1] == '}')
            {
                text.Append('}');
                i += 2;
                continue;
            }

            if (c == '{')
            {
                var close = message.IndexOf('}', i + 1);
                if (close < 0)
                {
                    text.Append(message, i, message.Length - i);
                    break;
                }

                if (text.Length > 0)
                {
                    segments.Add(new LogSegment(text.ToString(), isExpression: false));
                    text.Clear();
                }

                segments.Add(new LogSegment(message.Substring(i + 1, close - i - 1).Trim(), isExpression: true));
                i = close + 1;
                continue;
            }

            text.Append(c);
            i++;
        }

        if (text.Length > 0)
        {
            segments.Add(new LogSegment(text.ToString(), isExpression: false));
        }

        return segments;
    }

    /// <summary>The message with each expression replaced by <paramref name="evaluate"/>'s answer.</summary>
    public static string Interpolate(string message, Func<string, string> evaluate)
    {
        var sb = new StringBuilder();
        foreach (var s in Parse(message))
        {
            sb.Append(s.IsExpression ? evaluate(s.Text) : s.Text);
        }

        return sb.ToString();
    }

    /// <summary>
    /// The message in Mono.Debugging's trace syntax (<c>Breakpoint.TraceExpression</c>, which evaluates <c>{expr}</c>
    /// and reads <c>{{</c> as a literal brace but has no escape for <c>}</c>), so the library interpolates it as
    /// <see cref="Interpolate"/> does.
    /// </summary>
    public static string ToTraceExpression(string message)
    {
        var sb = new StringBuilder();
        foreach (var s in Parse(message))
        {
            if (s.IsExpression)
            {
                sb.Append('{').Append(s.Text).Append('}');
            }
            else
            {
                sb.Append(s.Text.Replace("{", "{{"));
            }
        }

        return sb.ToString();
    }
}

/// <summary>What <c>setExceptionBreakpoints</c> asks for.</summary>
public sealed class ExceptionSettings
{
    /// <summary>The filter for first-chance exceptions.</summary>
    public const string All = "all";

    /// <summary>The filter for exceptions user code does not handle.</summary>
    public const string UserUnhandled = "user-unhandled";

    /// <summary>Break when one of these types (or a subclass) is thrown; empty when <see cref="All"/> is off.</summary>
    public IReadOnlyList<string> ThrownTypes { get; private set; } = Array.Empty<string>();

    public bool BreakWhenThrown => ThrownTypes.Count > 0;

    public bool BreakWhenUserUnhandled { get; private set; }

    /// <summary>The types <see cref="UserUnhandled"/> is limited to; empty for every type.</summary>
    public IReadOnlyList<string> UnhandledTypes { get; private set; } = Array.Empty<string>();

    /// <summary>
    /// Parse <c>filters</c> and <c>filterOptions</c>. A filter option's <c>condition</c> is a comma-separated list of
    /// exception type names; <see cref="All"/> without one means <c>System.Exception</c> and every subclass.
    /// </summary>
    /// <exception cref="DapException">An unknown filter.</exception>
    public static ExceptionSettings Parse(JObject arguments)
    {
        var options = new List<(string Filter, string? Condition)>();
        foreach (var f in Json.Strings(arguments, "filters"))
        {
            options.Add((f, null));
        }

        if (arguments["filterOptions"] is JArray filterOptions)
        {
            foreach (var o in filterOptions.OfType<JObject>())
            {
                options.Add(((string?)o["filterId"] ?? string.Empty, (string?)o["condition"]));
            }
        }

        var thrown = new List<string>();
        var unhandled = new List<string>();
        var userUnhandled = false;
        foreach (var (filter, condition) in options)
        {
            var types = SplitTypes(condition);
            switch (filter)
            {
                case All:
                    thrown.AddRange(types.Count > 0 ? types : new[] { "System.Exception" });
                    break;
                case UserUnhandled:
                    userUnhandled = true;
                    unhandled.AddRange(types);
                    break;
                default:
                    throw new DapException("unknown exception filter `" + filter + "` (eludite-dbg-mono offers `all` and `user-unhandled`)");
            }
        }

        return new ExceptionSettings
        {
            ThrownTypes = thrown.Distinct(StringComparer.Ordinal).ToList(),
            BreakWhenUserUnhandled = userUnhandled,
            UnhandledTypes = unhandled.Distinct(StringComparer.Ordinal).ToList(),
        };
    }

    private static List<string> SplitTypes(string? condition) =>
        (condition ?? string.Empty)
            .Split(new[] { ',', ' ', ';' }, StringSplitOptions.RemoveEmptyEntries)
            .Select(t => t.Trim())
            .Where(t => t.Length > 0)
            .ToList();
}

/// <summary>The adapter's command line: <c>[--port N] [--log FILE]</c>.</summary>
public sealed class AdapterOptions
{
    /// <summary>The TCP port to listen on (0: any free one); null for stdio.</summary>
    public int? Port { get; private set; }

    public string? LogFile { get; private set; }

    public bool Help { get; private set; }

    public const string Usage =
        "usage: mono eludite-dbg-mono.exe [--port N] [--log FILE]\n" +
        "  Serves the Debug Adapter Protocol for .NET Framework programs under Mono (protocol/schemas/dap-mono.md).\n" +
        "  Without --port it speaks DAP on stdin and stdout. --port N listens on 127.0.0.1:N for one client\n" +
        "  (--port 0 picks a free port and prints it on stderr). Logs go to stderr and to FILE with --log.";

    /// <exception cref="DapException">An unknown option or a bad value.</exception>
    public static AdapterOptions Parse(IReadOnlyList<string> args)
    {
        var o = new AdapterOptions();
        for (var i = 0; i < args.Count; i++)
        {
            var a = args[i];
            string Value()
            {
                if (i + 1 >= args.Count)
                {
                    throw new DapException(a + " needs a value");
                }

                return args[++i];
            }

            switch (a)
            {
                case "--port":
                    var v = Value();
                    if (!int.TryParse(v, NumberStyles.None, CultureInfo.InvariantCulture, out var port) || port > 65535)
                    {
                        throw new DapException("--port needs a port number (0 to 65535), not " + v);
                    }

                    o.Port = port;
                    break;
                case "--log":
                    o.LogFile = Value();
                    break;
                case "--help":
                case "-h":
                    o.Help = true;
                    break;
                default:
                    throw new DapException("unknown option " + a);
            }
        }

        return o;
    }
}

/// <summary>Command lines for Mono's <c>Process.Start</c>, which splits its argument string as a POSIX shell would quote.</summary>
public static class CommandLine
{
    /// <summary><paramref name="args"/> as one argument string, each quoted when it needs to be.</summary>
    public static string Join(IEnumerable<string> args) => string.Join(" ", args.Select(Quote));

    /// <summary>One argument: as is when it has no blank, quote or backslash, else in double quotes with <c>"</c> and <c>\</c> escaped.</summary>
    public static string Quote(string arg)
    {
        if (arg.Length > 0 && !arg.Any(c => char.IsWhiteSpace(c) || c is '"' or '\'' or '\\'))
        {
            return arg;
        }

        var sb = new StringBuilder("\"");
        foreach (var c in arg)
        {
            if (c is '"' or '\\')
            {
                sb.Append('\\');
            }

            sb.Append(c);
        }

        return sb.Append('"').ToString();
    }
}

/// <summary>Small readers over JSON arguments that throw <see cref="DapException"/> on a wrong type.</summary>
public static class Json
{
    public static string? String(JObject o, string name)
    {
        var t = o[name];
        if (t is null || t.Type == JTokenType.Null)
        {
            return null;
        }

        if (t.Type != JTokenType.String)
        {
            throw new DapException("`" + name + "` must be a string");
        }

        return (string?)t;
    }

    public static bool? Bool(JObject o, string name)
    {
        var t = o[name];
        if (t is null || t.Type == JTokenType.Null)
        {
            return null;
        }

        if (t.Type != JTokenType.Boolean)
        {
            throw new DapException("`" + name + "` must be true or false");
        }

        return (bool)t;
    }

    public static int? Int(JObject o, string name)
    {
        var t = o[name];
        if (t is null || t.Type == JTokenType.Null)
        {
            return null;
        }

        if (t.Type != JTokenType.Integer)
        {
            throw new DapException("`" + name + "` must be an integer");
        }

        return (int)t;
    }

    public static IReadOnlyList<string> Strings(JObject o, string name)
    {
        var t = o[name];
        if (t is null || t.Type == JTokenType.Null)
        {
            return Array.Empty<string>();
        }

        if (t is not JArray a || a.Any(x => x.Type != JTokenType.String))
        {
            throw new DapException("`" + name + "` must be an array of strings");
        }

        return a.Select(x => (string)x!).ToList();
    }

    public static IReadOnlyDictionary<string, string> StringMap(JObject o, string name)
    {
        var t = o[name];
        if (t is null || t.Type == JTokenType.Null)
        {
            return new Dictionary<string, string>();
        }

        if (t is not JObject m)
        {
            throw new DapException("`" + name + "` must be an object of strings");
        }

        var map = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var p in m.Properties())
        {
            map[p.Name] = p.Value.Type == JTokenType.String ? (string)p.Value! : p.Value.ToString();
        }

        return map;
    }
}
