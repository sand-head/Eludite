using System.Text;

namespace Eludite.Debugger.Mono;

/// <summary>
/// The namespaces, <c>using</c> directives and type declarations of one C# source file, by line (brief 0036): what an
/// expression typed at a line of that file sees as type names. Read by a small scanner rather than a C# parser, so that
/// syntax newer than the evaluator's NRefactory 5.5 (file-scoped namespaces, records, raw strings) does not stop it:
/// comments, strings and character literals are skipped, braces are counted, and only these are recognized:
/// <c>namespace A.B {</c>, <c>namespace A.B;</c>, <c>using A.B;</c>, <c>using X = A.B;</c>, <c>global using A.B;</c>
/// (<c>using static</c> is ignored: it imports members, not types) and <c>class</c>, <c>struct</c>, <c>interface</c>,
/// <c>enum</c> and <c>record</c> declarations with a body.
/// </summary>
internal sealed class SourceScopes
{
    private readonly List<Region> _namespaces = new();
    private readonly List<Region> _types = new();
    private readonly Imports _file = new();

    /// <summary>A namespace's or a type's lines and name (a type's name is its full name, nested types joined by '.').</summary>
    private sealed class Region
    {
        public Region(string name, int start, int depth)
        {
            Name = name;
            Start = start;
            Depth = depth;
        }

        public string Name { get; }

        public int Start { get; }

        public int End { get; set; } = int.MaxValue;

        public int Depth { get; }

        public Imports Imports { get; } = new();

        public bool Contains(int line) => line >= Start && line <= End;
    }

    /// <summary>The namespaces a <c>using</c> level imports, and its aliases.</summary>
    internal sealed class Imports
    {
        public List<string> Namespaces { get; } = new();

        public Dictionary<string, string> Aliases { get; } = new(StringComparer.Ordinal);
    }

    /// <summary>One namespace level of a lookup, innermost first: the namespace (empty: the global one) and its usings.</summary>
    internal sealed class Level
    {
        public Level(string ns, Imports imports)
        {
            Namespace = ns;
            Imports = imports;
        }

        public string Namespace { get; }

        public Imports Imports { get; }
    }

    /// <summary>
    /// What a name at <paramref name="line"/> is looked up in: the enclosing types, innermost first, then the namespace
    /// levels, innermost first, ending with the global namespace and the file's top-level <c>using</c> directives.
    /// </summary>
    public (IReadOnlyList<string> Types, IReadOnlyList<Level> Levels) At(int line)
    {
        var types = _types.Where(t => t.Contains(line)).OrderByDescending(t => t.Depth).ThenByDescending(t => t.Start).Select(t => t.Name).ToList();
        var levels = new List<Level>();
        foreach (var ns in _namespaces.Where(n => n.Contains(line)).OrderByDescending(n => n.Depth).ThenByDescending(n => n.Start))
        {
            levels.Add(new Level(ns.Name, ns.Imports));
            // `namespace A.B` is the levels A.B and A: the parent part has no usings of its own.
            for (var name = Parent(ns.Name); name.Length > 0 && !_namespaces.Any(o => o.Name == name && o.Contains(line)); name = Parent(name))
            {
                levels.Add(new Level(name, new Imports()));
            }
        }

        levels.Add(new Level(string.Empty, _file));
        return (types, levels);
    }

    /// <summary>The namespace declared around <paramref name="line"/> (the innermost), or empty.</summary>
    public string NamespaceAt(int line) =>
        _namespaces.Where(n => n.Contains(line)).OrderByDescending(n => n.Depth).ThenByDescending(n => n.Start).Select(n => n.Name).FirstOrDefault() ?? string.Empty;

    private static string Parent(string name)
    {
        var dot = name.LastIndexOf('.');
        return dot > 0 ? name.Substring(0, dot) : string.Empty;
    }

    private static readonly Dictionary<string, (SourceScopes Scopes, DateTime Written)> Cache = new(StringComparer.Ordinal);

