using System.Diagnostics;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading.Channels;
using Microsoft.Extensions.Time.Testing;
using Nerdbank.Streams;
using Eludite.Host.Lsp;
using Eludite.Host.Rpc;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0052: <c>textDocument/codeLens</c> and <c>codeLens/resolve</c> forwarded and typed, the Roslyn lens commands
/// mapped to Eludite's, and <c>workspace/codeLens/refresh</c> relayed, against the in-memory fake language server.
/// </summary>
public sealed class CodeLensTests : IAsyncDisposable
{
    /// <summary>The document the fake's lenses describe: the class on line 2, the test method on line 4.</summary>
    internal const string TestsText = "namespace Corpus;\n\npublic class CalculatorTests\n{\n    [Fact] public void Adds() { }\n}\n";

    private readonly FakeLanguageServer _fake = new();
    private readonly LspProxy _proxy;
    private readonly Task<int> _server;
    private readonly JsonRpc _client;
    private readonly Channel<JsonElement> _refreshes = Channel.CreateUnbounded<JsonElement>();

    public CodeLensTests()
    {
        _proxy = new LspProxy(_fake, TextWriter.Null, preparer: null, timeProvider: new FakeTimeProvider());
        var (clientStream, serverStream) = FullDuplexStream.CreatePair();
        var target = new HostRpcTarget(new FakeSdkDiscoverer(), TextWriter.Null, languageServer: _proxy);
        _server = HostServer.RunAsync(serverStream, serverStream, target);
        _client = TestRpc.Create(clientStream);
        TestRpc.On(_client, LspProxy.ShellCodeLensRefresh, p => _refreshes.Writer.TryWrite(p.Clone()));
        _client.StartListening();
    }

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private const string Uri = "file:///work/CalculatorTests.cs";

