using ICSharpCode.NRefactory.CSharp;
using Mono.Debugging.Client;
using Mono.Debugging.Evaluation;
using Mono.Debugging.Soft;

namespace Eludite.Debugger.Mono;

/// <summary>
/// Mono.Debugging's C# evaluator with two gaps of its 2017 build closed (docs/briefs/0022-report.md):
/// <list type="bullet">
/// <item>Integer arithmetic and comparisons fail: the evaluator unboxes both operands as <c>long</c> (or <c>double</c>),
/// so <c>i == 5</c> on an <c>int</c> answers "Value '0' of type System.Int32 cannot be casted to System.Int64" (fixed
/// upstream since by <c>Convert.ToInt64</c>). When an evaluation fails that way, the expression is evaluated again with
/// each numeric operand cast to <c>long</c> or <c>double</c> explicitly, which the evaluator handles, and the result cast
/// back to the type C# gives it (<c>int</c> for two <c>int</c>s). Finding the operands' types evaluates them, so an
/// operand with side effects runs twice in that retry.</item>
/// <item>Type names (<c>Program.Hang()</c>, <c>Calculator.Twice(2)</c>) are unknown identifiers without an IDE's type
/// system: <see cref="ResolveType"/> resolves them against the types loaded in the debuggee, from the frame's type
/// outwards through its namespaces.</item>
/// </list>
/// </summary>
internal sealed class MonoEvaluator : IExpressionEvaluator
{
    private readonly SoftDebuggerSession _session;

    public MonoEvaluator(SoftDebuggerSession session)
    {
        _session = session;
    }

    public ExpressionEvaluator Evaluator { get; } = new NumericEvaluator();

    public ObjectValue[] GetLocals(global::Mono.Debugging.Client.StackFrame sf) => sf.GetAllLocals(sf.DebuggerSession.EvaluationOptions);

    /// <summary>The full name of type <paramref name="identifier"/> as seen from <paramref name="location"/>, or null.</summary>
    public string? ResolveType(string identifier, SourceLocation location)
    {
        if (string.IsNullOrEmpty(identifier) || !char.IsLetter(identifier[0]) && identifier[0] != '_')
        {
            return null;
        }

        var candidates = new List<string>();
        var method = location?.MethodName ?? string.Empty;
        var dot = method.LastIndexOf('.');
        var scope = dot > 0 ? method.Substring(0, dot) : string.Empty;
        while (scope.Length > 0)
        {
            candidates.Add(scope + "." + identifier);
            candidates.Add(scope + "+" + identifier);
            var d = scope.LastIndexOf('.');
            scope = d > 0 ? scope.Substring(0, d) : string.Empty;
        }

        candidates.Add(identifier);
        candidates.Add("System." + identifier);
        foreach (var c in candidates)
        {
            try
            {
                if (_session.GetType(c) is not null)
                {
                    return c;
                }
            }
#pragma warning disable CA1031 // An unresolvable name is simply not a type.
            catch (Exception)
#pragma warning restore CA1031
            {
                return null;
            }
        }

        return null;
    }

    /// <summary>The C# evaluator, retrying an expression that hit the numeric cast bug with explicit casts.</summary>
    private sealed class NumericEvaluator : NRefactoryExpressionEvaluator
    {
        public override ValueReference Evaluate(EvaluationContext ctx, string expression, object expectedType)
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