    /// <summary>The scopes of the file at <paramref name="path"/>, read once per version of the file; null when it cannot be read.</summary>
    public static SourceScopes? ForFile(string? path)
    {
        if (string.IsNullOrEmpty(path))
        {
            return null;
        }

        try
        {
            var written = File.GetLastWriteTimeUtc(path);
            lock (Cache)
            {
                if (Cache.TryGetValue(path!, out var known) && known.Written == written)
                {
                    return known.Scopes;
                }
            }

            var scopes = Parse(File.ReadAllText(path));
            lock (Cache)
            {
                Cache[path!] = (scopes, written);
            }

            return scopes;
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or ArgumentException or NotSupportedException)
        {
            return null;
        }
    }

    // ----- The scanner -----

    private enum Kind
    {
        Word,
        Punct,
    }

    private readonly struct Token
    {
        public Token(Kind kind, string text, int line)
        {
            Kind = kind;
            Text = text;
            Line = line;
        }

        public Kind Kind { get; }

        public string Text { get; }

        public int Line { get; }
    }

    private enum Frame
    {
        File,
        Namespace,
        Type,
        Block,
    }

    /// <summary>Read <paramref name="text"/>'s namespaces, usings and types.</summary>
    public static SourceScopes Parse(string text)
    {
        var s = new SourceScopes();
        var tokens = Tokens(text).ToList();
        // The open frames: what each `{` opened (a file-scoped namespace opens no brace and closes at the end).
        var frames = new Stack<(Frame Kind, Region? Region)>();
        frames.Push((Frame.File, null));
        Region? fileScoped = null;
        string? pendingType = null;
        var pendingNamespace = (string?)null;
        var parens = 0;

        Region? CurrentNamespace() => frames.Where(f => f.Kind == Frame.Namespace).Select(f => f.Region).FirstOrDefault() ?? fileScoped;
        Region? CurrentType() => frames.Where(f => f.Kind == Frame.Type).Select(f => f.Region).FirstOrDefault();
        int Depth() => frames.Count + (fileScoped is null ? 0 : 1);

        for (var i = 0; i < tokens.Count; i++)
        {
            var t = tokens[i];
            var top = frames.Peek().Kind;
            if (t.Kind == Kind.Punct)
            {
                switch (t.Text)
                {
                    case "(":
                        parens++;
                        break;
                    case ")":
                        parens = Math.Max(0, parens - 1);
                        break;
                    case ";" when parens == 0:
                        if (pendingNamespace is not null)
                        {
                            // `namespace A.B;`: the rest of the file.
                            fileScoped = new Region(Join(CurrentNamespace()?.Name, pendingNamespace), t.Line, Depth());
                            s._namespaces.Add(fileScoped);
                            pendingNamespace = null;
                        }

                        pendingType = null;
                        break;
                    case "{":
                        if (pendingNamespace is not null)
                        {
                            var ns = new Region(Join(CurrentNamespace()?.Name, pendingNamespace), t.Line, Depth() + 1);
                            s._namespaces.Add(ns);
                            frames.Push((Frame.Namespace, ns));
                            pendingNamespace = null;
                        }
                        else if (pendingType is not null && parens == 0)
                        {
                            var outer = CurrentType()?.Name ?? CurrentNamespace()?.Name;
                            var type = new Region(Join(outer, pendingType), t.Line, Depth() + 1);
                            s._types.Add(type);
                            frames.Push((Frame.Type, type));
                            pendingType = null;
                        }
                        else
                        {
                            frames.Push((Frame.Block, null));
                        }

                        break;
                    case "}":
                        if (frames.Count > 1)
                        {
                            var (_, region) = frames.Pop();
                            if (region is not null)
                            {
                                region.End = t.Line;
                            }
                        }

                        break;
                }

                continue;
            }

            // Words: only declarations outside method bodies matter.
            if (top == Frame.Block)
            {
                continue;
            }

            switch (t.Text)
            {
                case "namespace" when top != Frame.Type && i + 1 < tokens.Count && tokens[i + 1].Kind == Kind.Word:
                    pendingNamespace = QualifiedName(tokens, ref i);
                    break;
                case "using" when top != Frame.Type && i + 1 < tokens.Count && tokens[i + 1].Kind == Kind.Word:
                    ReadUsing(tokens, ref i, CurrentNamespace()?.Imports ?? s._file, global: i > 0 && tokens[i - 1].Text == "global", s._file);
                    break;
                case "class" or "struct" or "interface" or "enum" or "record" when pendingType is null:
                    var n = i + 1;
                    if (t.Text == "record" && n < tokens.Count && tokens[n].Text is "class" or "struct")
                    {
                        n++;
                    }

                    if (n < tokens.Count && tokens[n].Kind == Kind.Word && !IsKeyword(tokens[n].Text))
                    {
                        pendingType = tokens[n].Text;
                        i = n;
                    }

                    break;
            }
        }

        return s;
    }

