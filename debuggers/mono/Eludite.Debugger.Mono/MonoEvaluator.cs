using System.Globalization;
using ICSharpCode.NRefactory.CSharp;
using Mono.Debugging.Client;
using Mono.Debugging.Evaluation;
using Mono.Debugging.Soft;

namespace Eludite.Debugger.Mono;

/// <summary>
/// Mono.Debugging's C# evaluator with the gaps of its 2017 build closed (docs/briefs/0022-report.md, brief 0036):
/// <list type="bullet">
/// <item>Integer arithmetic and comparisons fail: the evaluator unboxes both operands as <c>long</c> (or <c>double</c>),
/// so <c>i == 5</c> on an <c>int</c> answers "Value '0' of type System.Int32 cannot be casted to System.Int64" (fixed
/// upstream since by <c>Convert.ToInt64</c>). When an evaluation fails that way, the expression is evaluated again with
/// each numeric operand cast to <c>long</c> or <c>double</c> explicitly, which the evaluator handles, and the result cast
/// back to the type C# gives it (<c>int</c> for two <c>int</c>s). Finding the operands' types evaluates them, so an
/// operand with side effects runs twice in that retry.</item>
/// <item>Type names (<c>Program.Hang()</c>, <c>Coin.Quarter</c>, <c>Math.Max(a, b)</c>) are unknown identifiers without
/// an IDE's type system. Two passes resolve them, the same for <c>evaluate</c>, watches, breakpoint conditions and
/// tracepoint expressions. Mono.Debugging's own pre-pass asks <see cref="ResolveType"/> about every identifier before
/// the evaluation: it answers the types of the frame's enclosing types and namespaces and <c>System</c> (from the source
/// file's declarations when Mono.Debugging gives only the method's name, as it does for a condition), and a namespace's
/// first segment as itself so that <c>System.Math.Max(a, b)</c> evaluates in its <c>global::</c> form. Then, when the
/// evaluator still reports an unknown identifier or type (after the locals, parameters and members, C#'s order), the
/// expression is evaluated again with that name qualified by <see cref="TypeNames"/>: the enclosing types, each
/// namespace level with the file's <c>using</c> directives declared there, <c>System</c>, then a unique simple name
/// among the loaded assemblies' types; an ambiguous name is an error naming the candidates.</item>
/// </list>
/// </summary>
internal sealed class MonoEvaluator : IExpressionEvaluator
{
    private readonly Action<string> _log;

    public MonoEvaluator(SoftDebuggerSession session, Action<string> log)
    {
        _log = log;
        Names = new TypeNames(session, log);
        Evaluator = new NumericEvaluator(Names);
    }

    public ExpressionEvaluator Evaluator { get; }

    /// <summary>The full resolution, after the evaluator's own lookups failed (brief 0036).</summary>
    public TypeNames Names { get; }

    public ObjectValue[] GetLocals(global::Mono.Debugging.Client.StackFrame sf) => sf.GetAllLocals(sf.DebuggerSession.EvaluationOptions);

    /// <summary>
    /// Mono.Debugging's pre-pass: the full name of type <paramref name="identifier"/> as seen from
    /// <paramref name="location"/>, or null. A condition's location names only the method (<c>Cents</c>, not
    /// <c>MissingCase.Coins.Cents</c>): its type and namespace then come from the declarations around the line in the
    /// source file.
    /// </summary>
    public string? ResolveType(string identifier, SourceLocation location)
    {
        if (string.IsNullOrEmpty(identifier) || !char.IsLetter(identifier[0]) && identifier[0] != '_')
        {
            return null;
        }

        var method = location?.MethodName ?? string.Empty;
        var dot = method.LastIndexOf('.');
        var scope = dot > 0 ? method.Substring(0, dot) : ScopeFromFile(location);
        var key = scope + "|" + identifier;
        lock (_resolved)
        {
            if (_resolved.TryGetValue(key, out var known))
            {
                return known;
            }
        }

        var candidates = new List<string>();
        for (var s = scope; s.Length > 0;)
        {
            candidates.Add(s + "." + identifier);
            candidates.Add(s + "+" + identifier);
            var d = s.LastIndexOf('.');
            s = d > 0 ? s.Substring(0, d) : string.Empty;
        }

        candidates.Add(identifier);
        candidates.Add("System." + identifier);
        var clock = System.Diagnostics.Stopwatch.StartNew();
        string? found = candidates.FirstOrDefault(c => Names.IsType(c, char.IsUpper(identifier[0])));
        if (found is not null)
        {
            _log(FormattableString.Invariant($"type name `{identifier}` in {scope}: {found} ({clock.Elapsed.TotalMilliseconds:F1} ms, pre-pass)"));
        }
        if (found is null && IsNamespaceRoot(identifier, scope))
        {
            found = identifier;
        }

        lock (_resolved)
        {
            _resolved[key] = found;
        }

        return found;
    }

