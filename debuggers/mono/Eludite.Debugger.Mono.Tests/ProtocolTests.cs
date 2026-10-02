using System.Text;
using Eludite.Debugger.Mono.Protocol;
using Newtonsoft.Json.Linq;

namespace Eludite.Debugger.Mono.Tests;

/// <summary>The protocol library without Mono: framing, the dispatcher, argument parsing, hit conditions, log messages and the handle tables.</summary>
public sealed class ProtocolTests
{
    [Fact]
    public void Framing_round_trips_messages_with_multibyte_text_and_extra_headers()
    {
        var a = new JObject { ["seq"] = 1, ["type"] = "request", ["command"] = "evaluate", ["arguments"] = new JObject { ["expression"] = "\"héllo ✓\"" } };
        var b = new JObject { ["seq"] = 2, ["type"] = "event", ["event"] = "output", ["body"] = new JObject { ["output"] = new string('x', 100_000) } };
        using var stream = new MemoryStream();
        var first = Framing.Encode(a);
        Assert.StartsWith("Content-Length: " + (first.Length - first.AsSpan().IndexOf("{"u8)), Encoding.ASCII.GetString(first), StringComparison.Ordinal);
        stream.Write(first);
        // Another header field is allowed and ignored.
        stream.Write(Encoding.ASCII.GetBytes("Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n"));
        stream.Write(Framing.Encode(b));
        stream.Position = 0;
        var reader = new FrameReader(stream);
        Assert.True(JToken.DeepEquals(a, reader.Read()));
        Assert.True(JToken.DeepEquals(b, reader.Read()));
        Assert.Null(reader.Read());
    }

    [Fact]
    public void Framing_rejects_a_message_without_content_length()
    {
        using var stream = new MemoryStream(Encoding.ASCII.GetBytes("X-Other: 1\r\n\r\n{}"));
        Assert.Throws<InvalidDataException>(() => new FrameReader(stream).Read());
    }

    private static List<JObject> Written(MemoryStream output)
    {
        output.Position = 0;
        var reader = new FrameReader(output);
        var all = new List<JObject>();
        while (reader.Read() is { } m)
        {
            all.Add(m);
        }

        return all;
    }

    [Fact]
    public void The_dispatcher_answers_unknown_requests_and_failing_handlers_with_failures_never_crashing()
    {
        using var output = new MemoryStream();
        var log = new List<string>();
        using var d = new Dispatcher(new MemoryStream(), output, log.Add);
        d.Register("threads", _ => new JObject { ["threads"] = new JArray() });
        d.Register("boom", _ => throw new InvalidOperationException("kaput"));
        d.Register("refuse", _ => throw new DapException("not now"));
        d.Dispatch(JObject.Parse("""{"seq": 7, "type": "request", "command": "gotoTargets", "arguments": {}}"""));
        d.Dispatch(JObject.Parse("""{"seq": 8, "type": "request", "command": "threads"}"""));
        d.Dispatch(JObject.Parse("""{"seq": 9, "type": "request", "command": "boom"}"""));
        d.Dispatch(JObject.Parse("""{"seq": 10, "type": "request", "command": "refuse"}"""));
        d.Dispatch(JObject.Parse("""{"seq": 11, "type": "event", "event": "nonsense"}"""));
        d.SendEvent("initialized");
        var m = Written(output);
        Assert.Equal(5, m.Count);
        Assert.Equal(7, (int)m[0]["request_seq"]!);
        Assert.False((bool)m[0]["success"]!);
        Assert.Contains("gotoTargets", (string)m[0]["message"]!, StringComparison.Ordinal);
        Assert.True((bool)m[1]["success"]!);
        Assert.Equal("threads", (string)m[1]["command"]!);
        Assert.Empty((JArray)m[1]["body"]!["threads"]!);
        Assert.False((bool)m[2]["success"]!);
        Assert.Contains("kaput", (string)m[2]["message"]!, StringComparison.Ordinal);
        Assert.Equal("not now", (string)m[3]["message"]!);
        Assert.Equal("initialized", (string)m[4]["event"]!);
        // Sequence numbers grow across responses and events.
        Assert.Equal(Enumerable.Range(1, 5), m.Select(x => (int)x["seq"]!));
    }

    [Fact]
    public void The_dispatcher_runs_posted_work_and_requests_on_one_thread_in_order()
    {
        using var output = new MemoryStream();
        var input = new MemoryStream();
        input.Write(Framing.Encode(JObject.Parse("""{"seq": 1, "type": "request", "command": "a"}""")));
        input.Write(Framing.Encode(JObject.Parse("""{"seq": 2, "type": "request", "command": "a"}""")));
        input.Position = 0;
        using var d = new Dispatcher(input, output, _ => { });
        var threads = new HashSet<int>();
        var closed = false;
        d.Register("a", _ =>
        {
            threads.Add(Environment.CurrentManagedThreadId);
            d.Post(() => threads.Add(Environment.CurrentManagedThreadId));
            return null;
        });
        d.Closed += () => closed = true;
        // The input ends after two requests: the dispatcher stops by itself.
        var runner = new Thread(d.Run);
        runner.Start();
        Assert.True(runner.Join(TimeSpan.FromSeconds(10)));
        Assert.True(closed);
        Assert.Single(threads);
        Assert.Equal(runner.ManagedThreadId, threads.Single());
        Assert.Equal(2, Written(output).Count);
    }