    private static string Join(string? outer, string name) => string.IsNullOrEmpty(outer) ? name : outer + "." + name;

    private static bool IsKeyword(string word) => word is "where" or "new" or "class" or "struct" or "unmanaged" or "notnull";

    /// <summary>A dotted name starting after <paramref name="i"/> (the keyword); <paramref name="i"/> ends on its last part.</summary>
    private static string QualifiedName(List<Token> tokens, ref int i)
    {
        var b = new StringBuilder();
        var j = i + 1;
        if (j + 1 < tokens.Count && tokens[j].Text == "global" && tokens[j + 1].Text == "::")
        {
            j += 2;
        }

        while (j < tokens.Count && tokens[j].Kind == Kind.Word)
        {
            b.Append(tokens[j].Text);
            if (j + 1 < tokens.Count && tokens[j + 1].Text == "." && j + 2 < tokens.Count && tokens[j + 2].Kind == Kind.Word)
            {
                b.Append('.');
                j += 2;
                continue;
            }

            break;
        }

        i = j;
        return b.ToString();
    }

    /// <summary>
    /// A <c>using</c> directive at <paramref name="i"/>: a namespace or an alias goes to <paramref name="into"/> (a
    /// <c>global using</c> to the file's); <c>using static</c> and a <c>using</c> statement are skipped.
    /// </summary>
    private static void ReadUsing(List<Token> tokens, ref int i, Imports into, bool global, Imports file)
    {
        if (tokens[i + 1].Text == "static")
        {
            return;
        }

        var start = i;
        var first = QualifiedName(tokens, ref i);
        if (i + 1 < tokens.Count && tokens[i + 1].Text == "=" && !first.Contains('.'))
        {
            i++;
            var target = QualifiedName(tokens, ref i);
            if (i + 1 < tokens.Count && tokens[i + 1].Text == ";" && target.Length > 0)
            {
                (global ? file : into).Aliases[first] = target;
                i++;
            }

            return;
        }

        if (i + 1 < tokens.Count && tokens[i + 1].Text == ";" && first.Length > 0)
        {
            var list = (global ? file : into).Namespaces;
            if (!list.Contains(first))
            {
                list.Add(first);
            }

            i++;
            return;
        }

        i = start;
    }

