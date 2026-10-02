using System.Diagnostics;
using System.Globalization;
using System.Net;
using System.Net.Sockets;
using Newtonsoft.Json.Linq;

// The adapter tests time the debugger: run them one at a time.
[assembly: CollectionBehavior(CollectionBehavior.CollectionPerAssembly)]

namespace Eludite.Debugger.Mono.Tests;

/// <summary>
/// <c>mono eludite-dbg-mono.exe</c> debugging the built TestApp over stdio and TCP (brief 0022's proving test). Skipped
/// with a message when no Mono is found. Timings go to the test output for the report.
/// </summary>
public sealed class MonoAdapterTests
{
    private static readonly (string Mono, IReadOnlyDictionary<string, string> Env)? Located = MonoLocator.Locate();
    private static readonly List<double> LaunchToStop = new();

    private readonly ITestOutputHelper _out;

    public MonoAdapterTests(ITestOutputHelper output)
    {
        _out = output;
    }

    private static string Source => Built.TestAppSource;

    private static (string Mono, IReadOnlyDictionary<string, string> Env) RequireMono()
    {
        Assert.SkipWhen(
            Located is null,
            "Mono was not found (ELUDITE_MONO_PREFIX, mono on PATH, ~/.local/opt/mono-root/usr, /usr, /usr/local, " +
            "/Library/Frameworks/Mono.framework/Versions/Current): install mono-devel (Debian, Ubuntu) or mono (Arch) to run the adapter's tests");
        Assert.True(File.Exists(Built.Adapter), Built.Adapter + " is not built");
        Assert.True(File.Exists(Built.TestApp), Built.TestApp + " is not built");
        return Located!.Value;
    }

    private static JObject Bp(string mark, JObject? extra = null)
    {
        var b = new JObject { ["line"] = Built.Line(mark) };
        if (extra is not null)
        {
            b.Merge(extra);
        }

        return b;
    }

    private static JObject SetBreakpoints(DapTestClient c, params JObject[] breakpoints) =>
        c.Body("setBreakpoints", new JObject
        {
            ["source"] = new JObject { ["name"] = "Program.cs", ["path"] = Source },
            ["breakpoints"] = new JArray(breakpoints.Cast<object>().ToArray()),
        });

    /// <summary>initialize, launch, the configuration, configurationDone; the clock started at launch.</summary>
    private static Stopwatch Launch(DapTestClient c, Action<DapTestClient> configure, JObject? launch = null)
    {
        var caps = c.Body("initialize", new JObject
        {
            ["clientID"] = "eludite",
            ["adapterID"] = "mono",
            ["linesStartAt1"] = true,
            ["columnsStartAt1"] = true,
            ["pathFormat"] = "path",
        });
        Assert.True((bool?)caps["supportsConfigurationDoneRequest"]);
        var clock = Stopwatch.StartNew();
        var args = new JObject { ["program"] = Built.TestApp };
        if (launch is not null)
        {
            args.Merge(launch);
        }

        c.Body("launch", args);
        c.WaitEvent("initialized");
        configure(c);
        c.Body("configurationDone");
        return clock;
    }

    private void RecordLaunch(DapTestClient c, Stopwatch clock, string what)
    {
        var ms = clock.Elapsed.TotalMilliseconds;
        lock (LaunchToStop)
        {
            LaunchToStop.Add(ms);
        }

        _out.WriteLine(string.Create(CultureInfo.InvariantCulture, $"timing: launch to the first stopped ({what}, {(c.First ? "the first adapter of this test run" : "warm")}): {ms:F0} ms; from the adapter's spawn: {c.SinceSpawn.Elapsed.TotalMilliseconds:F0} ms"));
        Assert.True(ms < 20_000, "launch to the first stop took " + ms + " ms");
    }

    private static JObject Top(DapTestClient c, long thread) =>
        (JObject)c.Body("stackTrace", new JObject { ["threadId"] = thread, ["startFrame"] = 0, ["levels"] = 20 })["stackFrames"]![0]!;

    private static string Eval(DapTestClient c, int frame, string expression, string context = "watch")
    {
        var body = c.Body("evaluate", new JObject { ["expression"] = expression, ["frameId"] = frame, ["context"] = context });
        return (string)body["result"]!;
    }

