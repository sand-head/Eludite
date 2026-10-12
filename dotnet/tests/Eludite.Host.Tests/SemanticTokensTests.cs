using System.Text.Json;
using System.Threading.Channels;
using Microsoft.Extensions.Time.Testing;
using Nerdbank.Streams;
using Eludite.Host.Lsp;
using Eludite.Host.Rpc;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0066: semantic tokens and inlay hints forwarded and typed, the capabilities and configuration that turn them
/// on in Roslyn, and <c>workspace/semanticTokens/refresh</c> and <c>workspace/inlayHint/refresh</c> relayed, against
/// the in-memory fake language server.
/// </summary>
public sealed class SemanticTokensTests : IAsyncDisposable
{
    private readonly FakeLanguageServer _fake = new();
    private readonly LspProxy _proxy;
    private readonly Task<int> _server;
    private readonly JsonRpc _client;
    private readonly Channel<(string Method, JsonElement Params)> _refreshes = Channel.CreateUnbounded<(string, JsonElement)>();

    public SemanticTokensTests()
    {
        _proxy = new LspProxy(_fake, TextWriter.Null, preparer: null, timeProvider: new FakeTimeProvider());
        var (clientStream, serverStream) = FullDuplexStream.CreatePair();
        var target = new HostRpcTarget(new FakeSdkDiscoverer(), TextWriter.Null, languageServer: _proxy);
        _server = HostServer.RunAsync(serverStream, serverStream, target);
        _client = TestRpc.Create(clientStream);
        foreach (var method in new[] { LspProxy.ShellSemanticTokensRefresh, LspProxy.ShellInlayHintRefresh })
        {
            TestRpc.On(_client, method, p => _refreshes.Writer.TryWrite((method, p.Clone())));
        }

        _client.StartListening();
    }

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private const string Uri = "file:///work/CalculatorTests.cs";