    private async Task InitializeAsync()
    {
        await _client.InvokeWithParameterObjectAsync<JsonElement>("eludite/host/initialize", new { clientName = "t", clientVersion = "0" }, Ct);
        await _fake.WaitForAsync("initialize");
        await _client.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri = Uri, languageId = "csharp", version = 1, text = TestsText } });
        await _fake.WaitForAsync("textDocument/didOpen");
    }

    private static JsonElement Arg(JsonElement lens) => lens.GetProperty("command").GetProperty("arguments")[0];

    [Fact]
    public async Task CodeLens_IsTypedForwardedAndRoslynCommandsAreMapped()
    {
        await InitializeAsync();
        foreach (var method in new[] { "textDocument/codeLens", "codeLens/resolve" })
        {
            Assert.Contains(method, LspProxy.TypedRequests);
            Assert.DoesNotContain(method, LspProxy.UntypedRequests);
        }

        var lenses = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/codeLens", new { textDocument = new { uri = Uri }, eluditeGeneration = 0 }, Ct);

        Assert.Equal(6, lenses.GetArrayLength());
        Assert.False(_fake.Last("textDocument/codeLens")!.Value.TryGetProperty(LspProxy.GenerationProperty, out _));
        // The references lenses are unresolved: no command, Roslyn's data unchanged.
        Assert.False(lenses[0].TryGetProperty("command", out _));
        Assert.Equal(0, lenses[0].GetProperty("data").GetProperty("listIndex").GetInt32());
        // dotnet.test.run becomes eludite.test.run or eludite.test.debug, with the member the range covers.
        var path = CodeLensCommands.PathOf(Uri);
        foreach (var (ix, title, command, member) in new[]
                 {
                     (2, "Run Test", "eludite.test.run", "Adds"),
                     (3, "Debug Test", "eludite.test.debug", "Adds"),
                     (4, "Run All Tests", "eludite.test.run", "CalculatorTests"),
                     (5, "Debug All Tests", "eludite.test.debug", "CalculatorTests"),
                 })
        {
            var lens = lenses[ix];
            Assert.Equal(title, lens.GetProperty("command").GetProperty("title").GetString());
            Assert.Equal(command, lens.GetProperty("command").GetProperty("command").GetString());
            var arg = Arg(lens);
            Assert.Equal(Uri, arg.GetProperty("uri").GetString());
            Assert.Equal(path, arg.GetProperty("path").GetString());
            Assert.Equal(member, arg.GetProperty("member").GetString());
            Assert.Equal(lens.GetProperty("range").GetRawText(), arg.GetProperty("range").GetRawText());
            Assert.False(arg.TryGetProperty("attachDebugger", out _));
        }

        // codeLens/resolve takes the lens back with its data; roslyn.client.peekReferences becomes
        // eludite.editor.find_references with the symbol's position.
        var unresolved = JsonNode.Parse(lenses[0].GetRawText())!.AsObject();
        unresolved[LspProxy.GenerationProperty] = 0;
        var resolved = await _client.InvokeWithParameterObjectAsync<JsonElement>("codeLens/resolve", unresolved, Ct);
        Assert.Equal("3 references", resolved.GetProperty("command").GetProperty("title").GetString());
        Assert.Equal("eludite.editor.find_references", resolved.GetProperty("command").GetProperty("command").GetString());
        var refs = Arg(resolved);
        Assert.Equal(Uri, refs.GetProperty("uri").GetString());
        Assert.Equal(path, refs.GetProperty("path").GetString());
        Assert.Equal(2, refs.GetProperty("position").GetProperty("line").GetInt32());
        Assert.Equal(13, refs.GetProperty("position").GetProperty("character").GetInt32());
        var upstream = _fake.Last("codeLens/resolve")!.Value;
        Assert.False(upstream.TryGetProperty(LspProxy.GenerationProperty, out _));
        Assert.Equal("1", upstream.GetProperty("data").GetProperty("syntaxVersion").GetString());

        // codeLens/resolve without a range is InvalidParams and never reaches the server.
        var (code, _) = await TestRpc.ErrorOfAsync(() => _client.InvokeWithParameterObjectAsync<JsonElement>(
            "codeLens/resolve", new { data = new { listIndex = 0 }, eluditeGeneration = 0 }, Ct));
        Assert.Equal(HostErrors.InvalidParams, code);
        (code, _) = await TestRpc.ErrorOfAsync(() => _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/codeLens", new { eluditeGeneration = 0 }, Ct));
        Assert.Equal(HostErrors.InvalidParams, code);
        Assert.Equal(1, _fake.Count("codeLens/resolve"));
        Assert.Equal(1, _fake.Count("textDocument/codeLens"));
    }

    [Fact]
    public async Task CodeLensRefresh_IsAnsweredAndRelayedToTheShellWithTheGeneration()
    {
        await InitializeAsync();
        var generation = _proxy.BumpGeneration();

        await _fake.RequestCodeLensRefreshAsync().WaitAsync(TimeSpan.FromSeconds(10), Ct);

        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(Ct);
        timeout.CancelAfter(TimeSpan.FromSeconds(10));
        var relayed = await _refreshes.Reader.ReadAsync(timeout.Token);
        Assert.Equal(generation, relayed.GetProperty(LspProxy.GenerationProperty).GetInt64());
    }

    [Fact]
    public async Task ClientCapabilities_AdvertiseCodeLensAndItsRefresh()
    {
        await InitializeAsync();

        var caps = _fake.Last("initialize")!.Value.GetProperty("capabilities");
        Assert.Equal(JsonValueKind.Object, caps.GetProperty("textDocument").GetProperty("codeLens").ValueKind);
        Assert.True(caps.GetProperty("workspace").GetProperty("codeLens").GetProperty("refreshSupport").GetBoolean());
    }

    [Fact]
    public void Mapping_LeavesOtherCommandsAndShapesAlone_AndReadsTheMemberAtTheRange()
    {
        var other = JsonSerializer.SerializeToElement(new[]
        {
            new { range = new { start = new { line = 0, character = 0 }, end = new { line = 0, character = 1 } }, command = new { title = "x", command = "rust-analyzer.runSingle", arguments = new[] { 1 } } },
        });
        Assert.Equal(other.GetRawText(), CodeLensCommands.Map(other, _ => null).GetRawText());
        var none = JsonSerializer.SerializeToElement<object?>(null);
        Assert.Equal(JsonValueKind.Null, CodeLensCommands.Map(none, _ => null).ValueKind);

        static JsonObject Range(int line, int from, int to) => new()
        {
            ["start"] = new JsonObject { ["line"] = line, ["character"] = from },
            ["end"] = new JsonObject { ["line"] = line, ["character"] = to },
        };
        Assert.Equal("Adds", CodeLensCommands.MemberAt(TestsText, Range(4, 23, 27)));
        // A verbatim identifier, CRLF line ends, an empty range (the identifier that starts there), unknown text.
        Assert.Equal("class", CodeLensCommands.MemberAt("void @class() {}", Range(0, 5, 11)));
        Assert.Equal("Second", CodeLensCommands.MemberAt("a\r\nvoid Second()\r\n", Range(1, 5, 11)));
        Assert.Equal("Second", CodeLensCommands.MemberAt("a\r\nvoid Second()\r\n", Range(1, 5, 5)));
        Assert.Equal(string.Empty, CodeLensCommands.MemberAt(null, Range(0, 0, 1)));
        // A test lens whose document the host does not hold keeps its range and an empty member.
        var lens = new JsonObject
        {
            ["range"] = Range(0, 0, 1),
            ["command"] = new JsonObject
            {
                ["title"] = "Run Test",
                ["command"] = "dotnet.test.run",
                ["arguments"] = new JsonArray(new JsonObject { ["textDocument"] = new JsonObject { ["uri"] = "file:///x.cs" }, ["range"] = Range(0, 0, 1), ["attachDebugger"] = false }),
            },
        };
        Assert.True(CodeLensCommands.MapLens(lens, _ => null));
        Assert.Equal("eludite.test.run", lens["command"]!["command"]!.GetValue<string>());
        Assert.Equal(string.Empty, lens["command"]!["arguments"]![0]!["member"]!.GetValue<string>());
    }

    public async ValueTask DisposeAsync()
    {
        _client.Dispose();
        await _proxy.DisposeAsync();
        try
        {
            await _server.WaitAsync(TimeSpan.FromSeconds(5));
        }
        catch (Exception)
        {
            // The server loop ends with the connection.
        }
    }
}

