using System.Diagnostics;
using System.Globalization;
using System.Text.RegularExpressions;
using Newtonsoft.Json.Linq;

namespace Eludite.Debugger.Mono.Tests;

/// <summary>
/// Brief 0036 against the real adapter: type names in conditions, tracepoints and evaluations resolve as Visual Studio
/// resolves them (the method's namespace, its enclosing types, the file's <c>using</c> directives, a unique simple
/// name), an ambiguous one is reported, and breakpoints inserted while the debuggee loads types all bind.
/// </summary>
public sealed partial class MonoAdapterTests
{
    private static JObject LaunchArgs(string mode) => new() { ["args"] = new JArray(mode) };

    private static string EvalError(DapTestClient c, int frame, string expression)
    {
        var r = c.Request("evaluate", new JObject { ["expression"] = expression, ["frameId"] = frame, ["context"] = "watch" });
        Assert.False((bool)r["success"]!, expression + " evaluated: " + r["body"]);
        return (string)r["message"]!;
    }

    private static double Timed(Action a)
    {
        var t = Stopwatch.StartNew();
        a();
        return t.Elapsed.TotalMilliseconds;
    }

    [Fact]
    public void An_unqualified_enum_name_resolves_in_a_condition_a_tracepoint_and_evaluations()
    {
        var (mono, env) = RequireMono();
        using var c = DapTestClient.Stdio(mono, env);
        // `Coin` is MissingCase-style: an enum of the stopped method's namespace, named without it.
        var clock = Launch(
            c,
            c => SetBreakpoints(
                c,
                Bp("coin-loop", new JObject { ["condition"] = "coin == Coin.Quarter" }),
                Bp("coin-default", new JObject { ["logMessage"] = "default for {coin}: quarter {coin == Coin.Quarter}" })),
            LaunchArgs("coins"));
        var s = c.WaitEvent("stopped");
        RecordLaunch(c, clock, "a condition with an unqualified enum name");
        Assert.Equal("breakpoint", (string)s["body"]!["reason"]!);
        var thread = (long)s["body"]!["threadId"]!;
        var top = Top(c, thread);
        Assert.Equal(Built.Line("coin-loop"), (int)top["line"]!);
        Assert.StartsWith("Eludite.Debugger.Mono.TestApp.Purse.Coins.Run(", (string)top["name"]!, StringComparison.Ordinal);
        var frame = (int)top["id"]!;
        // The third coin of the purse is the quarter.
        Assert.Equal("2", Eval(c, frame, "i"));
        Assert.Contains("Quarter", Eval(c, frame, "coin"), StringComparison.Ordinal);
        Assert.DoesNotContain(c.Events("breakpoint"), e => (bool?)e["body"]!["breakpoint"]!["verified"] == false && e["body"]!["breakpoint"]!["message"] is not null && ((string)e["body"]!["breakpoint"]!["message"]!).Contains("identifier", StringComparison.Ordinal));

        // Evaluations: the namespace's own type, a type a `using` imports, one found by its unique simple name.
        Assert.Contains("Quarter", Eval(c, frame, "Coin.Quarter"), StringComparison.Ordinal);
        Assert.Equal("true", Eval(c, frame, "coin == Coin.Quarter"));
        // The cost of a resolution: the first use of a name against a second, both evaluated warm.
        Eval(c, frame, "Eludite.Debugger.Mono.TestApp.Shapes.Shape.Square");
        var cold = Timed(() => Assert.Contains("Circle", Eval(c, frame, "Shape.Circle"), StringComparison.Ordinal));
        var warm = Timed(() => Assert.Contains("Square", Eval(c, frame, "Shape.Square"), StringComparison.Ordinal));
        Assert.Contains("Blue", Eval(c, frame, "Hue.Blue"), StringComparison.Ordinal);
        Assert.Equal("3", Eval(c, frame, "Gadget.Size"));
        Assert.Equal("3", Eval(c, frame, "(int)Coin.Quarter"));
        // `Kind` is in both imported namespaces: ambiguous, as the C# compiler says.
        var ambiguous = EvalError(c, frame, "Kind.Warm");
        Assert.Contains("ambiguous", ambiguous, StringComparison.Ordinal);
        Assert.Contains("Eludite.Debugger.Mono.TestApp.Colors.Kind", ambiguous, StringComparison.Ordinal);
        Assert.Contains("Eludite.Debugger.Mono.TestApp.Shapes.Kind", ambiguous, StringComparison.Ordinal);
        // A name that is no type stays unknown.
        Assert.Contains("nosuch", EvalError(c, frame, "nosuch.Value"), StringComparison.Ordinal);
        // The budget: resolving a name adds under 20 ms to its first evaluation. The adapter logs each resolution with
        // its time: the condition's `Coin` (the method's namespace, Mono.Debugging's pre-pass) and `Shape` (a using
        // directive, after the evaluator's own lookups failed).
        double Resolution(string name)
        {
            var m = Regex.Match(c.Stderr, "type name `" + name + "` in Eludite\\.Debugger\\.Mono\\.TestApp\\.Purse\\.Coins: [^(]*\\((?<ms>[0-9.]+) ms");
            Assert.True(m.Success, "the adapter logs the resolution of " + name + "\n" + c.Stderr);
            return double.Parse(m.Groups["ms"].Value, CultureInfo.InvariantCulture);
        }

        var conditionMs = Resolution("Coin");
        var usingMs = Resolution("Shape");
        _out.WriteLine(string.Create(CultureInfo.InvariantCulture, $"timing: resolving `Coin` for the condition {conditionMs:F1} ms; `Shape` through a using directive {usingMs:F1} ms; evaluate `Shape.Circle` {cold:F1} ms (its first evaluation failing, the lookup, the evaluation again), then `Shape.Square` {warm:F1} ms"));
        // 20 ms on a quiet machine.
        Budget.Assert("resolving the condition's type name", conditionMs, 50);
        Budget.Assert("resolving a type name through a using directive", usingMs, 50);

        // The tracepoint's expression resolves the same way.
        c.Body("continue", new JObject { ["threadId"] = thread });
        Assert.Equal(16, (int)c.WaitEvent("exited")["body"]!["exitCode"]!);
        c.WaitEvent("terminated");
        var lines = c.Events("output").Select(e => (string)e["body"]!["output"]!).Where(o => o.StartsWith("default for ", StringComparison.Ordinal)).ToList();
        Assert.Equal(new[] { "default for Eludite.Debugger.Mono.TestApp.Purse.Coin.Quarter: quarter true\n" }, lines);
        Assert.Single(c.Events("stopped"));
        c.Body("disconnect");
    }

