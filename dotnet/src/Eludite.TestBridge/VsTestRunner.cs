using System.Globalization;
using System.Text.Json;

namespace Eludite.TestBridge;

/// <summary>
/// VSTest's translation-layer protocol (host-rpc.md, "Tests"), for projects that have not moved to
/// Microsoft.Testing.Platform: the runner listens on a loopback port and starts
/// <c>dotnet &lt;vstest.console.dll&gt; --port:&lt;port&gt; --parentprocessid:&lt;pid&gt;</c>, which connects and says
/// <c>TestSession.Connected</c>; after <c>ProtocolVersion</c>, a discovery is <c>TestDiscovery.Start</c> answered by
/// <c>TestDiscovery.TestFound</c> batches and <c>TestDiscovery.Completed</c>, and a run is
/// <c>TestExecution.RunAllWithDefaultHost</c> (the container's assembly) or <c>TestExecution.RunSelectedWithDefaultHost</c>
/// (the discovered TestCase objects), answered by <c>TestExecution.StatsChange</c> and <c>TestExecution.Completed</c>.
/// A debug run asks for <c>TestExecution.GetTestRunnerProcessStartInfoForRunAll</c> (or <c>...ForRunSelected</c>) with
/// debugging: vstest.console starts the testhost paused and sends <c>TestExecution.EditorAttachDebugger2</c>, answered with
/// <c>TestExecution.EditorAttachDebuggerCallback</c> once the sink attached. One vstest.console serves one request and
/// ends with <c>TestSession.Terminate</c>.
/// </summary>
public sealed class VsTestRunner : ITestRunner
{
    private static readonly TimeSpan ConnectTimeout = TimeSpan.FromSeconds(30);

    public async Task DiscoverAsync(TestContainer container, ITestSink sink, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(container);
        ArgumentNullException.ThrowIfNull(sink);
        await using var session = await Session.StartAsync(container, sink, cancellationToken).ConfigureAwait(false);
        await session.Connection.SendAsync("TestDiscovery.Start", new Dictionary<string, object?>
        {
            ["Sources"] = new[] { container.Program },
            ["RunSettings"] = RunSettings(container),
        }).ConfigureAwait(false);
        var canceled = false;
        using var registration = cancellationToken.Register(() =>
        {
            canceled = true;
            _ = session.Connection.SendAsync("TestDiscovery.Cancel", null).ContinueWith(t => _ = t.Exception, TaskScheduler.Default);
            session.KillAfterGrace();
        });
        while (true)
        {
            var (type, payload) = await ReceiveAsync(session, () => canceled, cancellationToken).ConfigureAwait(false);
            switch (type)
            {
                case "TestDiscovery.TestFound":
                    Found(payload, sink);
                    break;
                case "TestSession.Message":
                    Message(payload, sink);
                    break;
                case "TestDiscovery.Completed":
                    if (payload.ValueKind == JsonValueKind.Object && payload.TryGetProperty("LastDiscoveredTests", out var last))
                    {
                        Found(last, sink);
                    }

                    if (canceled)
                    {
                        throw new OperationCanceledException(cancellationToken);
                    }

                    if (payload.ValueKind == JsonValueKind.Object && payload.TryGetProperty("IsAborted", out var aborted) && aborted.ValueKind == JsonValueKind.True)
                    {
                        throw new TestRunnerException("vstest.console aborted the discovery");
                    }

                    await session.TerminateAsync().ConfigureAwait(false);
                    return;
            }
        }
    }