    private static Dictionary<string, JObject> Vars(DapTestClient c, int reference, int? start = null, int? count = null)
    {
        var args = new JObject { ["variablesReference"] = reference };
        if (start is not null)
        {
            args["start"] = start;
            args["count"] = count;
        }

        return ((JArray)c.Body("variables", args)["variables"]!).Cast<JObject>().ToDictionary(v => (string)v["name"]!);
    }

    private static int Locals(DapTestClient c, int frame) =>
        (int)c.Body("scopes", new JObject { ["frameId"] = frame })["scopes"]![0]!["variablesReference"]!;

    private static double Percentile(List<double> values, double p)
    {
        var sorted = values.OrderBy(v => v).ToList();
        return sorted[Math.Min(sorted.Count - 1, (int)Math.Ceiling(p * sorted.Count) - 1)];
    }

    [Fact]
    public void Stdio_session_breaks_reads_the_stack_and_values_evaluates_sets_and_steps()
    {
        var (mono, env) = RequireMono();
        using var c = DapTestClient.Stdio(mono, env);
        JObject? asked = null;
        var clock = Launch(c, c => asked = (JObject)SetBreakpoints(c, Bp("add-sum"))["breakpoints"]![0]!);
        var stopped = c.WaitEvent("stopped");
        RecordLaunch(c, clock, "stdio");
        var body = (JObject)stopped["body"]!;
        Assert.Equal("breakpoint", (string?)body["reason"]);
        Assert.True((bool?)body["allThreadsStopped"]);
        var thread = (long)body["threadId"]!;
        // The breakpoint was verified in the answer, or bound by a `breakpoint` event before the stop.
        if ((bool?)asked!["verified"] != true)
        {
            var bound = c.WaitEvent("breakpoint", match: e => (bool?)e["body"]!["breakpoint"]!["verified"] == true);
            Assert.True((int)bound["seq"]! < (int)stopped["seq"]!);
            Assert.Equal((int)asked["id"]!, (int)bound["body"]!["breakpoint"]!["id"]!);
            Assert.Equal(Built.Line("add-sum"), (int)bound["body"]!["breakpoint"]!["line"]!);
        }

        var threads = (JArray)c.Body("threads")["threads"]!;
        Assert.Contains(threads, t => (long)t["id"]! == thread);

        // The stack: the breakpoint's line on top, paging with startFrame.
        var trace = c.Body("stackTrace", new JObject { ["threadId"] = thread });
        var frames = (JArray)trace["stackFrames"]!;
        var total = (int)trace["totalFrames"]!;
        Assert.True(total >= 2);
        Assert.Equal(total, frames.Count);
        Assert.Equal(Built.Line("add-sum"), (int)frames[0]!["line"]!);
        Assert.Equal("Eludite.Debugger.Mono.TestApp.Calculator.Add(int a, int b)", (string)frames[0]!["name"]!);
        Assert.Equal(Source, (string)frames[0]!["source"]!["path"]!);
        var paged = c.Body("stackTrace", new JObject { ["threadId"] = thread, ["startFrame"] = 1, ["levels"] = 1 });
        Assert.Equal(total, (int)paged["totalFrames"]!);
        var main = (JObject)((JArray)paged["stackFrames"]!).Single();
        Assert.StartsWith("Eludite.Debugger.Mono.TestApp.Program.Main(", (string)main["name"]!, StringComparison.Ordinal);
        Assert.Equal(Built.Line("main-add"), (int)main["line"]!);
        var top = (int)frames[0]!["id"]!;
        var mainId = (int)main["id"]!;

        // One Locals scope: this, the parameters, the locals.
        var scopes = (JArray)c.Body("scopes", new JObject { ["frameId"] = top })["scopes"]!;
        Assert.Equal("Locals", (string)Assert.Single(scopes)["name"]!);
        var localsRef = (int)scopes[0]!["variablesReference"]!;
        var locals = ((JArray)c.Body("variables", new JObject { ["variablesReference"] = localsRef })["variables"]!).Cast<JObject>().ToList();
        Assert.Equal(["this", "a", "b", "sum", "doubled"], locals.Select(v => (string)v["name"]!));
        Assert.Equal(["2", "3", "0"], locals.Skip(1).Take(3).Select(v => (string)v["value"]!));
        Assert.Equal("int", (string)locals[1]["type"]!);
        var self = Vars(c, (int)locals[0]["variablesReference"]!);
        Assert.Equal("\"calc\"", (string)self["Name"]["value"]!);
        Assert.Equal("1", (string)self["_calls"]["value"]!);

        // Main's frame: a string[] paged with start and count, an object expanded one level.
        var mainLocals = Vars(c, Locals(c, mainId));
        var names = mainLocals["names"];
        Assert.Equal("{string[25]}", (string)names["value"]!);
        Assert.Equal(25, (int)names["indexedVariables"]!);
        var page = Vars(c, (int)names["variablesReference"]!, start: 2, count: 3);
        Assert.Equal(["[2]", "[3]", "[4]"], page.Keys);
        Assert.Equal("\"name3\"", (string)page["[3]"]["value"]!);
        Assert.Equal("names[3]", (string)page["[3]"]["evaluateName"]!);
        // A long array is listed in Mono.Debugging's ranges, without indexedVariables or evaluate names for them.
        var big = mainLocals["big"];
        Assert.Equal("{int[1000]}", (string)big["value"]!);
        Assert.Null(big["indexedVariables"]);
        var ranges = Vars(c, (int)big["variablesReference"]!);
        Assert.Equal("[0..99]", ranges.Keys.First());
        Assert.Null(ranges["[0..99]"]["evaluateName"]);
        Assert.Equal("0", (string)Vars(c, (int)ranges["[0..99]"]["variablesReference"]!)["[42]"]["value"]!);
        var order = Vars(c, (int)mainLocals["order"]["variablesReference"]!);
        Assert.Equal("\"Contoso\"", (string)order["Customer"]["value"]!);
        Assert.Equal("7", (string)order["Id"]["value"]!);
        Assert.Equal("{string[2]}", (string)order["Tags"]["value"]!);
        Assert.Equal("0", (string)order["Total"]["value"]!);
        Assert.True((int)order["Tags"]["variablesReference"]! > 0);
        Assert.Equal(0, (int)order["Customer"]["variablesReference"]!);

        // evaluate: an identifier, a member chain, method calls, the arithmetic the 2017 evaluator fails on, a type name.
        Assert.Equal("2", Eval(c, top, "a"));
        Assert.Equal("4", Eval(c, top, "this.Name.Length"));
        Assert.Equal("8", Eval(c, top, "Twice(4)"));
        Assert.Equal("8", Eval(c, top, "a + b * 2"));
        Assert.Equal("true", Eval(c, top, "a < b && b == 3"));
        Assert.Equal("\"Contoso #7\"", Eval(c, mainId, "order.Describe()"));
        Assert.Equal("6", Eval(c, mainId, "Calculator.Twice(3)"));
        // Framework types not loaded yet, and namespace-qualified names.
        Assert.Equal("3", Eval(c, top, "Math.Max(a, b)"));
        Assert.Equal("3", Eval(c, top, "System.Math.Max(a, b)"));
        Assert.Equal("true", Eval(c, top, "Environment.NewLine.Length > 0"));
        Assert.Equal("10", Eval(c, top, "Eludite.Debugger.Mono.TestApp.Calculator.Twice(5)"));
        // An interpolated string is refused, not answered as its literal text (NRefactory 5.5 predates C# 6).
        var interpolated = c.Request("evaluate", new JObject { ["expression"] = "$\"{a}\"", ["frameId"] = top, ["context"] = "watch" });
        Assert.False((bool)interpolated["success"]!);
        Assert.Contains("string.Format", (string)interpolated["message"]!, StringComparison.Ordinal);
        Assert.Equal("\"name4\"", Eval(c, mainId, "names[4]"));
        // A failed hover is an error answer and writes nothing.
        var outputs = c.Events("output").Count;
        var hover = c.Request("evaluate", new JObject { ["expression"] = "nosuch", ["frameId"] = top, ["context"] = "hover" });
        Assert.False((bool)hover["success"]!);
        Assert.Contains("nosuch", (string)hover["message"]!, StringComparison.Ordinal);
        Assert.Equal(outputs, c.Events("output").Count);
        // An evaluation that never returns times out with a message naming the timeout, and the session goes on.
        var hang = c.Request("evaluate", new JObject { ["expression"] = "Program.Hang()", ["frameId"] = mainId, ["context"] = "repl", ["timeout"] = 500 });
        Assert.False((bool)hang["success"]!);
        Assert.Contains("500 ms", (string)hang["message"]!, StringComparison.Ordinal);
        Assert.Equal("\"Contoso #7\"", Eval(c, mainId, "order.Describe()"));

        // setVariable: the next variables shows the value.
        var set = c.Body("setVariable", new JObject { ["variablesReference"] = localsRef, ["name"] = "a", ["value"] = "10" });
        Assert.Equal("10", (string)set["value"]!);
        Assert.Equal("10", (string)Vars(c, localsRef)["a"]["value"]!);

        // Steps: next, stepIn, stepOut, each a `step` stop on the expected line.
        c.Body("next", new JObject { ["threadId"] = thread });
        Assert.Equal("step", (string)c.WaitEvent("stopped", 2)["body"]!["reason"]!);
        var after = Top(c, thread);
        Assert.Equal(Built.Line("add-twice"), (int)after["line"]!);
        Assert.Equal("13", Eval(c, (int)after["id"]!, "sum"));
        // References of an earlier stop are gone once the debuggee resumed.
        Assert.False((bool)c.Request("variables", new JObject { ["variablesReference"] = localsRef })["success"]!);
        c.Body("stepIn", new JObject { ["threadId"] = thread });
        Assert.Equal("step", (string)c.WaitEvent("stopped", 3)["body"]!["reason"]!);
        var inside = Top(c, thread);
        Assert.Equal("Eludite.Debugger.Mono.TestApp.Calculator.Twice(int x)", (string)inside["name"]!);
        // Mono stops on the method's opening brace first, as Visual Studio does.
        Assert.Equal(Built.Line("twice") - 1, (int)inside["line"]!);
        c.Body("stepOut", new JObject { ["threadId"] = thread });
        Assert.Equal("step", (string)c.WaitEvent("stopped", 4)["body"]!["reason"]!);
        var back = Top(c, thread);
        Assert.StartsWith("Eludite.Debugger.Mono.TestApp.Calculator.Add(", (string)back["name"]!, StringComparison.Ordinal);
        Assert.Equal(Built.Line("add-twice"), (int)back["line"]!);

        // 50 step-over round trips in the loop (the budget: p95 under 150 ms).
        SetBreakpoints(c, Bp("loop-body"));
        c.Body("continue", new JObject { ["threadId"] = thread });
        Assert.Equal("breakpoint", (string)c.WaitEvent("stopped", 5)["body"]!["reason"]!);
        SetBreakpoints(c);
        var steps = new List<double>();
        for (var i = 0; i < 50; i++)
        {
            var t = Stopwatch.StartNew();
            c.Body("next", new JObject { ["threadId"] = thread });
            var s = c.WaitEvent("stopped", 6 + i);
            steps.Add(t.Elapsed.TotalMilliseconds);
            Assert.Equal("step", (string)s["body"]!["reason"]!);
        }

        _out.WriteLine(string.Create(CultureInfo.InvariantCulture, $"timing: next round trip over {steps.Count} steps: p50 {Percentile(steps, 0.5):F1} ms, p95 {Percentile(steps, 0.95):F1} ms, max {steps.Max():F1} ms"));
        Assert.True(Percentile(steps, 0.95) < 1000, "next p95 " + Percentile(steps, 0.95));
        Assert.StartsWith("Eludite.Debugger.Mono.TestApp.Program.Main(", (string)Top(c, thread)["name"]!, StringComparison.Ordinal);

        // A frame with 200 locals: stackTrace, scopes and variables (the budget: under 100 ms), and the adapter's memory.
        SetBreakpoints(c, Bp("many"));
        c.Body("continue", new JObject { ["threadId"] = thread });
        Assert.Equal("breakpoint", (string)c.WaitEvent("stopped", 56)["body"]!["reason"]!);
        var read = Stopwatch.StartNew();
        var manyTop = (JObject)c.Body("stackTrace", new JObject { ["threadId"] = thread, ["startFrame"] = 0, ["levels"] = 200 })["stackFrames"]![0]!;
        var manyVars = ((JArray)c.Body("variables", new JObject { ["variablesReference"] = Locals(c, (int)manyTop["id"]!) })["variables"]!).Cast<JObject>().ToList();
        var readMs = read.Elapsed.TotalMilliseconds;
        Assert.Equal(201, manyVars.Count);
        Assert.Equal("199", (string)manyVars[199]["value"]!);
        _out.WriteLine(string.Create(CultureInfo.InvariantCulture, $"timing: stackTrace + scopes + variables for a frame with {manyVars.Count} locals: {readMs:F1} ms"));
        if (OperatingSystem.IsLinux())
        {
            var rss = File.ReadAllLines($"/proc/{c.Process.Id}/status").First(x => x.StartsWith("VmRSS:", StringComparison.Ordinal));
            _out.WriteLine("memory: the adapter's resident set at a break: " + rss.Substring(6).Trim());
        }

        c.Body("continue", new JObject { ["threadId"] = thread });
        Assert.Equal(3, (int)c.WaitEvent("exited")["body"]!["exitCode"]!);
        c.WaitEvent("terminated");
        // The program's output, with the value set in the debugger: (10 + 3) * 2.
        var stdout = string.Concat(c.Events("output").Where(e => (string?)e["body"]!["category"] == "stdout").Select(e => (string?)e["body"]!["output"]));
        var stderr = string.Concat(c.Events("output").Where(e => (string?)e["body"]!["category"] == "stderr").Select(e => (string?)e["body"]!["output"]));
        Assert.Contains("result 26", stdout, StringComparison.Ordinal);
        Assert.Contains("done 4950 Contoso #7 25 1000", stdout, StringComparison.Ordinal);
        Assert.Contains("caught boom", stderr, StringComparison.Ordinal);
        c.Body("disconnect", new JObject());
        Assert.True(c.WaitForExit(TimeSpan.FromSeconds(10)), "the adapter exits after disconnect");
    }

