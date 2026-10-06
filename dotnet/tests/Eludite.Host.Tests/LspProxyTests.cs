using System.Diagnostics;
using System.Text.Json;
using System.Threading.Channels;
using Microsoft.Extensions.Time.Testing;
using Nerdbank.Streams;
using Eludite.Host.Lsp;
using Eludite.Host.Rpc;
using StreamJsonRpc;
using StreamJsonRpc.Protocol;

namespace Eludite.Host.Tests;

/// <summary>The LSP bridge in eludite-host against an in-memory fake language server.</summary>
public sealed class LspProxyTests : IAsyncDisposable
{
    private readonly FakeLanguageServer _fake = new();
    private readonly FakeTimeProvider _time = new();
    private readonly FakePreparer _preparer = new();
    private readonly LspProxy _proxy;
    private readonly HostRpcTarget _target;
    private readonly Task<int> _server;
    private readonly JsonRpc _client;
    private readonly DirectoryInfo _dir = Directory.CreateTempSubdirectory("eludite-0007-");
    private readonly Channel<JsonElement> _solutionStatus = Channel.CreateUnbounded<JsonElement>();
    private readonly Channel<JsonElement> _serverStatus = Channel.CreateUnbounded<JsonElement>();
    private readonly Channel<JsonElement> _diagnostics = Channel.CreateUnbounded<JsonElement>();
    private readonly Channel<JsonElement> _applyEdits = Channel.CreateUnbounded<JsonElement>();

    /// <summary>How the test's shell answers <c>workspace/applyEdit</c>.</summary>
    private Func<JsonElement, CancellationToken, Task<JsonElement>> _applyEdit =
        (_, _) => Task.FromResult(JsonSerializer.SerializeToElement(new { applied = true }));

    public LspProxyTests()
    {
        _proxy = new LspProxy(_fake, TextWriter.Null, _preparer, timeProvider: _time);
        var (clientStream, serverStream) = FullDuplexStream.CreatePair();
        _target = new HostRpcTarget(new FakeSdkDiscoverer(), TextWriter.Null, languageServer: _proxy);
        _server = HostServer.RunAsync(serverStream, serverStream, _target);
        _client = TestRpc.Create(clientStream);
        Collect("eludite/solution/status", _solutionStatus);
        Collect("eludite/languageServer/status", _serverStatus);
        Collect("textDocument/publishDiagnostics", _diagnostics);
        var applyEdit = new Func<JsonElement, CancellationToken, Task<JsonElement>>((p, ct) =>
        {
            _applyEdits.Writer.TryWrite(p.Clone());
            return _applyEdit(p, ct);
        });
        _client.AddLocalRpcMethod(applyEdit.Method, applyEdit.Target, new JsonRpcMethodAttribute("workspace/applyEdit") { UseSingleObjectParameterDeserialization = true });
        _client.StartListening();
    }

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private string Uri(string name) => new Uri(Path.Combine(_dir.FullName, name)).AbsoluteUri;

    private void Collect(string method, Channel<JsonElement> into)
    {
        var handler = new Action<JsonElement>(p => into.Writer.TryWrite(p.Clone()));
        _client.AddLocalRpcMethod(handler.Method, handler.Target, new JsonRpcMethodAttribute(method) { UseSingleObjectParameterDeserialization = true });
    }

    private static async Task<JsonElement> NextAsync(Channel<JsonElement> channel, Func<JsonElement, bool>? where = null)
    {
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(Ct);
        timeout.CancelAfter(TimeSpan.FromSeconds(10));
        while (true)
        {
            var item = await channel.Reader.ReadAsync(timeout.Token);
            if (where is null || where(item))
            {
                return item;
            }
        }
    }

    private Task InitializeAsync() =>
        _client.InvokeWithParameterObjectAsync<JsonElement>("eludite/host/initialize", new { clientName = "t", clientVersion = "0" }, Ct);

    private async Task<string> WriteSolutionAsync(string name = "App.slnx", bool legacy = false)
    {
        var project = Path.Combine(_dir.FullName, "Lib", "Lib.csproj");
        Directory.CreateDirectory(Path.GetDirectoryName(project)!);
        await File.WriteAllTextAsync(
            project,
            legacy
                ? """<Project ToolsVersion="15.0" xmlns="http://schemas.microsoft.com/developer/msbuild/2003"><PropertyGroup /></Project>"""
                : """<Project Sdk="Microsoft.NET.Sdk" />""",
            Ct);
        var sln = Path.Combine(_dir.FullName, name);
        await File.WriteAllTextAsync(sln, """<Solution><Project Path="Lib/Lib.csproj" /></Solution>""", Ct);
        return sln;
    }

