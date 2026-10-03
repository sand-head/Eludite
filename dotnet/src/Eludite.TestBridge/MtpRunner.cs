using System.Globalization;
using System.Text.Json;

namespace Eludite.TestBridge;

/// <summary>
/// Microsoft.Testing.Platform's server mode (host-rpc.md, "Tests"): the runner listens on a loopback port, starts the
/// test application with <c>--server --client-port &lt;port&gt;</c> (a debug run has the shell start it under its
/// adapter instead), and speaks JSON-RPC 2.0 with it: <c>initialize</c>, then <c>testing/discoverTests</c> or
/// <c>testing/runTests</c>, whose <c>testing/testUpdates/tests</c> notifications carry the nodes and their states, and
/// <c>exit</c>. One application process serves one request. Cancel is <c>$/cancelRequest</c>; frameworks stop between
/// tests, so the process tree is killed when the request has not answered within <see cref="ITestRunner.CancelGrace"/>.
/// </summary>
public sealed class MtpRunner : ITestRunner
{
    public Task DiscoverAsync(TestContainer container, ITestSink sink, CancellationToken cancellationToken) =>
        ServeAsync(container, "testing/discoverTests", null, debug: false, sink, cancellationToken);

    public Task RunAsync(TestContainer container, IReadOnlyList<DiscoveredTest>? tests, bool debug, ITestSink sink, CancellationToken cancellationToken) =>
        ServeAsync(container, "testing/runTests", tests, debug, sink, cancellationToken);

    /// <summary>The command line that starts the test application, before <c>--server --client-port</c>.</summary>
    public static (string FileName, List<string> Args) CommandLine(TestContainer container)
    {
        ArgumentNullException.ThrowIfNull(container);
        var args = new List<string>();
        string fileName;
        switch (container.Runtime)
        {
            case TestRuntime.Mono:
                fileName = container.MonoExecutable ?? "mono";
                args.Add("--debug");
                args.Add(container.Program);
                break;
            case TestRuntime.NetFx:
                fileName = container.Program;
                break;
            default:
                // The apphost beside the DLL when the build made one, else `dotnet <dll>`.
                var apphost = OperatingSystem.IsWindows() ? Path.ChangeExtension(container.Program, ".exe") : Path.ChangeExtension(container.Program, null);
                if (container.Program.EndsWith(".dll", StringComparison.OrdinalIgnoreCase) && !File.Exists(apphost))
                {
                    fileName = container.DotnetExecutable ?? "dotnet";
                    args.Add(container.Program);
                }
                else
                {
                    fileName = container.Program.EndsWith(".dll", StringComparison.OrdinalIgnoreCase) ? apphost : container.Program;
                }

                break;
        }

        if (container.RunSettings is { Length: > 0 } settings)
        {
            args.Add("--settings");
            args.Add(settings);
        }

        return (fileName, args);
    }

    /// <summary>A debug run's launch: the program the adapter starts (the DLL under netcoredbg, the .exe under Mono).</summary>
    public static TestLaunch DebugLaunch(TestContainer container, int port)
    {
        ArgumentNullException.ThrowIfNull(container);
        var args = new List<string>();
        if (container.RunSettings is { Length: > 0 } settings)
        {
            args.Add("--settings");
            args.Add(settings);
        }

        args.AddRange(["--server", "--client-port", port.ToString(CultureInfo.InvariantCulture)]);
        var env = new Dictionary<string, string>(TestProcess.QuietEnvironment, StringComparer.Ordinal);
        if (container.Environment is not null)
        {
            foreach (var (k, v) in container.Environment)
            {
                env[k] = v;
            }
        }

        return new TestLaunch(container.Program, args, WorkingDirectory(container), env, container.Runtime);
    }

    private static string WorkingDirectory(TestContainer container) =>
        Path.GetDirectoryName(container.Program) ?? Environment.CurrentDirectory;