    [Fact]
    public void Conditions_hit_conditions_and_log_points()
    {
        var (mono, env) = RequireMono();
        using (var c = DapTestClient.Stdio(mono, env))
        {
            // A condition that skips the first five hits.
            var clock = Launch(c, c => SetBreakpoints(c, Bp("loop-body", new JObject { ["condition"] = "i == 5" })));
            var s = c.WaitEvent("stopped");
            RecordLaunch(c, clock, "conditional breakpoint skipping five hits");
            Assert.Equal("breakpoint", (string)s["body"]!["reason"]!);
            var top = Top(c, (long)s["body"]!["threadId"]!);
            Assert.Equal("5", Eval(c, (int)top["id"]!, "i"));
            c.Body("continue", new JObject { ["threadId"] = (long)s["body"]!["threadId"]! });
            c.WaitEvent("terminated");
            Assert.Single(c.Events("stopped"));
            c.Body("disconnect");
        }

        using (var c = DapTestClient.Stdio(mono, env))
        {
            // Hit condition %2: the 2nd and 4th hits (i = 1 and 3).
            Launch(c, c => SetBreakpoints(c, Bp("loop-body", new JObject { ["hitCondition"] = "%2" })));
            for (var n = 1; n <= 2; n++)
            {
                var s = c.WaitEvent("stopped", n);
                var thread = (long)s["body"]!["threadId"]!;
                Assert.Equal((2 * n - 1).ToString(CultureInfo.InvariantCulture), Eval(c, (int)Top(c, thread)["id"]!, "i"));
                c.Body("continue", new JObject { ["threadId"] = thread });
            }

            c.WaitEvent("stopped", 3);
            c.Body("disconnect", new JObject { ["terminateDebuggee"] = true });
            c.WaitEvent("terminated");
        }

        using (var c = DapTestClient.Stdio(mono, env))
        {
            // A log point writes its text as output and never stops.
            Launch(c, c => SetBreakpoints(c, Bp("loop-body", new JObject { ["logMessage"] = "i is {i}, total {total} {{literal}}" })));
            c.WaitEvent("terminated");
            Assert.Empty(c.Events("stopped"));
            var lines = c.Events("output").Select(e => (string)e["body"]!["output"]!).Where(o => o.StartsWith("i is ", StringComparison.Ordinal)).ToList();
            Assert.Equal(100, lines.Count);
            Assert.Equal("i is 0, total 0 {literal}\n", lines[0]);
            Assert.Equal("i is 3, total 3 {literal}\n", lines[3]);
            c.Body("disconnect");
        }
    }