    /// <summary>The innermost type (else namespace) declared around the location's line in its source file, or empty.</summary>
    private static string ScopeFromFile(SourceLocation? location)
    {
        if (location is null || location.Line <= 0 || SourceScopes.ForFile(location.FileName) is not { } scopes)
        {
            return string.Empty;
        }

        var (types, _) = scopes.At(location.Line);
        return types.Count > 0 ? types[0] : scopes.NamespaceAt(location.Line);
    }

    /// <summary>Names resolved per scope (breakpoint conditions resolve on Mono.Debugging's thread, hence the lock).</summary>
    private readonly Dictionary<string, string?> _resolved = new(StringComparer.Ordinal);

    /// <summary>Forget the names that did not resolve: the debuggee may load their types before the next stop.</summary>
    public void ForgetMisses()
    {
        lock (_resolved)
        {
            foreach (var k in _resolved.Where(kv => kv.Value is null).Select(kv => kv.Key).ToList())
            {
                _resolved.Remove(k);
            }
        }

        Names.ForgetMisses();
    }

    private static bool IsNamespaceRoot(string identifier, string scope) =>
        identifier is "System" or "Microsoft" || scope == identifier || scope.StartsWith(identifier + ".", StringComparison.Ordinal);

    /// <summary>
    /// The C# evaluator, retrying an expression that hit the numeric cast bug with explicit casts, and one that named
    /// a type the evaluator did not find with that name qualified.
    /// </summary>
    private sealed class NumericEvaluator : NRefactoryExpressionEvaluator
    {
        /// <summary>At most this many names are qualified in one expression.</summary>
        private const int MaxQualified = 8;

        private readonly TypeNames _names;

        public NumericEvaluator(TypeNames names)
        {
            _names = names;
        }

        public override ValueReference Evaluate(EvaluationContext ctx, string expression, object expectedType)
        {
            // NRefactory 5.5 predates C# 6: it reads $"{x}" as the plain string "{x}", a wrong answer, not an error.
            if (InterpolatedString.IsMatch(expression))
            {
                throw new EvaluatorException("interpolated strings are not supported by Mono's evaluator; use string.Format");
            }

            for (var attempt = 0; ; attempt++)
            {
                try
                {
                    return EvaluateWithCasts(ctx, expression, expectedType);
                }
                catch (EvaluatorException e) when (attempt < MaxQualified && TypeNames.UnknownName(e.Message) is not null)
                {
                    var rewritten = _names.Qualify(ctx, expression, TypeNames.UnknownName(e.Message)!);
                    if (rewritten is null || rewritten == expression)
                    {
                        throw;
                    }

                    expression = rewritten;
                }
            }
        }

        private ValueReference EvaluateWithCasts(EvaluationContext ctx, string expression, object expectedType)
        {
            try
            {
                return base.Evaluate(ctx, expression, expectedType);
            }
            catch (EvaluatorException e) when (IsNumericCastBug(e))
            {
                var rewritten = NumericCasts.Rewrite(expression, text => TypeOf(ctx, text));
                if (rewritten is null || rewritten == expression)
                {
                    throw;
                }

                return base.Evaluate(ctx, rewritten, expectedType);
            }
        }

        private static readonly System.Text.RegularExpressions.Regex InterpolatedString = new(@"(\$@?|@\$)""");

        private static bool IsNumericCastBug(EvaluatorException e) =>
            e.Message is { } m && m.Contains("cannot be casted to System.") &&
            (m.EndsWith("System.Int64", StringComparison.Ordinal) || m.EndsWith("System.Double", StringComparison.Ordinal));

        private string? TypeOf(EvaluationContext ctx, string text)
        {
            try
            {
                var v = base.Evaluate(ctx, text, null!);
                return ctx.Adapter.GetValueTypeName(ctx, v.Value);
            }
#pragma warning disable CA1031 // An operand that cannot be evaluated alone is left as it is.
            catch (Exception)
#pragma warning restore CA1031
            {
                return null;
            }
        }
    }
}