/// <summary>
/// Brief 0052's proving test against the real language server: the pinned Roslyn (tools/roslyn-pin) through the real
/// eludite-host process, on the test corpus's xunit.v3 project (corpus/tests, built in place by build.sh): the
/// references lens of <c>Calculator</c> (used three times from CalculatorTests.cs) resolves to a count above 1 that
/// matches <c>textDocument/references</c> without the declaration, and the test lenses carry Eludite's commands with
/// the member. Skipped with a message when the language server or the built corpus is absent.
/// </summary>
public sealed class RealCodeLensTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    [Fact]
    public async Task RealRoslyn_AnswersReferencesAndTestLensesOnTheCorpus_AndTheHostMapsTheirCommands()
    {
        var roslyn = RoslynProcessLauncher.Locate(null);
        Assert.SkipWhen(roslyn is null, "Roslyn language server not built; run tools/roslyn-pin/build.sh (or set ELUDITE_ROSLYN_LS).");
        var corpus = FindCorpus();
        Assert.SkipWhen(corpus is null, "corpus/tests not found above the test assembly.");
        var project = Path.Combine(corpus!, "Corpus.XunitV3", "Corpus.XunitV3.csproj");
        Assert.SkipWhen(
            !File.Exists(Path.Combine(corpus!, "Corpus.XunitV3", "obj", "project.assets.json")),
            "corpus/tests is not restored; run corpus/tests/build.sh.");
        var calculator = Path.Combine(corpus!, "Corpus.XunitV3", "Calculator.cs");
        var tests = Path.Combine(corpus!, "Corpus.XunitV3", "CalculatorTests.cs");

        var psi = new ProcessStartInfo("dotnet")
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        psi.ArgumentList.Add(Path.Combine(AppContext.BaseDirectory, "eludite-host.dll"));
        psi.ArgumentList.Add("--stdio");
        psi.ArgumentList.Add("--roslyn-ls");
        psi.ArgumentList.Add(roslyn!);
        using var host = Process.Start(psi)!;
        var stderr = host.StandardError.ReadToEndAsync(Ct);
        try
        {
            using var rpc = TestRpc.Create(host.StandardInput.BaseStream, host.StandardOutput.BaseStream);
            var loaded = new TaskCompletionSource<JsonElement>(TaskCreationOptions.RunContinuationsAsynchronously);
            TestRpc.On(rpc, "eludite/solution/status", s =>
            {
                if (s.GetProperty("state").GetString() is "loaded" or "failed")
                {
                    loaded.TrySetResult(s.Clone());
                }
            });
            rpc.StartListening();
            await rpc.InvokeWithParameterObjectAsync<JsonElement>("eludite/host/initialize", new { clientName = "codelens-test", clientVersion = "0" }, Ct);
            var opened = await rpc.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/open", new { path = project }, Ct);
            var generation = opened.GetProperty("generation").GetInt64();
            var calculatorUri = new Uri(calculator).AbsoluteUri;
            var testsUri = new Uri(tests).AbsoluteUri;
            foreach (var (uri, file) in new[] { (calculatorUri, calculator), (testsUri, tests) })
            {
                await rpc.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri, languageId = "csharp", version = 1, text = await File.ReadAllTextAsync(file, Ct) } });
            }

            var status = await loaded.Task.WaitAsync(TimeSpan.FromMinutes(5), Ct);
            Assert.Equal("loaded", status.GetProperty("state").GetString());

            // The references lens of the class Calculator, used from the second file.
            var text = await File.ReadAllTextAsync(calculator, Ct);
            var classLine = text.Split('\n').ToList().FindIndex(l => l.Contains("class Calculator", StringComparison.Ordinal));
            Assert.True(classLine >= 0);
            var watch = Stopwatch.StartNew();
            var lenses = await rpc.InvokeWithParameterObjectAsync<JsonElement>("textDocument/codeLens", new { textDocument = new { uri = calculatorUri }, eluditeGeneration = generation }, Ct);
            var requestMs = watch.Elapsed.TotalMilliseconds;
            var lens = lenses.EnumerateArray().First(l => l.GetProperty("range").GetProperty("start").GetProperty("line").GetInt32() == classLine);
            JsonElement resolved = default;
            for (var attempt = 0; ; attempt++)
            {
                var again = JsonNode.Parse(lens.GetRawText())!.AsObject();
                again[LspProxy.GenerationProperty] = generation;
                try
                {
                    watch.Restart();
                    resolved = await rpc.InvokeWithParameterObjectAsync<JsonElement>("codeLens/resolve", again, Ct);
                    break;
                }
                catch (RemoteInvocationException) when (attempt < 5)
                {
                    // ContentModified while Roslyn's syntax version settles: ask again, as the shell does.
                    await Task.Delay(500, Ct);
                }
            }

            var resolveMs = watch.Elapsed.TotalMilliseconds;
            var command = resolved.GetProperty("command");
            Assert.Equal("eludite.editor.find_references", command.GetProperty("command").GetString());
            var title = command.GetProperty("title").GetString()!;
            var count = int.Parse(title.Split(' ')[0].TrimEnd('+'), System.Globalization.CultureInfo.InvariantCulture);
            Assert.True(count > 1, $"the Calculator lens reads {title}");
            var arg = command.GetProperty("arguments")[0];
            Assert.Equal(calculator, arg.GetProperty("path").GetString());
            var position = arg.GetProperty("position");
            var references = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
                "textDocument/references",
                new { textDocument = new { uri = calculatorUri }, position, context = new { includeDeclaration = false }, eluditeGeneration = generation },
                Ct);
            Assert.Equal(count, references.GetArrayLength());

            // The test lenses of CalculatorTests.cs carry eludite.test.run and eludite.test.debug with the member.
            var testLenses = await rpc.InvokeWithParameterObjectAsync<JsonElement>("textDocument/codeLens", new { textDocument = new { uri = testsUri }, eluditeGeneration = generation }, Ct);
            var mapped = testLenses.EnumerateArray()
                .Where(l => l.TryGetProperty("command", out var c) && c.GetProperty("command").GetString()!.StartsWith("eludite.test.", StringComparison.Ordinal))
                .Select(l => (Command: l.GetProperty("command").GetProperty("command").GetString(), Title: l.GetProperty("command").GetProperty("title").GetString(), Member: l.GetProperty("command").GetProperty("arguments")[0].GetProperty("member").GetString()))
                .ToList();
            Assert.Contains(("eludite.test.run", "Run Test", "Adds"), mapped);
            Assert.Contains(("eludite.test.debug", "Debug Test", "Adds"), mapped);
            Assert.Contains(mapped, m => m.Member == "CalculatorTests" && m.Command == "eludite.test.run");
            Assert.DoesNotContain(testLenses.EnumerateArray(), l => l.TryGetProperty("command", out var c) && c.GetProperty("command").GetString() == CodeLensCommands.RoslynRunTests);
            TestContext.Current.SendDiagnosticMessage(
                $"CodeLens on the corpus: codeLens {requestMs:F1} ms ({lenses.GetArrayLength()} lenses), resolve {resolveMs:F1} ms ({title}), test lenses {mapped.Count}");

            await rpc.InvokeWithCancellationAsync<JsonElement>("eludite/host/shutdown", [], Ct);
            await rpc.NotifyAsync("eludite/host/exit");
            await host.WaitForExitAsync(Ct).WaitAsync(TimeSpan.FromSeconds(30), Ct);
        }
        finally
        {
            if (!host.HasExited)
            {
                host.Kill(entireProcessTree: true);
            }

            var log = await stderr;
            TestContext.Current.TestOutputHelper?.WriteLine(log.Length > 4000 ? log[^4000..] : log);
        }
    }

    private static string? FindCorpus()
    {
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, "corpus", "tests");
            if (File.Exists(Path.Combine(candidate, "README.md")))
            {
                return candidate;
            }
        }

        return null;
    }
}