    [Fact]
    public void Stop_at_entry_stops_in_main_and_just_my_code_steps_over_framework_calls()
    {
        var (mono, env) = RequireMono();
        using var c = DapTestClient.Stdio(mono, env);
        var clock = Launch(c, _ => { }, new JObject { ["stopAtEntry"] = true });
        var s = c.WaitEvent("stopped");
        RecordLaunch(c, clock, "stop at entry");
        Assert.Equal("entry", (string)s["body"]!["reason"]!);
        var thread = (long)s["body"]!["threadId"]!;
        var entry = Top(c, thread);
        Assert.StartsWith("Eludite.Debugger.Mono.TestApp.Program.Main(", (string)entry["name"]!, StringComparison.Ordinal);
        Assert.True((int)entry["line"]! <= Built.Line("entry"), "the entry stop is at or before Main's first line");

        // Just My Code (the default): stepIn on a line that calls only framework code (Console.WriteLine, string
        // concatenation) lands on the next line of Main, not inside mscorlib.
        SetBreakpoints(c, Bp("print-result"));
        c.Body("continue", new JObject { ["threadId"] = thread });
        Assert.Equal("breakpoint", (string)c.WaitEvent("stopped", 2)["body"]!["reason"]!);
        c.Body("stepIn", new JObject { ["threadId"] = thread });
        Assert.Equal("step", (string)c.WaitEvent("stopped", 3)["body"]!["reason"]!);
        var after = Top(c, thread);
        Assert.StartsWith("Eludite.Debugger.Mono.TestApp.Program.Main(", (string)after["name"]!, StringComparison.Ordinal);
        Assert.Equal(Built.Line("total"), (int)after["line"]!);
        c.WaitEvent("output", match: e => (string)e["body"]!["output"]! == "result 10\n");

        SetBreakpoints(c);
        c.Body("continue", new JObject { ["threadId"] = thread });
        Assert.Equal(3, (int)c.WaitEvent("exited")["body"]!["exitCode"]!);
        c.WaitEvent("terminated");
        Assert.Equal(3, c.Events("stopped").Count);
        c.Body("disconnect");
    }

