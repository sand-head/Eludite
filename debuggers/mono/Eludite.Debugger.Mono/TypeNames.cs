using System.Globalization;
using System.Text.RegularExpressions;
using ICSharpCode.NRefactory.CSharp;
using Mono.Debugger.Soft;
using Mono.Debugging.Client;
using Mono.Debugging.Evaluation;
using Mono.Debugging.Soft;

namespace Eludite.Debugger.Mono;

/// <summary>
/// The type a simple name means in the stopped frame, in C#'s order (brief 0036), for the evaluator's second pass: a
/// name it did not find as a local, parameter or member (<c>Unknown identifier: Coin</c>) or as a type
/// (<c>Could not resolve type: Coin</c>) is looked up
/// <list type="number">
/// <item>among the nested types of the stopped method's type and of the types enclosing it, innermost first;</item>
/// <item>in each namespace level of the method, innermost first, ending with the global namespace: the level's own
/// types, then its <c>using</c> aliases and the namespaces its <c>using</c> directives import (the file's top-level
/// ones at the global level), read from the source file the debug information names (<see cref="SourceScopes"/>, cached
/// per file); two imports at one level that both have the name make it ambiguous;</item>
/// <item>in <c>System</c>, for a file without <c>using System;</c> (as the pre-pass does);</item>
/// <item>among the types of the loaded assemblies by simple name (the public ones of the debuggee's own assemblies,
/// all of the method's assembly, and the types Mono.Debugging has seen; the runtime's own assemblies are left to the
/// <c>using</c> directives), when exactly one has it; two or more make it ambiguous;</item>
/// <item>as the first segment of a namespace (<c>System</c> in <c>System.Math.Max</c>).</item>
/// </list>
/// An ambiguous name is an <see cref="EvaluatorException"/> naming the candidates: for a condition it becomes the
/// breakpoint's message. Results are cached per scope; misses are forgotten at every stop.
/// </summary>
internal sealed class TypeNames
{
    private static readonly Regex Unknown = new(@"^(?:Unknown identifier|Could not resolve type|Could not load type): (?<name>[A-Za-z_][A-Za-z0-9_]*)$", RegexOptions.CultureInvariant);

    private readonly SoftDebuggerSession _session;
    private readonly Dictionary<string, string?> _found = new(StringComparer.Ordinal);
    private readonly Dictionary<string, AssemblyTypes> _assemblies = new(StringComparer.Ordinal);

    private readonly Action<string> _log;

    public TypeNames(SoftDebuggerSession session, Action<string> log)
    {
        _session = session;
        _log = log;
    }

    /// <summary>The name an evaluator error says it could not find, or null for another error.</summary>
    public static string? UnknownName(string? message)
    {
        var m = message is null ? null : Unknown.Match(message.Trim());
        return m is { Success: true } ? m.Groups["name"].Value : null;
    }

    /// <summary>
    /// <paramref name="expression"/> with <paramref name="name"/> written as the type it means in <paramref name="ctx"/>'s
    /// frame (<c>global::MissingCase.Coin</c>), or null when it means none; throws when it is ambiguous.
    /// </summary>
    public string? Qualify(EvaluationContext ctx, string expression, string name)
    {
        if (ctx is not SoftEvaluationContext soft)
        {
            return null;
        }

        var full = Lookup(soft, name);
        return full is null ? null : Rewrite(expression, name, "global::" + full);
    }

    /// <summary>Forget the names that did not resolve: the debuggee may load their types before the next stop.</summary>
    public void ForgetMisses()
    {
        lock (_found)
        {
            foreach (var k in _found.Where(kv => kv.Value is null).Select(kv => kv.Key).ToList())
            {
                _found.Remove(k);
            }
        }
    }

    /// <summary>The C# name of the type (or namespace) <paramref name="name"/> means in the frame, or null.</summary>
    private string? Lookup(SoftEvaluationContext ctx, string name)
    {
        var frame = ctx.Frame;
        var type = frame.Method.DeclaringType;
        var file = frame.FileName;
        var line = frame.LineNumber;
        var scopes = SourceScopes.ForFile(file);
        var key = type.FullName + "|" + file + "|" + (scopes?.NamespaceAt(line) ?? string.Empty) + "|" + name;
        lock (_found)
        {
            if (_found.TryGetValue(key, out var known))
            {
                return known;
            }
        }

        var clock = System.Diagnostics.Stopwatch.StartNew();
        string? found;
        try
        {
            found = Find(ctx, type, scopes, line, name);
        }
        catch (EvaluatorException e)
        {
            _log(FormattableString.Invariant($"type name `{name}` in {type.FullName}: {e.Message} ({clock.Elapsed.TotalMilliseconds:F1} ms)"));
            throw;
        }

        _log(FormattableString.Invariant($"type name `{name}` in {type.FullName}: {found ?? "none"} ({clock.Elapsed.TotalMilliseconds:F1} ms)"));
        lock (_found)
        {
            _found[key] = found;
        }

        return found;
    }