    [Fact]
    public void Launch_arguments_parse_with_defaults_and_resolve_paths()
    {
        var root = Path.GetFullPath(Path.Combine(Path.GetTempPath(), "eludite-launch"));
        var a = LaunchArguments.Parse(JObject.Parse("""{"program": "bin/App.exe"}"""), root);
        Assert.Equal(Path.Combine(root, "bin", "App.exe"), a.Program);
        Assert.Equal(Path.Combine(root, "bin"), a.Cwd);
        Assert.Empty(a.Args);
        Assert.Empty(a.Env);
        Assert.Null(a.RuntimeExecutable);
        Assert.Empty(a.RuntimeArgs);
        Assert.False(a.StopAtEntry);
        Assert.True(a.JustMyCode);

        var b = LaunchArguments.Parse(JObject.Parse("""
            {"program": "App.exe", "args": ["--x", "two words"], "cwd": "work", "env": {"A": "1", "N": 2},
             "runtimeExecutable": "/opt/mono/bin/mono", "runtimeArgs": ["--gc=sgen"], "stopAtEntry": true, "justMyCode": false}
            """), root);
        Assert.Equal(Path.Combine(root, "work", "App.exe"), b.Program);
        Assert.Equal(Path.Combine(root, "work"), b.Cwd);
        Assert.Equal(["--x", "two words"], b.Args);
        Assert.Equal("1", b.Env["A"]);
        Assert.Equal("2", b.Env["N"]);
        Assert.Equal("/opt/mono/bin/mono", b.RuntimeExecutable);
        Assert.Equal(["--gc=sgen"], b.RuntimeArgs);
        Assert.True(b.StopAtEntry);
        Assert.False(b.JustMyCode);

        Assert.Contains("program", Assert.Throws<DapException>(() => LaunchArguments.Parse(new JObject(), root)).Message, StringComparison.Ordinal);
        Assert.Contains("args", Assert.Throws<DapException>(() => LaunchArguments.Parse(JObject.Parse("""{"program": "a.exe", "args": "x"}"""), root)).Message, StringComparison.Ordinal);
        Assert.Contains("stopAtEntry", Assert.Throws<DapException>(() => LaunchArguments.Parse(JObject.Parse("""{"program": "a.exe", "stopAtEntry": "yes"}"""), root)).Message, StringComparison.Ordinal);

        var attach = AttachArguments.Parse(JObject.Parse("""{"port": 55555}"""));
        Assert.Equal(("127.0.0.1", 55555), (attach.Address, attach.Port));
        Assert.Equal("winbox", AttachArguments.Parse(JObject.Parse("""{"port": 1, "address": "winbox"}""")).Address);
        Assert.Throws<DapException>(() => AttachArguments.Parse(new JObject()));
        Assert.Throws<DapException>(() => AttachArguments.Parse(JObject.Parse("""{"port": 70000}""")));
    }

    [Fact]
    public void Hit_conditions_parse_as_the_shell_sends_them()
    {
        static HitCondition P(string s) => HitCondition.TryParse(s, out var h) ? h : throw new InvalidOperationException(s);
        Assert.Equal(new HitCondition(HitConditionKind.EqualTo, 3), P("3"));
        Assert.Equal(new HitCondition(HitConditionKind.EqualTo, 3), P(" ==3 "));
        Assert.Equal(new HitCondition(HitConditionKind.GreaterThanOrEqualTo, 2), P(">=2"));
        Assert.Equal(new HitCondition(HitConditionKind.MultipleOf, 2), P("%2"));
        Assert.Equal(new HitCondition(HitConditionKind.GreaterThan, 4), P("> 4"));
        Assert.Equal(new HitCondition(HitConditionKind.LessThanOrEqualTo, 5), P("<=5"));
        foreach (var bad in new[] { "", "0", "%0", "x", ">=", "-1", "2.5" })
        {
            Assert.False(HitCondition.TryParse(bad, out _), bad);
        }

        Assert.Equal([2, 4, 6], Enumerable.Range(1, 6).Where(P("%2").BreaksOn));
        Assert.Equal([3], Enumerable.Range(1, 6).Where(P("3").BreaksOn));
        Assert.Equal([2, 3, 4], Enumerable.Range(1, 4).Where(P(">=2").BreaksOn));
    }

    [Fact]
    public void Log_messages_interpolate_expressions_and_keep_escaped_braces()
    {
        var evaluated = new List<string>();
        string Eval(string e)
        {
            evaluated.Add(e);
            return e == "i" ? "5" : "\"" + e + "\"";
        }

        Assert.Equal("i is 5, name \"order.Name\" {literal}", LogMessage.Interpolate("i is {i}, name { order.Name } {{literal}}", Eval));
        Assert.Equal(["i", "order.Name"], evaluated);
        Assert.Equal("no expressions", LogMessage.Interpolate("no expressions", Eval));
        Assert.Equal("open { brace", LogMessage.Interpolate("open { brace", Eval));
        Assert.Equal(
            [new LogSegment("a=", false), new LogSegment("a", true), new LogSegment(" {b}", false)],
            LogMessage.Parse("a={a} {{b}}").Select(s => s).ToList(),
            new SegmentComparer());
        // Mono.Debugging's trace syntax: expressions in braces, a literal opening brace doubled.
        Assert.Equal("i is {i}, {{x} done", LogMessage.ToTraceExpression("i is {i}, {{x}} done"));
    }