    [Fact]
    public void A_function_breakpoint_stops_in_the_method_named()
    {
        var (mono, env) = RequireMono();
        using var c = DapTestClient.Stdio(mono, env);
        var clock = Launch(c, c =>
        {
            var answer = c.Body("setFunctionBreakpoints", new JObject
            {
                ["breakpoints"] = new JArray(new JObject { ["name"] = "Eludite.Debugger.Mono.TestApp.Calculator.Twice" }),
            });
            Assert.Single((JArray)answer["breakpoints"]!);
        });
        var s = c.WaitEvent("stopped");
        RecordLaunch(c, clock, "function breakpoint");
        Assert.Equal("function breakpoint", (string)s["body"]!["reason"]!);
        var top = Top(c, (long)s["body"]!["threadId"]!);
        Assert.Equal("Eludite.Debugger.Mono.TestApp.Calculator.Twice(int x)", (string)top["name"]!);
        Assert.Equal("5", Eval(c, (int)top["id"]!, "x"));
        c.Body("continue", new JObject { ["threadId"] = (long)s["body"]!["threadId"]! });
        c.WaitEvent("terminated");
        Assert.Single(c.Events("stopped"));
        c.Body("disconnect");
    }

    [Fact]
    public void The_all_exception_filter_stops_at_the_throw_and_without_it_the_throw_does_not_stop()
    {
        var (mono, env) = RequireMono();
        using (var c = DapTestClient.Stdio(mono, env))
        {
            var clock = Launch(c, c => c.Body("setExceptionBreakpoints", new JObject { ["filters"] = new JArray("all", "user-unhandled") }));
            var s = c.WaitEvent("stopped");
            RecordLaunch(c, clock, "first-chance exception");
            var body = (JObject)s["body"]!;
            Assert.Equal("exception", (string)body["reason"]!);
            Assert.Contains("System.InvalidOperationException", (string)body["description"]!, StringComparison.Ordinal);
            Assert.Equal("boom", (string)body["text"]!);
            var thread = (long)body["threadId"]!;
            var top = Top(c, thread);
            Assert.Equal(Built.Line("throw"), (int)top["line"]!);
            var info = c.Body("exceptionInfo", new JObject { ["threadId"] = thread });
            Assert.Equal("System.InvalidOperationException", (string)info["exceptionId"]!);
            Assert.Equal("boom", (string)info["description"]!);
            Assert.Equal("always", (string)info["breakMode"]!);
            Assert.Contains("Program.Fail", (string)info["details"]!["stackTrace"]!, StringComparison.Ordinal);
            c.Body("continue", new JObject { ["threadId"] = thread });
            c.WaitEvent("terminated");
            Assert.Single(c.Events("stopped"));
            c.Body("disconnect");
        }

        using (var c = DapTestClient.Stdio(mono, env))
        {
            Launch(c, c => c.Body("setExceptionBreakpoints", new JObject { ["filters"] = new JArray() }));
            Assert.Equal(3, (int)c.WaitEvent("exited")["body"]!["exitCode"]!);
            c.WaitEvent("terminated");
            Assert.Empty(c.Events("stopped"));
            Assert.Contains(c.Events("output"), e => (string)e["body"]!["output"]! == "caught boom\n");
            c.Body("disconnect");
        }
    }