    private string? Find(SoftEvaluationContext ctx, TypeMirror type, SourceScopes? scopes, int line, string name)
    {
        // 1. Nested types of the method's type and of its enclosing types (`NS.Outer+Inner`, then `NS.Outer`).
        var full = type.FullName;
        var tick = full.IndexOf('[');
        full = tick > 0 ? full.Substring(0, tick) : full;
        for (var t = full; t.Length > 0; t = t.LastIndexOf('+') is var plus && plus > 0 ? t.Substring(0, plus) : string.Empty)
        {
            if (IsType(t + "+" + name, true))
            {
                return CSharpName(t) + "." + name;
            }
        }

        // 2. The namespace levels with their usings.
        var outermost = full.IndexOf('+') is var p && p > 0 ? full.Substring(0, p) : full;
        var levels = new List<SourceScopes.Level>();
        var fileLevels = scopes?.At(line).Levels ?? new[] { new SourceScopes.Level(string.Empty, new SourceScopes.Imports()) };
        for (var ns = Parent(outermost); ; ns = Parent(ns))
        {
            var imports = fileLevels.FirstOrDefault(l => l.Namespace == ns)?.Imports ?? new SourceScopes.Imports();
            levels.Add(new SourceScopes.Level(ns, imports));
            if (ns.Length == 0)
            {
                break;
            }
        }

        foreach (var level in levels)
        {
            var own = level.Namespace.Length == 0 ? name : level.Namespace + "." + name;
            if (IsType(own, true))
            {
                return own;
            }

            if (level.Imports.Aliases.TryGetValue(name, out var alias))
            {
                return alias;
            }

            var imported = level.Imports.Namespaces.Select(u => u + "." + name).Where(c => IsType(c, true)).Distinct(StringComparer.Ordinal).ToList();
            if (imported.Count == 1)
            {
                return imported[0];
            }

            if (imported.Count > 1)
            {
                throw Ambiguous(name, imported);
            }
        }

        // 3. System, as the pre-pass does.
        if (IsType("System." + name, true))
        {
            return "System." + name;
        }

        // 4. A unique simple name among the loaded assemblies' types.
        var matches = LoadedTypes(ctx, type).Where(t => SimpleName(t) == name).Select(CSharpName).Distinct(StringComparer.Ordinal).ToList();
        if (matches.Count == 1)
        {
            return matches[0];
        }

        if (matches.Count > 1)
        {
            throw Ambiguous(name, matches);
        }

        // 5. The first segment of a namespace.
        var root = name + ".";
        if (outermost.StartsWith(root, StringComparison.Ordinal) ||
            name is "System" or "Microsoft" || LoadedTypes(ctx, type).Any(t => t.StartsWith(root, StringComparison.Ordinal)))
        {
            return name;
        }

        return null;
    }

    private static EvaluatorException Ambiguous(string name, IReadOnlyList<string> candidates) =>
        new("'" + name + "' is ambiguous between " + string.Join(" and ", candidates.OrderBy(c => c, StringComparer.Ordinal).Take(4)) + ": qualify it");

    /// <summary>
    /// Whether <paramref name="name"/> is a type in the debuggee: one Mono.Debugging has seen loaded, else (with
    /// <paramref name="askTheDebuggee"/>, so locals cost no round trip in the pre-pass) one the debuggee's loaded
    /// assemblies define.
    /// </summary>
    public bool IsType(string name, bool askTheDebuggee)
    {
        try
        {
            if (_session.GetType(name) is not null)
            {
                return true;
            }

            return askTheDebuggee && _session.VirtualMachine is { } vm && vm.GetTypes(name, false).Count > 0;
        }
#pragma warning disable CA1031 // A name the debuggee cannot look up is not a type.
        catch (Exception)
#pragma warning restore CA1031
        {
            return false;
        }
    }

    /// <summary>
    /// The full names (CLR form, <c>+</c> for nesting) of the loaded assemblies' types: their public ones, all of the
    /// stopped method's own assembly, and every type Mono.Debugging has seen. Each assembly is read once, with Cecil.
    /// </summary>
    private List<string> LoadedTypes(SoftEvaluationContext ctx, TypeMirror type)
    {
        var names = new List<string>();
        var own = Location(type.Assembly);
        AssemblyMirror[] assemblies;
        try
        {
            assemblies = ctx.Domain.GetAssemblies();
        }
#pragma warning disable CA1031 // Without the domain's list, the method's own assembly still counts.
        catch (Exception)
#pragma warning restore CA1031
        {
            assemblies = new[] { type.Assembly };
        }

        // The runtime's own assemblies (under Mono's lib/mono, mscorlib's folder's parent) are left to the `using`
        // directives and `System`: reading their thousands of types would cost more than the budget of a resolution.
        var corlib = Location(ctx.Domain.Corlib);
        var framework = corlib is null ? null : Path.GetDirectoryName(Path.GetDirectoryName(corlib));
        foreach (var a in assemblies)
        {
            var path = Location(a);
            if (path is null || (path != own && framework is not null && path.StartsWith(framework + Path.DirectorySeparatorChar, StringComparison.Ordinal)))
            {
                continue;
            }

            var types = Read(path);
            names.AddRange(path == own ? types.All : types.Public);
        }

        for (var attempt = 0; attempt < 3; attempt++)
        {
            try
            {
                // Mono.Debugging's event thread adds to this table while it is read.
                names.AddRange(_session.GetAllTypes().Select(t => t.FullName).ToList());
                break;
            }
            catch (InvalidOperationException)
            {
            }
        }

        return names;
    }