/// <summary>The rewrite behind <see cref="MonoEvaluator"/>'s numeric retry, over NRefactory's C# syntax tree.</summary>
internal static class NumericCasts
{
    private static readonly HashSet<string> IntLike = new(StringComparer.Ordinal)
    {
        "System.Int32", "System.Int16", "System.SByte", "System.Byte", "System.UInt16", "System.Char",
        "int", "short", "sbyte", "byte", "ushort", "char",
    };

    private static readonly HashSet<string> Wide = new(StringComparer.Ordinal)
    {
        "System.Int64", "System.UInt32", "long", "uint",
    };

    private static readonly HashSet<string> Floating = new(StringComparer.Ordinal)
    {
        "System.Double", "System.Single", "double", "float",
    };

    private static bool IsFloat(string t) => t is "System.Single" or "float";

    private static readonly HashSet<BinaryOperatorType> Arithmetic = new()
    {
        BinaryOperatorType.Add, BinaryOperatorType.Subtract, BinaryOperatorType.Multiply, BinaryOperatorType.Divide,
        BinaryOperatorType.Modulus, BinaryOperatorType.BitwiseAnd, BinaryOperatorType.BitwiseOr,
        BinaryOperatorType.ExclusiveOr, BinaryOperatorType.ShiftLeft, BinaryOperatorType.ShiftRight,
    };

    private static readonly HashSet<BinaryOperatorType> Comparison = new()
    {
        BinaryOperatorType.Equality, BinaryOperatorType.InEquality, BinaryOperatorType.LessThan,
        BinaryOperatorType.LessThanOrEqual, BinaryOperatorType.GreaterThan, BinaryOperatorType.GreaterThanOrEqual,
    };

    /// <summary>
    /// <paramref name="expression"/> with every numeric binary or unary operation's operands cast to <c>long</c> or
    /// <c>double</c> and its result cast back to <c>int</c> or <c>float</c> where C# would type it so; null when it does
    /// not parse. <paramref name="typeOf"/> names an operand's type (System.Int32, ...) or answers null.
    /// </summary>
    public static string? Rewrite(string expression, Func<string, string?> typeOf)
    {
        var parser = new CSharpParser();
        var root = parser.ParseExpression(expression);
        if (root is null || root.IsNull || parser.HasErrors)
        {
            return null;
        }

        var holder = new ParenthesizedExpression(root);
        var nodes = holder.Descendants
            .Where(n => n is BinaryOperatorExpression b && (Arithmetic.Contains(b.Operator) || Comparison.Contains(b.Operator))
                || n is UnaryOperatorExpression u && u.Operator is UnaryOperatorType.Minus or UnaryOperatorType.Plus or UnaryOperatorType.BitNot)
            .Reverse()
            .ToList();
        foreach (var node in nodes)
        {
            if (node is BinaryOperatorExpression b)
            {
                var lt = typeOf(b.Left.ToString());
                var rt = typeOf(b.Right.ToString());
                if (!IsNumeric(lt) || !IsNumeric(rt))
                {
                    continue;
                }

                var target = Floating.Contains(lt!) || Floating.Contains(rt!) ? "double" : "long";
                CastOperand(b.Left, lt!, target);
                CastOperand(b.Right, rt!, target);
                if (Arithmetic.Contains(b.Operator))
                {
                    var result = IntLike.Contains(lt!) && IntLike.Contains(rt!) ? "int"
                        : (IsFloat(lt!) || IntLike.Contains(lt!)) && (IsFloat(rt!) || IntLike.Contains(rt!)) && (IsFloat(lt!) || IsFloat(rt!)) ? "float"
                        : null;
                    if (result is not null)
                    {
                        b.ReplaceWith(x => new CastExpression(new PrimitiveType(result), new ParenthesizedExpression((Expression)x)));
                    }
                }
            }
            else if (node is UnaryOperatorExpression u)
            {
                var t = typeOf(u.Expression.ToString());
                if (t is null || !IntLike.Contains(t))
                {
                    continue;
                }

                CastOperand(u.Expression, t, "long");
                u.ReplaceWith(x => new CastExpression(new PrimitiveType("int"), new ParenthesizedExpression((Expression)x)));
            }
        }

        return holder.Expression.ToString();
    }

    private static bool IsNumeric(string? t) => t is not null && (IntLike.Contains(t) || Wide.Contains(t) || Floating.Contains(t));

    private static void CastOperand(Expression operand, string type, string target)
    {
        if ((target == "long" && type is "System.Int64" or "long") || (target == "double" && type is "System.Double" or "double"))
        {
            return;
        }

        operand.ReplaceWith(x => new CastExpression(new PrimitiveType(target), new ParenthesizedExpression((Expression)x)));
    }
}