    [Fact]
    public void Pause_breaks_a_sleeping_program_and_disconnect_ends_it()
    {
        var (mono, env) = RequireMono();
        using var c = DapTestClient.Stdio(mono, env);
        Launch(c, _ => { }, new JObject { ["args"] = new JArray("sleep") });
        c.WaitEvent("output", match: e => ((string)e["body"]!["output"]!).Contains("sleeping", StringComparison.Ordinal));
        var pid = (int)c.WaitEvent("process")["body"]!["systemProcessId"]!;
        c.Body("pause", new JObject { ["threadId"] = 1 });
        var s = c.WaitEvent("stopped");
        Assert.Equal("pause", (string)s["body"]!["reason"]!);
        var trace = c.Body("stackTrace", new JObject { ["threadId"] = (long)s["body"]!["threadId"]! });
        Assert.Contains((JArray)trace["stackFrames"]!, f => ((string)f["name"]!).StartsWith("Eludite.Debugger.Mono.TestApp.Program.Main(", StringComparison.Ordinal) && (int)f["line"]! == Built.Line("sleep"));
        c.Body("disconnect", new JObject { ["terminateDebuggee"] = true });
        c.WaitEvent("terminated");
        Assert.True(c.WaitForExit(TimeSpan.FromSeconds(10)));
        Assert.True(Gone(pid), "the debuggee ended with the session");
    }