    private async Task InitializeAsync()
    {
        await _client.InvokeWithParameterObjectAsync<JsonElement>("eludite/host/initialize", new { clientName = "t", clientVersion = "0" }, Ct);
        await _fake.WaitForAsync("initialize");
        await _client.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri = Uri, languageId = "csharp", version = 1, text = CodeLensTests.TestsText } });
        await _fake.WaitForAsync("textDocument/didOpen");
    }

    [Fact]
    public async Task SemanticTokensAndInlayHints_AreTypedAndForwardedWithoutTheGeneration()
    {
        await InitializeAsync();
        foreach (var method in new[]
                 {
                     "textDocument/semanticTokens/full", "textDocument/semanticTokens/full/delta",
                     "textDocument/semanticTokens/range", "textDocument/inlayHint", "inlayHint/resolve",
                 })
        {
            Assert.Contains(method, LspProxy.TypedRequests);
            Assert.DoesNotContain(method, LspProxy.UntypedRequests);
        }

        var doc = new { uri = Uri };
        var range = new { start = new { line = 0, character = 0 }, end = new { line = 10, character = 0 } };
        var full = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/semanticTokens/full", new { textDocument = doc, eluditeGeneration = 0 }, Ct);
        Assert.Equal("1", full.GetProperty("resultId").GetString());
        Assert.Equal(10, full.GetProperty("data").GetArrayLength());
        Assert.False(_fake.Last("textDocument/semanticTokens/full")!.Value.TryGetProperty(LspProxy.GenerationProperty, out _));

        var delta = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/semanticTokens/full/delta", new { textDocument = doc, previousResultId = "1", eluditeGeneration = 0 }, Ct);
        Assert.Equal(10, delta.GetProperty("edits")[0].GetProperty("start").GetInt32());
        Assert.Equal("1", _fake.Last("textDocument/semanticTokens/full/delta")!.Value.GetProperty("previousResultId").GetString());

        var ranged = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/semanticTokens/range", new { textDocument = doc, range, eluditeGeneration = 0 }, Ct);
        Assert.Equal(5, ranged.GetProperty("data").GetArrayLength());

        var hints = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/inlayHint", new { textDocument = doc, range, eluditeGeneration = 0 }, Ct);
        Assert.Equal("count:", hints[0].GetProperty("label").GetString());
        var hint = hints[0];
        var resolved = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "inlayHint/resolve",
            new { position = hint.GetProperty("position"), label = hint.GetProperty("label"), kind = 2, data = hint.GetProperty("data"), eluditeGeneration = 0 },
            Ct);
        Assert.Equal("int count", resolved.GetProperty("tooltip").GetString());
        Assert.Equal(0, _fake.Last("inlayHint/resolve")!.Value.GetProperty("data").GetProperty("listIndex").GetInt32());
    }

    [Fact]
    public void Validation_RequiresTheDocumentOrThePosition()
    {
        var bare = JsonSerializer.SerializeToElement(new { });
        Assert.NotNull(LspProxy.ValidateTyped("textDocument/semanticTokens/full", bare));
        Assert.NotNull(LspProxy.ValidateTyped("textDocument/inlayHint", bare));
        Assert.NotNull(LspProxy.ValidateTyped("inlayHint/resolve", bare));
        Assert.Null(LspProxy.ValidateTyped("inlayHint/resolve", JsonSerializer.SerializeToElement(new { position = new { line = 0, character = 0 }, label = "x" })));
    }

    [Fact]
    public async Task ClientCapabilities_AdvertiseTokensHintsTagsAndTheirRefreshes()
    {
        await InitializeAsync();

        var caps = _fake.Last("initialize")!.Value.GetProperty("capabilities");
        var td = caps.GetProperty("textDocument");
        var tokens = td.GetProperty("semanticTokens");
        Assert.True(tokens.GetProperty("requests").GetProperty("full").GetProperty("delta").GetBoolean());
        Assert.True(tokens.GetProperty("requests").GetProperty("range").GetBoolean());
        Assert.Equal("relative", tokens.GetProperty("formats")[0].GetString());
        Assert.True(tokens.GetProperty("augmentsSyntaxTokens").GetBoolean());
        Assert.Equal(JsonValueKind.Object, td.GetProperty("inlayHint").GetProperty("resolveSupport").ValueKind);
        Assert.Equal(2, td.GetProperty("publishDiagnostics").GetProperty("tagSupport").GetProperty("valueSet").GetArrayLength());
        Assert.True(caps.GetProperty("workspace").GetProperty("semanticTokens").GetProperty("refreshSupport").GetBoolean());
        Assert.True(caps.GetProperty("workspace").GetProperty("inlayHint").GetProperty("refreshSupport").GetBoolean());
    }

    [Fact]
    public void Configuration_TurnsRoslynsInlayHintsOnForCSharpAndVisualBasic()
    {
        var items = new List<object> { new { section = "projects.dotnet_enable_file_based_programs" }, new { section = "other.thing" } };
        foreach (var language in new[] { "csharp", "visual_basic" })
        {
            items.Add(new { section = $"{language}|inlay_hints.dotnet_enable_inlay_hints_for_parameters" });
            items.Add(new { section = $"{language}|inlay_hints.csharp_enable_inlay_hints_for_types" });
            items.Add(new { section = $"{language}|inlay_hints.dotnet_suppress_inlay_hints_for_parameters_that_match_argument_name" });
        }

        var answer = LspProxy.AnswerConfiguration(JsonSerializer.SerializeToElement(new { items }));
        Assert.False(answer[0].GetBoolean());
        Assert.Equal(JsonValueKind.Null, answer[1].ValueKind);
        for (var i = 2; i < 8; i += 3)
        {
            Assert.True(answer[i].GetBoolean());
            Assert.True(answer[i + 1].GetBoolean());
            // Roslyn's suppressions keep their defaults.
            Assert.Equal(JsonValueKind.Null, answer[i + 2].ValueKind);
        }

        Assert.Equal(9, LspProxy.InlayHintOptions.Length);
    }

    [Fact]
    public async Task Refreshes_AreAnsweredAndRelayedWithTheGeneration()
    {
        await InitializeAsync();
        var generation = _proxy.Generation;

        foreach (var (method, shell) in new[]
                 {
                     ("workspace/semanticTokens/refresh", LspProxy.ShellSemanticTokensRefresh),
                     ("workspace/inlayHint/refresh", LspProxy.ShellInlayHintRefresh),
                 })
        {
            await _fake.RequestAsync(method).WaitAsync(TimeSpan.FromSeconds(10), Ct);
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(Ct);
            timeout.CancelAfter(TimeSpan.FromSeconds(10));
            var relayed = await _refreshes.Reader.ReadAsync(timeout.Token);
            Assert.Equal(shell, relayed.Method);
            Assert.Equal(generation, relayed.Params.GetProperty(LspProxy.GenerationProperty).GetInt64());
        }
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