    private sealed class SegmentComparer : IEqualityComparer<LogSegment>
    {
        public bool Equals(LogSegment x, LogSegment y) => x.Text == y.Text && x.IsExpression == y.IsExpression;

        public int GetHashCode(LogSegment obj) => obj.Text.GetHashCode(StringComparison.Ordinal);
    }

    [Fact]
    public void The_variables_table_pages_with_start_and_count_and_is_cleared_on_resume()
    {
        var table = new HandleTable<object>();
        var items = Enumerable.Range(0, 25).Select(i => "name" + i).ToList();
        var r = table.Add(items);
        Assert.Equal(1, r);
        Assert.Same(items, table.Get(r));
        Assert.Equal(["name2", "name3", "name4"], Paging.Page(items, 2, 3));
        Assert.Equal(items, Paging.Page(items, null, null));
        Assert.Equal(["name24"], Paging.Page(items, 24, 10));
        Assert.Empty(Paging.Page(items, 30, 5));
        Assert.Equal(items.Skip(20), Paging.Page(items, 20, 0));
        Assert.Equal((0, 25), Paging.Window(25, -3, null));
        // stackTrace's startFrame and levels use the same window.
        Assert.Equal((1, 1), Paging.Window(2, 1, 20));

        var second = table.Add("x");
        Assert.Equal(2, table.Count);
        // The debuggee resumed: every reference of the stop is gone, and numbers are not reused.
        table.Clear();
        Assert.Null(table.Get(r));
        Assert.Null(table.Get(second));
        Assert.Equal(0, table.Count);
        Assert.Equal(3, table.Add("y"));
    }

    [Fact]
    public void Exception_filters_parse_types_and_reject_unknown_filters()
    {
        var none = ExceptionSettings.Parse(JObject.Parse("""{"filters": []}"""));
        Assert.False(none.BreakWhenThrown);
        Assert.False(none.BreakWhenUserUnhandled);
        var all = ExceptionSettings.Parse(JObject.Parse("""{"filters": ["all", "user-unhandled"]}"""));
        Assert.Equal(["System.Exception"], all.ThrownTypes);
        Assert.True(all.BreakWhenUserUnhandled);
        Assert.Empty(all.UnhandledTypes);
        var typed = ExceptionSettings.Parse(JObject.Parse("""
            {"filters": [], "filterOptions": [
              {"filterId": "all", "condition": "System.InvalidOperationException, System.IO.IOException"},
              {"filterId": "user-unhandled", "condition": "System.ArgumentException"}]}
            """));
        Assert.Equal(["System.InvalidOperationException", "System.IO.IOException"], typed.ThrownTypes);
        Assert.Equal(["System.ArgumentException"], typed.UnhandledTypes);
        Assert.Contains("uncaught", Assert.Throws<DapException>(() => ExceptionSettings.Parse(JObject.Parse("""{"filters": ["uncaught"]}"""))).Message, StringComparison.Ordinal);
    }

    [Fact]
    public void Options_capabilities_and_command_lines()
    {
        Assert.Null(AdapterOptions.Parse([]).Port);
        var o = AdapterOptions.Parse(["--port", "0", "--log", "/tmp/a.log"]);
        Assert.Equal(0, o.Port);
        Assert.Equal("/tmp/a.log", o.LogFile);
        Assert.Throws<DapException>(() => AdapterOptions.Parse(["--port"]));
        Assert.Throws<DapException>(() => AdapterOptions.Parse(["--port", "x"]));
        Assert.Throws<DapException>(() => AdapterOptions.Parse(["--stdio"]));

        var caps = Capabilities.Initialize();
        foreach (var name in new[]
        {
            "supportsConfigurationDoneRequest", "supportsConditionalBreakpoints", "supportsHitConditionalBreakpoints",
            "supportsFunctionBreakpoints", "supportsLogPoints", "supportsEvaluateForHovers", "supportsSetVariable",
            "supportsExceptionInfoRequest", "supportsExceptionFilterOptions", "supportsTerminateRequest",
            "supportsDelayedStackTraceLoading",
        })
        {
            Assert.True((bool?)caps[name], name);
        }

        Assert.Equal(["all", "user-unhandled"], caps["exceptionBreakpointFilters"]!.Select(f => (string)f["filter"]!));
        Assert.True((bool)caps["exceptionBreakpointFilters"]![1]!["default"]!);

        Assert.Equal("a \"two words\" \"q\\\"x\" \"\" \"back\\\\slash\"", CommandLine.Join(["a", "two words", "q\"x", "", "back\\slash"]));
    }
}