    public async Task RunAsync(TestContainer container, IReadOnlyList<DiscoveredTest>? tests, bool debug, ITestSink sink, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(container);
        ArgumentNullException.ThrowIfNull(sink);
        await using var session = await Session.StartAsync(container, sink, cancellationToken).ConfigureAwait(false);
        var payload = new Dictionary<string, object?>
        {
            ["RunSettings"] = RunSettings(container),
            ["KeepAlive"] = false,
            ["DebuggingEnabled"] = debug,
        };
        if (tests is null)
        {
            payload["Sources"] = new[] { container.Program };
        }
        else
        {
            payload["TestCases"] = tests.Select(t => t.Native).ToList();
        }

        var message = (debug, tests is null) switch
        {
            (true, true) => "TestExecution.GetTestRunnerProcessStartInfoForRunAll",
            (true, false) => "TestExecution.GetTestRunnerProcessStartInfoForRunSelected",
            (false, true) => "TestExecution.RunAllWithDefaultHost",
            (false, false) => "TestExecution.RunSelectedWithDefaultHost",
        };
        await session.Connection.SendAsync(message, payload).ConfigureAwait(false);
        var canceled = false;
        using var registration = cancellationToken.Register(() =>
        {
            canceled = true;
            _ = session.Connection.SendAsync("TestExecution.Cancel", null).ContinueWith(t => _ = t.Exception, TaskScheduler.Default);
            session.KillAfterGrace();
        });
        var done = new HashSet<string>(StringComparer.Ordinal);
        while (true)
        {
            var (type, body) = await ReceiveAsync(session, () => canceled, cancellationToken).ConfigureAwait(false);
            switch (type)
            {
                case "TestExecution.StatsChange":
                    Changed(body, sink, done);
                    break;
                case "TestSession.Message":
                    Message(body, sink);
                    break;
                case "TestExecution.EditorAttachDebugger2":
                case "TestExecution.EditorAttachDebugger":
                    {
                        var pid = body.ValueKind == JsonValueKind.Object && body.TryGetProperty("ProcessID", out var p) && p.TryGetInt32(out var id) ? id : 0;
                        var (attached, why) = pid > 0
                            ? await sink.AttachAsync(pid, cancellationToken).ConfigureAwait(false)
                            : (false, "vstest.console named no process");
                        await session.Connection.SendAsync("TestExecution.EditorAttachDebuggerCallback", new Dictionary<string, object?>
                        {
                            ["Attached"] = attached,
                            ["ErrorMessage"] = why,
                        }).ConfigureAwait(false);
                        break;
                    }

                case "TestExecution.CustomTestHostLaunch":
                    {
                        // Sent only to a launcher that cannot attach: the start info goes to the shell as a launch.
                        sink.Launch(StartInfo(body, container));
                        await session.Connection.SendAsync("TestExecution.CustomTestHostLaunchCallback", new Dictionary<string, object?>
                        {
                            ["HostProcessId"] = -1,
                            ["ErrorMessage"] = "Eludite launches the testhost under its debug adapter",
                        }).ConfigureAwait(false);
                        break;
                    }

                case "TestExecution.Completed":
                    if (body.ValueKind == JsonValueKind.Object && body.TryGetProperty("LastRunTests", out var last) && last.ValueKind == JsonValueKind.Object)
                    {
                        Changed(last, sink, done);
                    }

                    if (canceled)
                    {
                        throw new OperationCanceledException(cancellationToken);
                    }

                    if (body.ValueKind == JsonValueKind.Object
                        && body.TryGetProperty("TestRunCompleteArgs", out var args)
                        && args.ValueKind == JsonValueKind.Object)
                    {
                        var aborted = args.TryGetProperty("IsAborted", out var a) && a.ValueKind == JsonValueKind.True;
                        var error = args.TryGetProperty("Error", out var e) && e.ValueKind == JsonValueKind.Object && e.TryGetProperty("Message", out var em) ? em.GetString() : null;
                        if (aborted || error is not null)
                        {
                            throw new TestRunnerException(error ?? "vstest.console aborted the run");
                        }
                    }

                    await session.TerminateAsync().ConfigureAwait(false);
                    return;
            }
        }
    }

    /// <summary>The next message; a connection that closes after a cancel is the cancel.</summary>
    private static async Task<(string Type, JsonElement Payload)> ReceiveAsync(Session session, Func<bool> canceled, CancellationToken cancellationToken)
    {
        try
        {
            return await session.Connection.ReceiveAsync(CancellationToken.None).ConfigureAwait(false);
        }
        catch (TestRunnerException) when (canceled())
        {
            throw new OperationCanceledException(cancellationToken);
        }
    }

    /// <summary>The RunSettings sent with every request: the file's contents, or the parallel setting alone.</summary>
    public static string RunSettings(TestContainer container)
    {
        ArgumentNullException.ThrowIfNull(container);
        if (container.RunSettings is { Length: > 0 } path && File.Exists(path))
        {
            return File.ReadAllText(path);
        }

        var cpus = container.Parallel ? 0 : 1;
        return $"<RunSettings><RunConfiguration><MaxCpuCount>{cpus}</MaxCpuCount><DesignMode>True</DesignMode></RunConfiguration></RunSettings>";
    }