    private static async Task ServeAsync(TestContainer container, string method, IReadOnlyList<DiscoveredTest>? tests, bool debug, ITestSink sink, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(container);
        ArgumentNullException.ThrowIfNull(sink);
        if (!File.Exists(container.Program))
        {
            throw new TestRunnerException($"not built: {container.Program} does not exist; build the project first");
        }

        using var listener = TestProcess.Listen();
        var port = TestProcess.Port(listener);
        TestProcess? process = null;
        try
        {
            if (debug)
            {
                var launch = DebugLaunch(container, port);
                sink.Log($"Debugging {container.Name} ({container.TargetFramework}): {TestProcess.CommandLine(launch.Program, launch.Args)}");
                sink.Launch(launch);
            }
            else
            {
                var (fileName, args) = CommandLine(container);
                args.AddRange(["--server", "--client-port", port.ToString(CultureInfo.InvariantCulture)]);
                sink.Log($"> {TestProcess.CommandLine(fileName, args)}");
                process = TestProcess.Start(fileName, args, WorkingDirectory(container), container.Environment, sink.Log);
            }

            using var client = await TestProcess.AcceptAsync(listener, process, debug ? ITestRunner.DebugWait : TimeSpan.FromSeconds(30), container.Name, cancellationToken).ConfigureAwait(false);
            var mapping = new MtpMapping(sink);
            await using var connection = new MtpConnection(client.GetStream(), mapping.OnNotification);
            var (_, init) = await connection.RequestAsync("initialize", new
            {
                processId = Environment.ProcessId,
                clientInfo = new { name = "eludite", version = "0.1.0" },
                capabilities = new { testing = new { debuggerProvider = false } },
            }).ConfigureAwait(false);
            await WaitAsync(init, connection, null, process, cancellationToken).ConfigureAwait(false);

            object parameters = tests is null
                ? new { runId = Guid.NewGuid().ToString() }
                : new { runId = Guid.NewGuid().ToString(), tests = tests.Select(t => t.Native).ToList() };
            var (id, response) = await connection.RequestAsync(method, parameters).ConfigureAwait(false);
            await WaitAsync(response, connection, id, process, cancellationToken).ConfigureAwait(false);
            try
            {
                await connection.NotifyAsync("exit", new { }).ConfigureAwait(false);
            }
            catch (TestRunnerException)
            {
            }

            if (process is not null)
            {
                await process.Exited.WaitAsync(TimeSpan.FromSeconds(5), CancellationToken.None).ConfigureAwait(false);
            }
        }
        catch (TimeoutException)
        {
            // The application did not exit after `exit`; it is killed below.
        }
        finally
        {
            process?.Dispose();
        }
    }

    /// <summary>Waits for a response; on cancel, sends <c>$/cancelRequest</c> and kills the process after the grace period.</summary>
    private static async Task WaitAsync(Task<JsonElement> response, MtpConnection connection, long? id, TestProcess? process, CancellationToken cancellationToken)
    {
        var canceled = Task.Delay(Timeout.Infinite, cancellationToken);
        var first = await Task.WhenAny(response, canceled).ConfigureAwait(false);
        if (first == response)
        {
            await response.ConfigureAwait(false);
            return;
        }

        if (id is { } requestId)
        {
            try
            {
                await connection.NotifyAsync("$/cancelRequest", new { id = requestId }).ConfigureAwait(false);
            }
            catch (TestRunnerException)
            {
            }

            await Task.WhenAny(response, Task.Delay(ITestRunner.CancelGrace, CancellationToken.None)).ConfigureAwait(false);
        }

        process?.Kill();
        throw new OperationCanceledException(cancellationToken);
    }
}

/// <summary>Maps <c>testing/testUpdates/tests</c> and <c>client/log</c> to the sink (host-rpc.md, the mapping table).</summary>
internal sealed class MtpMapping(ITestSink sink)
{
    public void OnNotification(string method, JsonElement parameters)
    {
        switch (method)
        {
            case "testing/testUpdates/tests":
                OnUpdates(parameters);
                break;
            case "client/log":
                if (parameters.ValueKind == JsonValueKind.Object && parameters.TryGetProperty("message", out var message))
                {
                    var level = parameters.TryGetProperty("level", out var l) ? l.GetString() : null;
                    if (!string.Equals(level, "Trace", StringComparison.OrdinalIgnoreCase))
                    {
                        foreach (var line in (message.GetString() ?? string.Empty).Replace("\r\n", "\n", StringComparison.Ordinal).Split('\n'))
                        {
                            sink.Log(line);
                        }
                    }
                }

                break;
        }
    }