    private static string? Location(AssemblyMirror a)
    {
        try
        {
            return a.Location;
        }
#pragma warning disable CA1031 // An assembly without a file (dynamic) has no types to read.
        catch (Exception)
#pragma warning restore CA1031
        {
            return null;
        }
    }

    private sealed class AssemblyTypes
    {
        public AssemblyTypes(List<string> visible, List<string> all)
        {
            Public = visible;
            All = all;
        }

        public List<string> Public { get; }

        public List<string> All { get; }
    }

    private AssemblyTypes Read(string path)
    {
        lock (_assemblies)
        {
            if (_assemblies.TryGetValue(path, out var known))
            {
                return known;
            }
        }

        var all = new List<string>();
        var visible = new List<string>();
        try
        {
            var module = global::Mono.Cecil.ModuleDefinition.ReadModule(path);
            void Add(global::Mono.Cecil.TypeDefinition t, bool outerPublic)
            {
                if (t.Name.Length == 0 || t.Name[0] == '<')
                {
                    return;
                }

                var name = t.FullName.Replace('/', '+');
                all.Add(name);
                var isPublic = outerPublic && (t.IsPublic || t.IsNestedPublic);
                if (isPublic)
                {
                    visible.Add(name);
                }

                foreach (var nested in t.NestedTypes)
                {
                    Add(nested, isPublic);
                }
            }

            foreach (var t in module.Types)
            {
                Add(t, true);
            }
        }
#pragma warning disable CA1031 // An assembly Cecil cannot read adds no names.
        catch (Exception)
#pragma warning restore CA1031
        {
        }

        var read = new AssemblyTypes(visible, all);
        lock (_assemblies)
        {
            _assemblies[path] = read;
        }

        return read;
    }

    private static string SimpleName(string clrName)
    {
        var cut = Math.Max(clrName.LastIndexOf('.'), clrName.LastIndexOf('+'));
        var simple = cut >= 0 ? clrName.Substring(cut + 1) : clrName;
        var tick = simple.IndexOf('`');
        return tick > 0 ? simple.Substring(0, tick) : simple;
    }

    /// <summary>A CLR type name as C# writes it: nesting with '.', no arity.</summary>
    private static string CSharpName(string clrName)
    {
        var name = clrName.Replace('+', '.');
        return Regex.Replace(name, "`[0-9]+", string.Empty);
    }

    private static string Parent(string ns)
    {
        var dot = ns.LastIndexOf('.');
        return dot > 0 ? ns.Substring(0, dot) : string.Empty;
    }

    /// <summary>
    /// <paramref name="expression"/> with each use of <paramref name="name"/> as an identifier or a type (a cast, a
    /// <c>typeof</c>, an <c>is</c>) replaced by <paramref name="replacement"/>; null when it does not parse.
    /// </summary>
    public static string? Rewrite(string expression, string name, string replacement)
    {
        var text = expression.Replace("\r", " ").Replace("\n", " ");
        var parser = new CSharpParser();
        var root = parser.ParseExpression(text);
        if (root is null || root.IsNull || parser.HasErrors)
        {
            return null;
        }

        var at = new List<int>();
        foreach (var node in root.DescendantsAndSelf)
        {
            var hit = node switch
            {
                IdentifierExpression id => id.Identifier == name && !id.TypeArguments.Any(),
                SimpleType st => st.Identifier == name && !st.TypeArguments.Any(),
                _ => false,
            };
            if (hit && node.StartLocation.Line == 1)
            {
                at.Add(node.StartLocation.Column - 1);
            }
        }

        if (at.Count == 0)
        {
            return null;
        }

        foreach (var offset in at.Distinct().OrderByDescending(o => o))
        {
            if (offset < 0 || offset + name.Length > text.Length || string.CompareOrdinal(text, offset, name, 0, name.Length) != 0)
            {
                return null;
            }

            text = text.Substring(0, offset) + replacement + text.Substring(offset + name.Length);
        }

        return text;
    }
}