    private static void Found(JsonElement cases, ITestSink sink)
    {
        if (cases.ValueKind != JsonValueKind.Array)
        {
            return;
        }

        var tests = cases.EnumerateArray().Where(c => c.ValueKind == JsonValueKind.Object).Select(c => new DiscoveredTest(Item(c), c.Clone())).ToList();
        if (tests.Count > 0)
        {
            sink.Tests(tests);
        }
    }

    private static void Message(JsonElement payload, ITestSink sink)
    {
        if (payload.ValueKind == JsonValueKind.Object && payload.TryGetProperty("Message", out var m) && m.GetString() is { } text)
        {
            foreach (var line in text.Replace("\r\n", "\n", StringComparison.Ordinal).Split('\n'))
            {
                sink.Log(line);
            }
        }
    }

    /// <summary>A TestRunChangedEventArgs: new results, then the active tests not reported yet as running.</summary>
    private static void Changed(JsonElement args, ITestSink sink, HashSet<string> done)
    {
        if (args.ValueKind != JsonValueKind.Object)
        {
            return;
        }

        var results = new List<TestResult>();
        if (args.TryGetProperty("NewTestResults", out var news) && news.ValueKind == JsonValueKind.Array)
        {
            foreach (var r in news.EnumerateArray())
            {
                var result = Result(r);
                if (result is not null)
                {
                    done.Add(result.Id);
                    results.Add(result);
                }
            }
        }

        if (args.TryGetProperty("ActiveTests", out var active) && active.ValueKind == JsonValueKind.Array)
        {
            foreach (var c in active.EnumerateArray())
            {
                var item = Item(c);
                if (!done.Contains(item.Id))
                {
                    results.Add(new TestResult(item.Id, TestOutcomes.Running) { DisplayName = item.DisplayName, FullyQualifiedName = item.FullyQualifiedName });
                }
            }
        }

        if (results.Count > 0)
        {
            sink.Results(results);
        }
    }

    public static TestItem Item(JsonElement testCase)
    {
        var id = Str(testCase, "Id") ?? string.Empty;
        var fqn = Str(testCase, "FullyQualifiedName") ?? id;
        var display = Str(testCase, "DisplayName") ?? fqn;
        string? type = null;
        string? method = null;
        List<TestTrait>? traits = null;
        string? nunitClass = null;
        string? nunitMethod = null;
        if (testCase.TryGetProperty("Properties", out var properties) && properties.ValueKind == JsonValueKind.Array)
        {
            foreach (var p in properties.EnumerateArray())
            {
                if (!p.TryGetProperty("Key", out var key) || !p.TryGetProperty("Value", out var value))
                {
                    continue;
                }

                switch (Str(key, "Id"))
                {
                    case "TestCase.ManagedType":
                        type = value.ValueKind == JsonValueKind.String ? value.GetString() : null;
                        break;
                    case "TestCase.ManagedMethod":
                        method = value.ValueKind == JsonValueKind.String ? value.GetString() : null;
                        break;
                    case "NUnit.ClassName" when value.ValueKind == JsonValueKind.String:
                        nunitClass = value.GetString();
                        break;
                    case "NUnit.MethodName" when value.ValueKind == JsonValueKind.String:
                        nunitMethod = value.GetString();
                        break;
                    case "NUnit.TestCategory" or "MSTestDiscoverer.TestCategory" or "MSTest.TestCategory" when value.ValueKind == JsonValueKind.Array:
                        traits ??= [];
                        traits.AddRange(value.EnumerateArray()
                            .Where(v => v.ValueKind == JsonValueKind.String)
                            .Select(v => new TestTrait("Category", v.GetString() ?? string.Empty)));
                        break;
                    case "TestObject.Traits" when value.ValueKind == JsonValueKind.Array:
                        traits ??= [];
                        traits.AddRange(value.EnumerateArray()
                            .Where(t => t.ValueKind == JsonValueKind.Object)
                            .Select(t => new TestTrait(Str(t, "Key") ?? string.Empty, Str(t, "Value") ?? string.Empty)));
                        break;
                }
            }
        }

        // NUnit names the class and method in properties of its own.
        type ??= nunitClass;
        method ??= nunitMethod;
        traits = traits?.Distinct().ToList();
        string? ns;
        string? cls;
        string? m;
        if (type is not null)
        {
            (ns, cls, m) = TestNames.Split(type, method);
        }
        else
        {
            // No managed names: split the fully qualified name before any argument list.
            var bare = TestNames.StripParameters(fqn) ?? fqn;
            var dot = bare.LastIndexOf('.');
            (ns, cls, m) = dot < 0 ? (null, null, bare) : TestNames.Split(bare[..dot], bare[(dot + 1)..]);
        }

        return new TestItem(id, display, TestNames.FullName(type, method, fqn))
        {
            Namespace = ns,
            ClassName = cls,
            Method = m,
            Source = Str(testCase, "CodeFilePath"),
            Line = testCase.TryGetProperty("LineNumber", out var line) && line.TryGetInt32(out var n) && n > 0 ? n : null,
            Traits = traits is { Count: > 0 } ? traits : null,
        };
    }