    [Fact]
    public void A_condition_naming_an_ambiguous_type_is_reported_on_the_breakpoint()
    {
        var (mono, env) = RequireMono();
        using var c = DapTestClient.Stdio(mono, env);
        var id = 0;
        Launch(
            c,
            c => id = (int)SetBreakpoints(c, Bp("coin-loop", new JObject { ["condition"] = "Kind.Warm == Kind.Cold" }))["breakpoints"]![0]!["id"]!,
            LaunchArgs("coins"));
        // The condition fails at the first hit: the breakpoint is reported unbound with the evaluator's reason, and the
        // program runs to its end.
        var failed = c.WaitEvent("breakpoint", match: e => (bool?)e["body"]!["breakpoint"]!["verified"] == false && e["body"]!["breakpoint"]!["message"] is not null && ((string)e["body"]!["breakpoint"]!["message"]!).Contains("ambiguous", StringComparison.Ordinal));
        Assert.Equal(id, (int)failed["body"]!["breakpoint"]!["id"]!);
        Assert.Contains("Eludite.Debugger.Mono.TestApp.Colors.Kind", (string)failed["body"]!["breakpoint"]!["message"]!, StringComparison.Ordinal);
        Assert.Equal(16, (int)c.WaitEvent("exited")["body"]!["exitCode"]!);
        c.WaitEvent("terminated");
        Assert.Empty(c.Events("stopped"));
        c.Body("disconnect");
    }

    /// <summary>
    /// Brief 0036's race: twenty breakpoints (log points) sent one more at a time while the program loads this file's
    /// types and six framework assemblies. Mono.Debugging inserts each on its operation thread from tables its event
    /// thread fills as types load; before the fix an insertion could fail with "Collection was modified" and its
    /// breakpoint never bound. Ten runs, every breakpoint bound in each.
    /// </summary>
    [Fact]
    public void Breakpoints_sent_while_types_and_assemblies_load_all_bind_in_ten_runs()
    {
        var (mono, env) = RequireMono();
        var parts = Enumerable.Range(1, 20).Select(n => "part-" + n.ToString("00", CultureInfo.InvariantCulture)).ToList();
        for (var run = 1; run <= 10; run++)
        {
            using var c = DapTestClient.Stdio(mono, env);
            JObject Send(int count) => SetBreakpoints(
                c,
                new[] { Bp("load-done") }.Concat(parts.Take(count).Select(p => Bp(p, new JObject { ["logMessage"] = p }))).ToArray());
            Launch(c, c => Send(0), LaunchArgs("load"));
            c.WaitEvent("output", match: e => (string?)e["body"]!["output"] == "loading\n");
            JObject answer = new();
            for (var k = 1; k <= parts.Count; k++)
            {
                answer = Send(k);
                Thread.Sleep(10);
            }

            var s = c.WaitEvent("stopped");
            Assert.Equal(Built.Line("load-done"), (int)Top(c, (long)s["body"]!["threadId"]!)["line"]!);
            var final = ((JArray)answer["breakpoints"]!).Cast<JObject>().ToList();
            Assert.Equal(21, final.Count);
            // Bound in the answer, or by a `breakpoint` event since (one the library missed is inserted again at the stop).
            List<int> Unbound()
            {
                var bound = c.Events("breakpoint")
                    .Select(e => (JObject)e["body"]!["breakpoint"]!)
                    .Where(b => (bool?)b["verified"] == true)
                    .Select(b => (int)b["id"]!)
                    .ToHashSet();
                return final.Where(b => (bool?)b["verified"] != true && !bound.Contains((int)b["id"]!)).Select(b => (int)b["line"]!).ToList();
            }

            var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(3);
            while (Unbound().Count > 0 && DateTime.UtcNow < deadline)
            {
                Thread.Sleep(20);
            }

            var unbound = Unbound();
            var console = string.Concat(c.Events("output").Where(e => (string?)e["body"]!["category"] == "console").Select(e => (string?)e["body"]!["output"]));
            Assert.True(unbound.Count == 0, "run " + run + ": breakpoints never bound on lines " + string.Join(", ", unbound) + "\n" + console);
            Assert.DoesNotContain("Could not set breakpoint", console, StringComparison.Ordinal);
            var again = c.Stderr.Split('\n').Count(l => l.Contains("inserting it again", StringComparison.Ordinal) || l.Contains("inserting again a pending breakpoint", StringComparison.Ordinal));
            _out.WriteLine(string.Create(CultureInfo.InvariantCulture, $"run {run}: all 21 bound; insertions redone after the race: {again}"));
            c.Body("continue", new JObject { ["threadId"] = (long)s["body"]!["threadId"]! });
            c.WaitEvent("terminated");
            c.Body("disconnect");
        }
    }
}