    private void OnUpdates(JsonElement parameters)
    {
        if (parameters.ValueKind != JsonValueKind.Object
            || !parameters.TryGetProperty("changes", out var changes)
            || changes.ValueKind != JsonValueKind.Array)
        {
            return;
        }

        var discovered = new List<DiscoveredTest>();
        var results = new List<TestResult>();
        foreach (var change in changes.EnumerateArray())
        {
            if (!change.TryGetProperty("node", out var node) || node.ValueKind != JsonValueKind.Object)
            {
                continue;
            }

            // Only test nodes ("action"); group nodes a framework may report are not tests.
            if (Str(node, "node-type") is { } type && type != "action")
            {
                continue;
            }

            var state = Str(node, "execution-state");
            if (state == "discovered")
            {
                discovered.Add(new DiscoveredTest(Item(node), Native(node)));
            }
            else if (Outcome(state) is { } outcome)
            {
                results.Add(Result(node, outcome));
            }
        }

        if (discovered.Count > 0)
        {
            sink.Tests(discovered);
        }

        if (results.Count > 0)
        {
            sink.Results(results);
        }
    }

    /// <summary>The node to send back in <c>testing/runTests</c>: its uid and display name.</summary>
    private static JsonElement Native(JsonElement node) =>
        JsonSerializer.SerializeToElement(new Dictionary<string, string?>
        {
            ["uid"] = Str(node, "uid"),
            ["display-name"] = Str(node, "display-name"),
        });

    public static TestItem Item(JsonElement node)
    {
        var uid = Str(node, "uid") ?? string.Empty;
        var display = Str(node, "display-name") ?? uid;
        var type = Str(node, "location.type");
        var method = Str(node, "location.method");
        var (ns, cls, m) = TestNames.Split(type, method);
        return new TestItem(uid, display, TestNames.FullName(type, method, display))
        {
            Namespace = ns,
            ClassName = cls,
            Method = m,
            Source = Str(node, "location.file"),
            Line = node.TryGetProperty("location.line-start", out var line) && line.TryGetInt32(out var n) && n > 0 ? n : null,
            Traits = Traits(node),
        };
    }

    /// <summary>
    /// MTP's <c>traits</c> are <c>[{ name: value }]</c>. MSTest reports a <c>[TestCategory("X")]</c> as <c>{ X: "" }</c>,
    /// which is read as <c>Category=X</c>, as VSTest names it.
    /// </summary>
    private static IReadOnlyList<TestTrait>? Traits(JsonElement node)
    {
        if (!node.TryGetProperty("traits", out var traits) || traits.ValueKind != JsonValueKind.Array)
        {
            return null;
        }

        var list = new List<TestTrait>();
        foreach (var t in traits.EnumerateArray())
        {
            if (t.ValueKind != JsonValueKind.Object)
            {
                continue;
            }

            foreach (var p in t.EnumerateObject())
            {
                var value = p.Value.ValueKind == JsonValueKind.String ? p.Value.GetString() ?? string.Empty : p.Value.ToString();
                list.Add(value.Length == 0 ? new TestTrait("Category", p.Name) : new TestTrait(p.Name, value));
            }
        }

        return list.Count == 0 ? null : list;
    }

    public static string? Outcome(string? state) => state switch
    {
        "in-progress" => TestOutcomes.Running,
        "passed" => TestOutcomes.Passed,
        "failed" or "timed-out" or "error" => TestOutcomes.Failed,
        "skipped" => TestOutcomes.Skipped,
        "cancelled" => TestOutcomes.NotRun,
        _ => null,
    };

    public static TestResult Result(JsonElement node, string outcome)
    {
        var item = Item(node);
        var stdout = Str(node, "standardOutput");
        var stderr = Str(node, "standardError");
        var output = (stdout, stderr) switch
        {
            (null, null) => null,
            (_, null) => stdout,
            (null, _) => stderr,
            _ => stdout!.EndsWith('\n') ? stdout + stderr : stdout + "\n" + stderr,
        };
        return new TestResult(item.Id, outcome)
        {
            DurationMs = node.TryGetProperty("time.duration-ms", out var d) && d.TryGetDouble(out var ms) ? ms : null,
            Message = Str(node, "error.message"),
            StackTrace = Str(node, "error.stacktrace"),
            Output = string.IsNullOrEmpty(output) ? null : output,
            DisplayName = item.DisplayName,
            FullyQualifiedName = item.FullyQualifiedName,
        };
    }

    private static string? Str(JsonElement node, string name) =>
        node.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;
}