    public static TestResult? Result(JsonElement result)
    {
        if (result.ValueKind != JsonValueKind.Object || !result.TryGetProperty("TestCase", out var testCase))
        {
            return null;
        }

        var item = Item(testCase);
        var outcome = result.TryGetProperty("Outcome", out var o) && o.TryGetInt32(out var code) ? code : 0;
        var output = new System.Text.StringBuilder();
        string? info = null;
        if (result.TryGetProperty("Messages", out var messages) && messages.ValueKind == JsonValueKind.Array)
        {
            foreach (var category in new[] { "StdOutMsgs", "StdErrMsgs", "AdditionalInfo" })
            {
                foreach (var msg in messages.EnumerateArray().Where(x => Str(x, "Category") == category))
                {
                    var text = Str(msg, "Text") ?? string.Empty;
                    if (category == "AdditionalInfo")
                    {
                        info ??= text;
                    }

                    output.Append(text);
                    if (!text.EndsWith('\n'))
                    {
                        output.Append('\n');
                    }
                }
            }
        }

        var mapped = outcome switch
        {
            1 => TestOutcomes.Passed,
            2 => TestOutcomes.Failed,
            3 => TestOutcomes.Skipped,
            _ => TestOutcomes.NotRun,
        };
        return new TestResult(item.Id, mapped)
        {
            DurationMs = Str(result, "Duration") is { } d && TimeSpan.TryParse(d, CultureInfo.InvariantCulture, out var span) ? span.TotalMilliseconds : null,
            Message = Str(result, "ErrorMessage") ?? (mapped == TestOutcomes.Skipped ? info?.Trim() : null),
            StackTrace = Str(result, "ErrorStackTrace"),
            Output = output.Length == 0 ? null : output.ToString(),
            DisplayName = Str(result, "DisplayName") ?? item.DisplayName,
            FullyQualifiedName = item.FullyQualifiedName,
        };
    }

    /// <summary>A TestProcessStartInfo (<c>FileName</c>, <c>Arguments</c>, <c>WorkingDirectory</c>, <c>EnvironmentVariables</c>) as a launch.</summary>
    public static TestLaunch StartInfo(JsonElement info, TestContainer container)
    {
        ArgumentNullException.ThrowIfNull(container);
        var env = new Dictionary<string, string>(StringComparer.Ordinal);
        if (info.ValueKind == JsonValueKind.Object && info.TryGetProperty("EnvironmentVariables", out var vars) && vars.ValueKind == JsonValueKind.Object)
        {
            foreach (var v in vars.EnumerateObject())
            {
                env[v.Name] = v.Value.ValueKind == JsonValueKind.String ? v.Value.GetString() ?? string.Empty : v.Value.ToString();
            }
        }

        var args = SplitArguments(Str(info, "Arguments") ?? string.Empty);
        return new TestLaunch(
            Str(info, "FileName") ?? "dotnet",
            args,
            Str(info, "WorkingDirectory") ?? Path.GetDirectoryName(container.Program) ?? Environment.CurrentDirectory,
            env,
            container.Runtime);
    }