    private static bool Gone(int pid)
    {
        try
        {
            using var p = Process.GetProcessById(pid);
            return p.WaitForExit(5000);
        }
        catch (ArgumentException)
        {
            return true;
        }
    }

    [Fact]
    public void Tcp_with_port_zero_serves_a_session()
    {
        var (mono, env) = RequireMono();
        using var c = DapTestClient.Tcp(mono, env);
        var clock = Launch(c, c => SetBreakpoints(c, Bp("add-sum")));
        var s = c.WaitEvent("stopped");
        RecordLaunch(c, clock, "tcp");
        Assert.Equal("breakpoint", (string)s["body"]!["reason"]!);
        Assert.Equal(Built.Line("add-sum"), (int)Top(c, (long)s["body"]!["threadId"]!)["line"]!);
        var unknown = c.Request("restart");
        Assert.False((bool)unknown["success"]!);
        Assert.Contains("restart", (string)unknown["message"]!, StringComparison.Ordinal);
        c.Body("disconnect", new JObject { ["terminateDebuggee"] = true });
        c.WaitEvent("terminated");
        Assert.True(c.WaitForExit(TimeSpan.FromSeconds(10)));
    }

    [Fact]
    public void Attach_to_a_program_waiting_for_the_debugger()
    {
        var (mono, env) = RequireMono();
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        var port = ((IPEndPoint)listener.LocalEndpoint).Port;
        listener.Stop();
        var info = new ProcessStartInfo(mono)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        info.ArgumentList.Add("--debug");
        info.ArgumentList.Add($"--debugger-agent=transport=dt_socket,server=y,suspend=y,address=127.0.0.1:{port}");
        info.ArgumentList.Add(Built.TestApp);
        foreach (var kv in env)
        {
            info.Environment[kv.Key] = kv.Value;
        }

        using var app = Process.Start(info)!;
        try
        {
            using var c = DapTestClient.Stdio(mono, env);
            c.Body("initialize", new JObject { ["adapterID"] = "mono" });
            c.Body("attach", new JObject { ["address"] = "127.0.0.1", ["port"] = port });
            c.WaitEvent("initialized");
            SetBreakpoints(c, Bp("add-sum"));
            c.Body("configurationDone");
            var s = c.WaitEvent("stopped");
            Assert.Equal("breakpoint", (string)s["body"]!["reason"]!);
            var thread = (long)s["body"]!["threadId"]!;
            Assert.Equal("2", Eval(c, (int)Top(c, thread)["id"]!, "a"));
            c.Body("continue", new JObject { ["threadId"] = thread });
            Assert.Equal(3, (int)c.WaitEvent("exited")["body"]!["exitCode"]!);
            c.WaitEvent("terminated");
            c.Body("disconnect");
            Assert.True(app.WaitForExit(10_000));
            Assert.Equal(3, app.ExitCode);
            Assert.Contains("result 10", app.StandardOutput.ReadToEnd(), StringComparison.Ordinal);
        }
        finally
        {
            if (!app.HasExited)
            {
                app.Kill();
            }
        }
    }