    private async Task<long> OpenAsync(string sln)
    {
        var r = await _client.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/open", new { path = sln }, Ct);
        return r.GetProperty("generation").GetInt64();
    }

    private Task<JsonElement> CompleteAsync(long generation, string uri = "file:///a.cs", CancellationToken? ct = null) =>
        _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/completion",
            new { textDocument = new { uri }, position = new { line = 3, character = 7 }, eluditeGeneration = generation },
            ct ?? Ct);

    private Task DidOpenAsync(string uri, string text, int version = 1) =>
        _client.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri, languageId = "csharp", version, text } });

    [Fact]
    public async Task Initialize_PerformsUpstreamHandshakeAndOpensSolution()
    {
        await InitializeAsync();
        var sln = await WriteSolutionAsync();

        var generation = await OpenAsync(sln);

        Assert.Equal(1, generation);
        await _fake.WaitForAsync("solution/open");
        Assert.Equal(new Uri(sln).AbsoluteUri, _fake.Last("solution/open")!.Value.GetProperty("solution").GetString());
        Assert.Equal(["initialize", "initialized", "solution/open"], _fake.Snapshot().Take(3));
        Assert.Equal(1, _fake.Launches);
    }

    [Fact]
    public async Task LanguageServerStatus_ReportsRunningWithServerCapabilities()
    {
        await InitializeAsync();

        Assert.Equal("starting", (await NextAsync(_serverStatus)).GetProperty("state").GetString());
        var running = await NextAsync(_serverStatus);
        Assert.Equal("running", running.GetProperty("state").GetString());
        Assert.Equal("fake-ls", running.GetProperty("serverInfo").GetProperty("name").GetString());
        Assert.True(running.GetProperty("capabilities").GetProperty("hoverProvider").GetBoolean());
    }

    [Fact]
    public async Task Completion_IsForwardedWithParamsIntactAndResultReturned()
    {
        await InitializeAsync();

        var result = await CompleteAsync(0);

        Assert.Equal("Compute", result.GetProperty("items")[0].GetProperty("label").GetString());
        var forwarded = _fake.Last("textDocument/completion")!.Value;
        Assert.Equal("file:///a.cs", forwarded.GetProperty("textDocument").GetProperty("uri").GetString());
        Assert.Equal(7, forwarded.GetProperty("position").GetProperty("character").GetInt32());
        Assert.False(forwarded.TryGetProperty(LspProxy.GenerationProperty, out _));
    }

    [Fact]
    public async Task UntypedRequest_IsForwardedVerbatim()
    {
        await InitializeAsync();

        var result = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/typeDefinition",
            new { textDocument = new { uri = "file:///a.cs" }, position = new { line = 1, character = 2 }, custom = "kept", eluditeGeneration = 0 },
            Ct);

        Assert.Equal("file:///b.cs", result[0].GetProperty("uri").GetString());
        var forwarded = _fake.Last("textDocument/typeDefinition")!.Value;
        Assert.Equal("kept", forwarded.GetProperty("custom").GetString());
        Assert.False(forwarded.TryGetProperty(LspProxy.GenerationProperty, out _));
    }

    [Fact]
    public async Task MissingGeneration_IsInvalidParamsAndNotForwarded()
    {
        await InitializeAsync();

        var (code, _) = await TestRpc.ErrorOfAsync(() => _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/completion", new { textDocument = new { uri = "file:///a.cs" }, position = new { line = 0, character = 0 } }, Ct));

        Assert.Equal(HostErrors.InvalidParams, code);
        Assert.Equal(0, _fake.Count("textDocument/completion"));
    }

    [Fact]
    public async Task TypedRequest_WithoutDocumentUri_IsInvalidParams()
    {
        await InitializeAsync();

        var (code, _) = await TestRpc.ErrorOfAsync(() => _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/hover", new { position = new { line = 0, character = 0 }, eluditeGeneration = 0 }, Ct));

        Assert.Equal(HostErrors.InvalidParams, code);
    }

    [Fact]
    public async Task SignatureHelp_IsTypedAndValidated()
    {
        await InitializeAsync();

        Assert.Contains("textDocument/signatureHelp", LspProxy.TypedRequests);
        Assert.DoesNotContain("textDocument/signatureHelp", LspProxy.UntypedRequests);
        var (code, _) = await TestRpc.ErrorOfAsync(() => _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/signatureHelp", new { position = new { line = 0, character = 0 }, eluditeGeneration = 0 }, Ct));
        Assert.Equal(HostErrors.InvalidParams, code);
        Assert.Null(_fake.Last("textDocument/signatureHelp"));

        var result = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/signatureHelp",
            new { textDocument = new { uri = "file:///a.cs" }, position = new { line = 1, character = 2 }, eluditeGeneration = 0 },
            Ct);
        Assert.Equal("M(int x)", result.GetProperty("signatures")[0].GetProperty("label").GetString());
        Assert.False(_fake.Last("textDocument/signatureHelp")!.Value.TryGetProperty(LspProxy.GenerationProperty, out _));
    }

    [Fact]
    public async Task RenameAndCodeActions_AreTypedValidatedAndForwarded()
    {
        await InitializeAsync();

        foreach (var method in new[] { "textDocument/prepareRename", "textDocument/rename", "textDocument/codeAction", "codeAction/resolve" })
        {
            Assert.Contains(method, LspProxy.TypedRequests);
            Assert.DoesNotContain(method, LspProxy.UntypedRequests);
        }

        var position = new { line = 2, character = 5 };
        var range = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/prepareRename", new { textDocument = new { uri = "file:///a.cs" }, position, eluditeGeneration = 0 }, Ct);
        Assert.Equal(4, range.GetProperty("start").GetProperty("character").GetInt32());

        var edit = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/rename", new { textDocument = new { uri = "file:///a.cs" }, position, newName = "Pong", eluditeGeneration = 0 }, Ct);
        Assert.Equal("Pong", edit.GetProperty("documentChanges")[0].GetProperty("edits")[0].GetProperty("newText").GetString());
        Assert.Equal("Pong", _fake.Last("textDocument/rename")!.Value.GetProperty("newName").GetString());

        var actions = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/codeAction",
            new { textDocument = new { uri = "file:///a.cs" }, range = new { start = position, end = position }, context = new { diagnostics = Array.Empty<object>(), triggerKind = 2 }, eluditeGeneration = 0 },
            Ct);
        var action = actions[0];
        Assert.Equal("Use primary constructor", action.GetProperty("title").GetString());

        // codeAction/resolve takes the action back with its data; a missing title is InvalidParams.
        var resolved = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "codeAction/resolve", new { title = "Use primary constructor", kind = "quickfix", data = new { id = 1 }, eluditeGeneration = 0 }, Ct);
        Assert.True(resolved.TryGetProperty("edit", out _));
        Assert.Equal(1, _fake.Last("codeAction/resolve")!.Value.GetProperty("data").GetProperty("id").GetInt32());
        var (code, _) = await TestRpc.ErrorOfAsync(() => _client.InvokeWithParameterObjectAsync<JsonElement>(
            "codeAction/resolve", new { kind = "quickfix", eluditeGeneration = 0 }, Ct));
        Assert.Equal(HostErrors.InvalidParams, code);
        Assert.Equal(1, _fake.Count("codeAction/resolve"));
        (code, _) = await TestRpc.ErrorOfAsync(() => _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/rename", new { position, newName = "X", eluditeGeneration = 0 }, Ct));
        Assert.Equal(HostErrors.InvalidParams, code);
        foreach (var method in new[] { "textDocument/prepareRename", "textDocument/rename", "textDocument/codeAction", "codeAction/resolve" })
        {
            Assert.False(_fake.Last(method)!.Value.TryGetProperty(LspProxy.GenerationProperty, out _), method);
        }
    }

    [Fact]
    public async Task ClientCapabilities_AdvertiseWorkspaceEditsCodeActionsAndRename()
    {
        await InitializeAsync();
        await _fake.WaitForAsync("initialize");

        var caps = _fake.Last("initialize")!.Value.GetProperty("capabilities");
        var workspace = caps.GetProperty("workspace");
        Assert.True(workspace.GetProperty("applyEdit").GetBoolean());
        var workspaceEdit = workspace.GetProperty("workspaceEdit");
        Assert.True(workspaceEdit.GetProperty("documentChanges").GetBoolean());
        Assert.Equal(["create", "rename", "delete"], workspaceEdit.GetProperty("resourceOperations").EnumerateArray().Select(e => e.GetString()));
        var codeAction = caps.GetProperty("textDocument").GetProperty("codeAction");
        Assert.Equal(["edit"], codeAction.GetProperty("resolveSupport").GetProperty("properties").EnumerateArray().Select(e => e.GetString()));
        Assert.True(codeAction.GetProperty("dataSupport").GetBoolean());
        Assert.Contains("quickfix", codeAction.GetProperty("codeActionLiteralSupport").GetProperty("codeActionKind").GetProperty("valueSet").EnumerateArray().Select(e => e.GetString()));
        Assert.True(caps.GetProperty("textDocument").GetProperty("rename").GetProperty("prepareSupport").GetBoolean());
    }

    [Fact]
    public async Task ApplyEdit_IsRelayedToTheShellWithTheGenerationAndAnswered()
    {
        await InitializeAsync();
        var generation = await OpenAsync(await WriteSolutionAsync());
        await _fake.WaitForAsync("solution/open");
        var edit = new { changes = new Dictionary<string, object[]> { ["file:///a.cs"] = [new { range = new { start = new { line = 0, character = 0 }, end = new { line = 0, character = 0 } }, newText = "// x\n" }] } };

        var answer = await _fake.ApplyEditAsync(new { label = "Fix", edit }, Ct);

        Assert.True(answer.GetProperty("applied").GetBoolean());
        var relayed = await NextAsync(_applyEdits);
        Assert.Equal(generation, relayed.GetProperty(LspProxy.GenerationProperty).GetInt64());
        Assert.Equal("Fix", relayed.GetProperty("label").GetString());
        Assert.Equal("// x\n", relayed.GetProperty("edit").GetProperty("changes").GetProperty("file:///a.cs")[0].GetProperty("newText").GetString());

        // The shell's refusal comes back unchanged.
        _applyEdit = (_, _) => Task.FromResult(JsonSerializer.SerializeToElement(new { applied = false, failureReason = "stale" }));
        answer = await _fake.ApplyEditAsync(new { edit }, Ct);
        Assert.False(answer.GetProperty("applied").GetBoolean());
        Assert.Equal("stale", answer.GetProperty("failureReason").GetString());
    }

    [Fact]
    public async Task ApplyEdit_ShellErrorIsNotAppliedAndCancellationIsRelayed()
    {
        await InitializeAsync();
        await _fake.WaitForAsync("initialized");
        var edit = new { changes = new Dictionary<string, object[]>() };

        _applyEdit = (_, _) => throw new LocalRpcException("the applier failed") { ErrorCode = -32603 };
        var answer = await _fake.ApplyEditAsync(new { edit }, Ct);
        Assert.False(answer.GetProperty("applied").GetBoolean());
        Assert.Contains("the applier failed", answer.GetProperty("failureReason").GetString());

        var canceled = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var started = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        _applyEdit = async (_, ct) =>
        {
            started.TrySetResult();
            try
            {
                await Task.Delay(Timeout.Infinite, ct);
            }
            catch (OperationCanceledException)
            {
                canceled.TrySetResult();
                throw;
            }

            return default;
        };
        using var cts = CancellationTokenSource.CreateLinkedTokenSource(Ct);
        var pending = _fake.ApplyEditAsync(new { edit }, cts.Token);
        await started.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);
        await cts.CancelAsync();
        await canceled.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => pending);
    }

    [Fact]
    public async Task DidOpen_IsForwardedAsNotification()
    {
        await InitializeAsync();

        await DidOpenAsync("file:///a.cs", "class A {}");

        await _fake.WaitForAsync("textDocument/didOpen");
        Assert.Equal("class A {}", _fake.Last("textDocument/didOpen")!.Value.GetProperty("textDocument").GetProperty("text").GetString());
    }

    [Fact]
    public async Task CompletionAfterDidChange_ReachesTheServerAfterTheChange()
    {
        await InitializeAsync();
        await DidOpenAsync("file:///a.cs", "class A {}");

        for (var v = 2; v < 12; v++)
        {
            await _client.NotifyWithParameterObjectAsync("textDocument/didChange", new { textDocument = new { uri = "file:///a.cs", version = v }, contentChanges = new[] { new { text = $"class A{v} {{}}" } } });
            await CompleteAsync(0);
        }

        var order = _fake.Snapshot().Where(m => m is "textDocument/didChange" or "textDocument/completion").ToList();
        Assert.Equal(Enumerable.Range(0, 10).SelectMany(_ => new[] { "textDocument/didChange", "textDocument/completion" }), order);
    }

    [Fact]
    public async Task ProjectInitializationComplete_BecomesLoadedStatusWithoutChangingGeneration()
    {
        await InitializeAsync();
        var sln = await WriteSolutionAsync();
        var generation = await OpenAsync(sln);
        await _fake.WaitForAsync("solution/open");

        await _fake.NotifyProjectsLoadedAsync();

        var loaded = await NextAsync(_solutionStatus, s => s.GetProperty("state").GetString() == "loaded");
        await _proxy.ProjectsLoaded.WaitAsync(TimeSpan.FromSeconds(10), Ct);
        Assert.Equal(generation, loaded.GetProperty("generation").GetInt64());
        Assert.Equal(1, loaded.GetProperty("counts").GetProperty("projects").GetInt32());
        Assert.Equal(0, loaded.GetProperty("counts").GetProperty("legacyProjects").GetInt32());
        Assert.False(loaded.TryGetProperty("msbuild", out _));
        Assert.Equal(1, _proxy.Generation);
    }

    [Fact]
    public async Task LegacySolution_ReportsPreparationPhaseMsBuildAndCorrections()
    {
        await InitializeAsync();
        var sln = await WriteSolutionAsync(legacy: true);

        await OpenAsync(sln);

        var first = await NextAsync(_solutionStatus);
        Assert.Equal("loading", first.GetProperty("state").GetString());
        Assert.Equal("legacyEvaluation", first.GetProperty("phase").GetString());
        var second = await NextAsync(_solutionStatus);
        Assert.Equal("projectLoad", second.GetProperty("phase").GetString());
        await _fake.WaitForAsync("solution/open");
        Assert.Equal(1, _preparer.Calls);
        await _fake.NotifyProjectsLoadedAsync();
        var loaded = await NextAsync(_solutionStatus);
        Assert.Equal("loaded", loaded.GetProperty("state").GetString());
        Assert.Equal(1, loaded.GetProperty("counts").GetProperty("legacyProjects").GetInt32());
        Assert.Equal(1, loaded.GetProperty("counts").GetProperty("legacyEvaluationFailures").GetInt32());
        Assert.Equal("mono", loaded.GetProperty("msbuild").GetProperty("kind").GetString());
        Assert.Equal("caseFixups", loaded.GetProperty("corrections")[0].GetProperty("kind").GetString());
        Assert.Equal("ELUDITE0106", loaded.GetProperty("diagnostics")[0].GetProperty("code").GetString());
    }

    [Fact]
    public async Task CanceledRequest_ReturnsRequestCancelledWithin50msAndCancelsUpstream()
    {
        await InitializeAsync();
        _fake.HangCompletion = true;
        using var cts = new CancellationTokenSource();

        var call = CompleteAsync(0, ct: cts.Token);
        await _fake.CompletionStarted.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);

        var sw = Stopwatch.StartNew();
        await cts.CancelAsync();
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => call);
        sw.Stop();

        Assert.True(sw.ElapsedMilliseconds < 50, $"cancellation took {sw.ElapsedMilliseconds} ms");
        await _fake.CompletionCanceled.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);
    }

    [Fact]
    public async Task StaleGeneration_IsRejectedWithContentModified()
    {
        await InitializeAsync();
        var sln = await WriteSolutionAsync();
        Assert.Equal(1, await OpenAsync(sln));

        var (code, data) = await TestRpc.ErrorOfAsync(() => CompleteAsync(0));

        Assert.Equal(LspProxy.ContentModified, code);
        Assert.Equal(0, data!.Value.GetProperty("requestedGeneration").GetInt64());
        Assert.Equal(1, data.Value.GetProperty("currentGeneration").GetInt64());
        Assert.Equal(0, _fake.Count("textDocument/completion"));
    }

    [Fact]
    public async Task GenerationChangeWhileInFlight_CancelsUpstreamAndReturnsContentModified()
    {
        await InitializeAsync();
        _fake.HangCompletion = true;

        var call = CompleteAsync(0);
        await _fake.CompletionStarted.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);
        Assert.False(_fake.Last("textDocument/completion")!.Value.TryGetProperty(LspProxy.GenerationProperty, out _));

        _proxy.BumpGeneration();

        var ex = await Assert.ThrowsAsync<RemoteInvocationException>(() => call);
        Assert.Equal(LspProxy.ContentModified, ex.ErrorCode);
        await _fake.CompletionCanceled.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);
    }

    [Fact]
    public async Task SecondOpen_RestartsServerAndReplaysOpenDocumentsWithCurrentText()
    {
        await InitializeAsync();
        var sln = await WriteSolutionAsync();
        await OpenAsync(sln);
        await _fake.WaitForAsync("solution/open");
        var uri = Uri("A.cs");
        await DidOpenAsync(uri, "class A\n{\n}\n");
        await _client.NotifyWithParameterObjectAsync("textDocument/didChange", new
        {
            textDocument = new { uri, version = 2 },
            contentChanges = new[] { new { range = new { start = new { line = 1, character = 1 }, end = new { line = 1, character = 1 } }, text = " int X;" } },
        });

        var generation = await OpenAsync(await WriteSolutionAsync("Other.slnx"));

        Assert.Equal(2, generation);
        await _fake.WaitForAsync("solution/open", 2);
        Assert.Equal(2, _fake.Launches);
        var messages = _fake.Messages();
        Assert.Contains(messages, m => m.Launch == 1 && m.Method == "shutdown");
        var replayed = messages.Single(m => m.Launch == 2 && m.Method == "textDocument/didOpen").Params.GetProperty("textDocument");
        Assert.Equal(uri, replayed.GetProperty("uri").GetString());
        Assert.Equal(2, replayed.GetProperty("version").GetInt32());
        Assert.Equal("class A\n{ int X;\n}\n", replayed.GetProperty("text").GetString());
        Assert.Equal("restarting", (await NextAsync(_serverStatus, s => s.GetProperty("state").GetString() == "restarting")).GetProperty("state").GetString());
        Assert.Equal("Compute", (await CompleteAsync(2)).GetProperty("items")[0].GetProperty("label").GetString());
    }

    [Fact]
    public async Task OpenDocument_IsWarmedAtOnceAndDiagnosticsArePublishedWithVersionAndGeneration()
    {
        await InitializeAsync();
        var uri = Uri("A.cs");

        await DidOpenAsync(uri, "class A {}", version: 4);

        var published = await NextAsync(_diagnostics);
        Assert.Equal(uri, published.GetProperty("uri").GetString());
        Assert.Equal(4, published.GetProperty("version").GetInt32());
        Assert.Equal(0, published.GetProperty("eluditeGeneration").GetInt64());
        Assert.Equal("CS0168", published.GetProperty("diagnostics")[0].GetProperty("code").GetString());
        var order = _fake.Snapshot();
        Assert.True(order.IndexOf("textDocument/didOpen") < order.IndexOf("textDocument/diagnostic"));
    }

    [Fact]
    public async Task Change_WarmingIsDebounced()
    {
        await InitializeAsync();
        var uri = Uri("A.cs");
        await DidOpenAsync(uri, "class A {}");
        await NextAsync(_diagnostics);
        Assert.Equal(1, _fake.Count("textDocument/diagnostic"));

        for (var v = 2; v <= 6; v++)
        {
            await _client.NotifyWithParameterObjectAsync("textDocument/didChange", new { textDocument = new { uri, version = v }, contentChanges = new[] { new { text = $"class A{v} {{}}" } } });
        }

        await _fake.WaitForAsync("textDocument/didChange", 5);
        _time.Advance(DiagnosticsWarmer.DefaultDebounce - TimeSpan.FromMilliseconds(1));
        await Task.Delay(100, Ct);
        Assert.Equal(1, _fake.Count("textDocument/diagnostic"));

        _time.Advance(TimeSpan.FromMilliseconds(1));
        var published = await NextAsync(_diagnostics);
        Assert.Equal(6, published.GetProperty("version").GetInt32());
        Assert.Equal(2, _fake.Count("textDocument/diagnostic"));
    }

    [Fact]
    public async Task SolutionLoaded_And_DiagnosticRefresh_RewarmOpenDocuments()
    {
        await InitializeAsync();
        await DidOpenAsync(Uri("A.cs"), "class A {}");
        await DidOpenAsync(Uri("B.cs"), "class B {}");
        await _fake.WaitForAsync("textDocument/diagnostic", 2);
        await OpenAsync(await WriteSolutionAsync());
        await _fake.WaitForAsync("solution/open");

        await _fake.NotifyProjectsLoadedAsync();
        await _fake.WaitForAsync("textDocument/diagnostic", 4);

        await _fake.RequestDiagnosticRefreshAsync();
        await _fake.WaitForAsync("textDocument/diagnostic", 6);
    }

    [Fact]
    public async Task WarmingPullInFlight_DoesNotDelayCompletion()
    {
        await InitializeAsync();
        _fake.DiagnosticGate = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        await DidOpenAsync(Uri("A.cs"), "class A {}");
        await _fake.WaitForAsync("textDocument/diagnostic");

        var sw = Stopwatch.StartNew();
        var result = await CompleteAsync(0, Uri("A.cs"));
        sw.Stop();

        Assert.Equal("Compute", result.GetProperty("items")[0].GetProperty("label").GetString());
        Assert.True(sw.ElapsedMilliseconds < 1000, $"completion waited {sw.ElapsedMilliseconds} ms behind the warming pull");
        _fake.DiagnosticGate.TrySetResult();
        await NextAsync(_diagnostics);
    }

    [Fact]
    public async Task DidClose_PublishesEmptyDiagnostics()
    {
        await InitializeAsync();
        var uri = Uri("A.cs");
        await DidOpenAsync(uri, "class A {}");
        await NextAsync(_diagnostics);

        await _client.NotifyWithParameterObjectAsync("textDocument/didClose", new { textDocument = new { uri } });

        var cleared = await NextAsync(_diagnostics);
        Assert.Equal(uri, cleared.GetProperty("uri").GetString());
        Assert.Equal(0, cleared.GetProperty("diagnostics").GetArrayLength());
    }

    [Fact]
    public async Task ServerExit_ReportsExitedAndFailsRequestsWithRequestFailed()
    {
        await InitializeAsync();
        var sln = await WriteSolutionAsync();
        await OpenAsync(sln);
        await _fake.WaitForAsync("solution/open");

        _fake.Crash();

        Assert.Equal("exited", (await NextAsync(_serverStatus, s => s.GetProperty("state").GetString() == "exited")).GetProperty("state").GetString());
        var failed = await NextAsync(_solutionStatus, s => s.GetProperty("state").GetString() == "failed");
        Assert.Equal("ELUDITE0002", failed.GetProperty("diagnostics")[0].GetProperty("code").GetString());
        var ex = await Assert.ThrowsAsync<RemoteInvocationException>(() => CompleteAsync(1));
        Assert.Equal(HostErrors.RequestFailed, ex.ErrorCode);
    }

    [Fact]
    public async Task FsprojOpen_IsLoadedWithoutHandingItToRoslyn()
    {
        // Brief 0057: Roslyn cannot load F#. A lone .fsproj is accepted, nothing is sent upstream for it, and the
        // status reaches loaded with the project counted; the server keeps running for the documents the shell opens.
        await InitializeAsync();
        var fsproj = Path.Combine(_dir.FullName, "Functional", "Functional.fsproj");
        Directory.CreateDirectory(Path.GetDirectoryName(fsproj)!);
        await File.WriteAllTextAsync(fsproj, """<Project Sdk="Microsoft.NET.Sdk"><ItemGroup><Compile Include="Library.fs" /></ItemGroup></Project>""", Ct);

        var generation = await OpenAsync(fsproj);

        var loading = await NextAsync(_solutionStatus, s => s.GetProperty("state").GetString() == "loading");
        Assert.Equal("projectLoad", loading.GetProperty("phase").GetString());
        var loaded = await NextAsync(_solutionStatus, s => s.GetProperty("state").GetString() == "loaded");
        Assert.Equal(generation, loaded.GetProperty("generation").GetInt64());
        Assert.Equal(fsproj, loaded.GetProperty("path").GetString());
        Assert.Equal(1, loaded.GetProperty("counts").GetProperty("projects").GetInt32());
        Assert.Equal(0, loaded.GetProperty("counts").GetProperty("legacyProjects").GetInt32());
        Assert.False(loaded.TryGetProperty("msbuild", out _));
        Assert.Contains("initialized", _fake.Snapshot());
        Assert.Equal(0, _fake.Count("project/open"));
        Assert.Equal(0, _fake.Count("solution/open"));
        Assert.Equal(0, _preparer.Calls);

        await DidOpenAsync(Uri("A.cs"), "class A {}");
        await _fake.WaitForAsync("textDocument/didOpen");
        Assert.Equal(0, _fake.Count("project/open"));
    }

    [Fact]
    public async Task VbprojOpen_IsHandedToRoslynAsAProject()
    {
        await InitializeAsync();
        var vbproj = Path.Combine(_dir.FullName, "Basic", "Basic.vbproj");
        Directory.CreateDirectory(Path.GetDirectoryName(vbproj)!);
        await File.WriteAllTextAsync(vbproj, """<Project Sdk="Microsoft.NET.Sdk" />""", Ct);

        var generation = await OpenAsync(vbproj);

        await _fake.WaitForAsync("project/open");
        Assert.Equal([new Uri(vbproj).AbsoluteUri], _fake.Last("project/open")!.Value.GetProperty("projects").EnumerateArray().Select(u => u.GetString()));
        await _fake.NotifyProjectsLoadedAsync();
        var loaded = await NextAsync(_solutionStatus, s => s.GetProperty("state").GetString() == "loaded");
        Assert.Equal(generation, loaded.GetProperty("generation").GetInt64());
        Assert.Equal(1, loaded.GetProperty("counts").GetProperty("projects").GetInt32());
    }

    [Fact]
    public async Task SolutionWithFsproj_IsHandedToRoslynWhole_AndCountsEveryProject()
    {
        await InitializeAsync();
        var fsproj = Path.Combine(_dir.FullName, "Functional", "Functional.fsproj");
        Directory.CreateDirectory(Path.GetDirectoryName(fsproj)!);
        await File.WriteAllTextAsync(fsproj, """<Project Sdk="Microsoft.NET.Sdk" />""", Ct);
        var sln = await WriteSolutionAsync();
        await File.WriteAllTextAsync(sln, """<Solution><Project Path="Lib/Lib.csproj" /><Project Path="Functional/Functional.fsproj" /></Solution>""", Ct);

        var generation = await OpenAsync(sln);

        await _fake.WaitForAsync("solution/open");
        Assert.Equal(new Uri(sln).AbsoluteUri, _fake.Last("solution/open")!.Value.GetProperty("solution").GetString());
        Assert.Equal(0, _fake.Count("project/open"));
        await _fake.NotifyProjectsLoadedAsync();
        var loaded = await NextAsync(_solutionStatus, s => s.GetProperty("state").GetString() == "loaded");
        Assert.Equal(generation, loaded.GetProperty("generation").GetInt64());
        Assert.Equal(2, loaded.GetProperty("counts").GetProperty("projects").GetInt32());
    }

    [Fact]
    public async Task EluditeMethodsStillWorkAndUnknownMethodsAreNotForwarded()
    {
        await InitializeAsync();

        var ping = await _client.InvokeWithCancellationAsync<JsonElement>("eludite/ping", [], Ct);
        Assert.True(ping.GetProperty("pong").GetBoolean());
        var ex = await Assert.ThrowsAsync<RemoteMethodNotFoundException>(
            () => _client.InvokeWithCancellationAsync<JsonElement>("textDocument/notAThing", [], Ct));
        Assert.Equal(JsonRpcErrorCode.MethodNotFound, ex.ErrorCode);
    }

    [Fact]
    public async Task ShutdownExit_ShutsDownUpstream()
    {
        await InitializeAsync();
        await NextAsync(_serverStatus, s => s.GetProperty("state").GetString() == "running");

        await _client.InvokeWithCancellationAsync<JsonElement>("eludite/host/shutdown", [], Ct);
        await _client.NotifyAsync("eludite/host/exit");

        Assert.Equal(0, await _server.WaitAsync(TimeSpan.FromSeconds(10), Ct));
        Assert.Contains("shutdown", _fake.Snapshot());
        Assert.True(_fake.Disposed);
    }

    public async ValueTask DisposeAsync()
    {
        _client.Dispose();
        await _server.WaitAsync(TimeSpan.FromSeconds(10));
        try
        {
            _dir.Delete(recursive: true);
        }
        catch (IOException)
        {
        }
    }

    private sealed class FakePreparer : ISolutionPreparer
    {
        private int _calls;

        public int Calls => Volatile.Read(ref _calls);

        public Task<SolutionPreparation> PrepareAsync(string solutionPath, CancellationToken cancellationToken)
        {
            Interlocked.Increment(ref _calls);
            return Task.FromResult(new SolutionPreparation(
                new MsBuildInfo("mono", "/usr/lib/mono/msbuild/Current/bin/MSBuild.dll", "system"),
                [new Correction("caseFixups", "/src/Lib.csproj", 1)],
                [new HostDiagnostic("warning", "ELUDITE0106", "case") { Project = "/src/Lib.csproj" }],
                1));
        }
    }
}