    /// <summary>Splits a Windows-style command line (double quotes group, backslash-quote escapes a quote).</summary>
    public static IReadOnlyList<string> SplitArguments(string commandLine)
    {
        ArgumentNullException.ThrowIfNull(commandLine);
        var args = new List<string>();
        var current = new System.Text.StringBuilder();
        var quoted = false;
        var any = false;
        for (var i = 0; i < commandLine.Length; i++)
        {
            var c = commandLine[i];
            if (c == '\\' && i + 1 < commandLine.Length && commandLine[i + 1] == '"')
            {
                current.Append('"');
                i++;
                any = true;
            }
            else if (c == '"')
            {
                quoted = !quoted;
                any = true;
            }
            else if (char.IsWhiteSpace(c) && !quoted)
            {
                if (any)
                {
                    args.Add(current.ToString());
                    current.Clear();
                    any = false;
                }
            }
            else
            {
                current.Append(c);
                any = true;
            }
        }

        if (any)
        {
            args.Add(current.ToString());
        }

        return args;
    }

    private static string? Str(JsonElement e, string name) =>
        e.ValueKind == JsonValueKind.Object && e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;

    /// <summary>One vstest.console process and its connection.</summary>
    private sealed class Session : IAsyncDisposable
    {
        private readonly TestProcess _process;
        private readonly System.Net.Sockets.TcpClient _client;

        private Session(TestProcess process, System.Net.Sockets.TcpClient client, VsTestConnection connection)
        {
            _process = process;
            _client = client;
            Connection = connection;
        }

        public VsTestConnection Connection { get; }

        public static async Task<Session> StartAsync(TestContainer container, ITestSink sink, CancellationToken cancellationToken)
        {
            if (!File.Exists(container.Program))
            {
                throw new TestRunnerException($"not built: {container.Program} does not exist; build the project first");
            }

            var dotnet = container.DotnetExecutable ?? "dotnet";
            var console = VsTestConsoleLocator.Locate(container.VsTestConsole, dotnet)
                ?? throw new TestRunnerException(container.VsTestConsole is { Length: > 0 } c
                    ? $"vstest.console.dll not found at {c} (the setting test.vstestConsolePath)"
                    : "vstest.console.dll not found in any .NET SDK (`dotnet --list-sdks`); set test.vstestConsolePath");
            using var listener = TestProcess.Listen();
            var port = TestProcess.Port(listener);
            string[] args = [console, $"--port:{port}", $"--parentprocessid:{Environment.ProcessId}"];
            sink.Log($"> {TestProcess.CommandLine(dotnet, args)}");
            var process = TestProcess.Start(dotnet, args, Path.GetDirectoryName(container.Program) ?? Environment.CurrentDirectory, container.Environment, sink.Log);
            System.Net.Sockets.TcpClient? client = null;
            try
            {
                client = await TestProcess.AcceptAsync(listener, process, ConnectTimeout, "vstest.console", cancellationToken).ConfigureAwait(false);
                var connection = new VsTestConnection(client.GetStream());
                var session = new Session(process, client, connection);
                var (connected, _) = await connection.ReceiveAsync(cancellationToken).ConfigureAwait(false);
                if (connected != "TestSession.Connected")
                {
                    throw new TestRunnerException($"vstest.console said {connected} instead of TestSession.Connected");
                }

                await connection.SendAsync("ProtocolVersion", VsTestConnection.ProtocolVersion, versioned: false).ConfigureAwait(false);
                while (true)
                {
                    var (type, payload) = await connection.ReceiveAsync(cancellationToken).ConfigureAwait(false);
                    if (type == "ProtocolVersion")
                    {
                        sink.Log($"vstest.console speaks protocol version {payload}");
                        break;
                    }
                }

                return session;
            }
            catch
            {
                client?.Dispose();
                process.Dispose();
                throw;
            }
        }

        /// <summary>Kills vstest.console's tree when the request has not ended within the grace period.</summary>
        public void KillAfterGrace() =>
            _ = Task.Delay(ITestRunner.CancelGrace, CancellationToken.None).ContinueWith(_ => _process.Kill(), TaskScheduler.Default);

        public async Task TerminateAsync()
        {
            try
            {
                await Connection.SendAsync("TestSession.Terminate", null).ConfigureAwait(false);
                await _process.Exited.WaitAsync(TimeSpan.FromSeconds(5)).ConfigureAwait(false);
            }
            catch (Exception ex) when (ex is TestRunnerException or TimeoutException)
            {
            }
        }

        public async ValueTask DisposeAsync()
        {
            await Connection.DisposeAsync().ConfigureAwait(false);
            _client.Dispose();
            _process.Dispose();
        }
    }
}