    /// <summary>Words and the punctuation the scanner needs, with their 1-based lines; comments, strings and preprocessor lines skipped.</summary>
    private static IEnumerable<Token> Tokens(string text)
    {
        var line = 1;
        var atLineStart = true;
        for (var i = 0; i < text.Length;)
        {
            var c = text[i];
            if (c == '\n')
            {
                line++;
                i++;
                atLineStart = true;
                continue;
            }

            if (char.IsWhiteSpace(c))
            {
                i++;
                continue;
            }

            if (c == '#' && atLineStart)
            {
                while (i < text.Length && text[i] != '\n')
                {
                    i++;
                }

                continue;
            }

            atLineStart = false;
            if (c == '/' && i + 1 < text.Length && text[i + 1] == '/')
            {
                while (i < text.Length && text[i] != '\n')
                {
                    i++;
                }

                continue;
            }

            if (c == '/' && i + 1 < text.Length && text[i + 1] == '*')
            {
                i += 2;
                while (i < text.Length && !(text[i] == '*' && i + 1 < text.Length && text[i + 1] == '/'))
                {
                    if (text[i] == '\n')
                    {
                        line++;
                    }

                    i++;
                }

                i += 2;
                continue;
            }

            if (c == '\'')
            {
                i++;
                while (i < text.Length && text[i] != '\'' && text[i] != '\n')
                {
                    i += text[i] == '\\' ? 2 : 1;
                }

                i++;
                continue;
            }

            if (c is '"' or '$' or '@' && StringStart(text, i) is int quote)
            {
                i = SkipString(text, i, quote, ref line);
                continue;
            }

            if (char.IsLetter(c) || c == '_')
            {
                var start = i;
                while (i < text.Length && (char.IsLetterOrDigit(text[i]) || text[i] == '_'))
                {
                    i++;
                }

                yield return new Token(Kind.Word, text.Substring(start, i - start), line);
                continue;
            }

            if (c == '@' && i + 1 < text.Length && (char.IsLetter(text[i + 1]) || text[i + 1] == '_'))
            {
                i++;
                continue;
            }

            if (c == ':' && i + 1 < text.Length && text[i + 1] == ':')
            {
                yield return new Token(Kind.Punct, "::", line);
                i += 2;
                continue;
            }

            if (c is '{' or '}' or ';' or '(' or ')' or '=' or '.')
            {
                // `=>`, `==` and the like are not `=`.
                if (c == '=' && i + 1 < text.Length && text[i + 1] is '=' or '>')
                {
                    i += 2;
                    continue;
                }

                yield return new Token(Kind.Punct, c.ToString(), line);
            }

            i++;
        }
    }

    /// <summary>The index of the opening quote of a string literal starting at <paramref name="i"/> (after <c>$</c>/<c>@</c> prefixes), or null.</summary>
    private static int? StringStart(string text, int i)
    {
        var j = i;
        while (j < text.Length && text[j] is '$' or '@')
        {
            j++;
        }

        return j < text.Length && text[j] == '"' ? j : null;
    }

    /// <summary>Past the string literal whose prefix starts at <paramref name="i"/> and whose quote is at <paramref name="quote"/>.</summary>
    private static int SkipString(string text, int i, int quote, ref int line)
    {
        var prefix = text.Substring(i, quote - i);
        var interpolated = prefix.Contains('$');
        var verbatim = prefix.Contains('@');
        var quotes = 0;
        while (quote + quotes < text.Length && text[quote + quotes] == '"')
        {
            quotes++;
        }

        int j;
        if (quotes >= 3)
        {
            // A raw string literal: ends at the same run of quotes.
            j = quote + quotes;
            var close = new string('"', quotes);
            var end = text.IndexOf(close, j, StringComparison.Ordinal);
            end = end < 0 ? text.Length : end + quotes;
            line += Count(text, j, end, '\n');
            return end;
        }

        if (quotes == 2 && !verbatim)
        {
            return quote + 2; // ""
        }

        j = quote + 1;
        var depth = 0;
        while (j < text.Length)
        {
            var c = text[j];
            if (c == '\n')
            {
                line++;
                if (!verbatim && depth == 0)
                {
                    return j;
                }
            }

            if (depth == 0)
            {
                if (!verbatim && c == '\\')
                {
                    j += 2;
                    continue;
                }

                if (c == '"')
                {
                    if (verbatim && j + 1 < text.Length && text[j + 1] == '"')
                    {
                        j += 2;
                        continue;
                    }

                    return j + 1;
                }

                if (interpolated && c == '{')
                {
                    if (j + 1 < text.Length && text[j + 1] == '{')
                    {
                        j += 2;
                        continue;
                    }

                    depth = 1;
                }
            }
            else if (c is '"' or '$' or '@' && StringStart(text, j) is int inner)
            {
                j = SkipString(text, j, inner, ref line);
                continue;
            }
            else if (c == '{')
            {
                depth++;
            }
            else if (c == '}')
            {
                depth--;
            }

            j++;
        }

        return j;
    }

    private static int Count(string text, int from, int to, char c)
    {
        var n = 0;
        for (var k = from; k < to && k < text.Length; k++)
        {
            if (text[k] == c)
            {
                n++;
            }
        }

        return n;
    }
}