    /// <summary>
    /// A detach (brief 0027 found the adapter spinning at about 90% of a core after it instead of exiting): the program
    /// stopped at a breakpoint runs on, the adapter answers, sends <c>terminated</c> without <c>exited</c> and exits
    /// with 0 within 2 s, spending almost no CPU time from the answer to its exit.
    /// </summary>
    [Fact]
    public void Disconnect_detaches_and_the_adapter_exits_promptly_while_the_program_runs_on() =>
        DetachAndExit(c =>
        {
            var answer = c.Request("disconnect", new JObject { ["terminateDebuggee"] = false });
            Assert.True((bool?)answer["success"] == true, "disconnect failed: " + answer["message"]);
            c.WaitEvent("terminated");
        });

    /// <summary>The client closing the channel during an attached session detaches the same way.</summary>
    [Fact]
    public void Closing_the_channel_detaches_and_the_adapter_exits_promptly_while_the_program_runs_on() =>
        DetachAndExit(c => c.CloseInput());

    private void DetachAndExit(Action<DapTestClient> detach)
    {
        var (mono, env) = RequireMono();
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        var port = ((IPEndPoint)listener.LocalEndpoint).Port;
        listener.Stop();
        var info = new ProcessStartInfo(mono)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        info.ArgumentList.Add("--debug");
        info.ArgumentList.Add($"--debugger-agent=transport=dt_socket,server=y,suspend=y,address=127.0.0.1:{port}");
        info.ArgumentList.Add(Built.TestApp);
        info.ArgumentList.Add("sleep");
        foreach (var kv in env)
        {
            info.Environment[kv.Key] = kv.Value;
        }

        using var app = Process.Start(info)!;
        try
        {
            using var c = DapTestClient.Stdio(mono, env);
            c.Body("initialize", new JObject { ["adapterID"] = "mono" });
            c.Body("attach", new JObject { ["address"] = "127.0.0.1", ["port"] = port });
            c.WaitEvent("initialized");
            SetBreakpoints(c, Bp("sleep-print"));
            c.Body("configurationDone");
            Assert.Equal("breakpoint", (string)c.WaitEvent("stopped")["body"]!["reason"]!);

            detach(c);
            var clock = Stopwatch.StartNew();
            var atAnswer = Cpu(c.Process);
            var last = atAnswer;
            while (!c.Process.HasExited && clock.Elapsed < TimeSpan.FromSeconds(2))
            {
                last = Cpu(c.Process) ?? last;
                Thread.Sleep(10);
            }

            var exited = c.WaitForExit(TimeSpan.FromMilliseconds(Math.Max(0, 2000 - clock.ElapsedMilliseconds)));
            var took = clock.Elapsed;
            var spent = atAnswer is { } a && last is { } l ? l - a : TimeSpan.Zero;
            _out.WriteLine(string.Create(CultureInfo.InvariantCulture, $"timing: detach to the adapter's exit: {took.TotalMilliseconds:F0} ms; its CPU time meanwhile: {spent.TotalMilliseconds:F0} ms"));
            Assert.True(exited, "the adapter did not exit within 2 s of the detach\n" + c.Stderr);
            Assert.Equal(0, c.Process.ExitCode);
            Assert.True(spent < TimeSpan.FromMilliseconds(100), "the adapter spent " + spent.TotalMilliseconds + " ms of CPU time after the detach");
            Assert.Empty(c.Events("exited"));

            // The program left at its breakpoint runs on: it prints and sleeps, still alive after the adapter is gone.
            var line = app.StandardOutput.ReadLineAsync();
            Assert.True(line.Wait(TimeSpan.FromSeconds(10)), "the program did not run on after the detach");
            Assert.Equal("sleeping", line.Result);
            Assert.False(app.WaitForExit(500), "the program ended with the detach");
        }
        finally
        {
            if (!app.HasExited)
            {
                app.Kill();
            }
        }
    }

    /// <summary>The process's CPU time, or null once it exited.</summary>
    private static TimeSpan? Cpu(Process p)
    {
        try
        {
            p.Refresh();
            return p.HasExited ? null : p.TotalProcessorTime;
        }
        catch (InvalidOperationException)
        {
            return null;
        }
        catch (System.ComponentModel.Win32Exception)
        {
            return null;
        }
    }
}
