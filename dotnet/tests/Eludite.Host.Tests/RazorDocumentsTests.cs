using System.Text.Json;
using System.Threading.Channels;
using Microsoft.Extensions.Time.Testing;
using Nerdbank.Streams;
using Eludite.Host.Lsp;
using Eludite.Host.Rpc;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0079: the host's side of Razor cohosting against the fake language server: <c>razor/updateHtml</c>
/// projections, Razor's HTML requests relayed to the shell as <c>eludite/razor/html</c>, the <c>null</c> answers, and
/// <c>razor/documentClosed</c>.
/// </summary>
public sealed class RazorDocumentsTests : IAsyncDisposable
{
    private const string RazorUri = "file:///w/Components/Pages/Home.razor";

    private readonly FakeLanguageServer _fake = new();
    private readonly LspProxy _proxy;
    private readonly Task<int> _server;
    private readonly JsonRpc _client;
    private readonly Channel<JsonElement> _htmlRequests = Channel.CreateUnbounded<JsonElement>();

    /// <summary>How the test's shell answers <c>eludite/razor/html</c>.</summary>
    private Func<JsonElement, CancellationToken, Task<JsonElement>> _answer =
        (_, _) => Task.FromResult(JsonSerializer.SerializeToElement(new { isIncomplete = false, items = new[] { new { label = "div" } } }));

    public RazorDocumentsTests()
    {
        _proxy = new LspProxy(_fake, TextWriter.Null, timeProvider: new FakeTimeProvider());
        var (clientStream, serverStream) = FullDuplexStream.CreatePair();
        var target = new HostRpcTarget(new FakeSdkDiscoverer(), TextWriter.Null, languageServer: _proxy);
        _server = HostServer.RunAsync(serverStream, serverStream, target);
        _client = TestRpc.Create(clientStream);
        var html = new Func<JsonElement, CancellationToken, Task<JsonElement>>((p, ct) =>
        {
            _htmlRequests.Writer.TryWrite(p.Clone());
            return _answer(p, ct);
        });
        _client.AddLocalRpcMethod(html.Method, html.Target, new JsonRpcMethodAttribute(RazorDocuments.ShellHtmlRequest) { UseSingleObjectParameterDeserialization = true });
        _client.StartListening();
    }

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private async Task StartAsync()
    {
        await _client.InvokeWithParameterObjectAsync<JsonElement>("eludite/host/initialize", new { clientName = "t", clientVersion = "0" }, Ct);
        await _fake.WaitForAsync("initialized");
    }

    private Task<JsonElement> PublishAsync(string checksum, string text, string uri = RazorUri) =>
        _fake.InvokeHostAsync(RazorDocuments.UpdateHtml, new { textDocument = new { uri }, checksum, text }, Ct);

    private Task<JsonElement> HtmlRequestAsync(string method, string checksum, string uri = RazorUri) =>
        _fake.InvokeHostAsync(method, new
        {
            textDocument = new { uri },
            checksum,
            request = new { textDocument = new { uri }, position = new { line = 0, character = 1 }, context = new { triggerKind = 1 } },
        }, Ct);

    [Fact]
    public void IsRazor_NamesRazorAndCshtmlDocumentsOnly()
    {
        Assert.True(RazorDocuments.IsRazor("file:///w/Counter.razor"));
        Assert.True(RazorDocuments.IsRazor("file:///w/Views/Index.CSHTML"));
        Assert.True(RazorDocuments.IsRazor("file:///w/Index.cshtml?x=1"));
        Assert.False(RazorDocuments.IsRazor("file:///w/Counter.razor.cs"));
        Assert.False(RazorDocuments.IsRazor("file:///w/Program.cs"));
        Assert.False(RazorDocuments.IsRazor(null));
    }

    [Fact]
    public async Task UpdateHtml_KeepsTheLatestProjectionAndAnswersNull()
    {
        await StartAsync();

        var answer = await PublishAsync("c1", "<h1>     </h1>");
        Assert.Equal(JsonValueKind.Null, answer.ValueKind);
        await PublishAsync("c2", "<h2>     </h2>");

        Assert.Equal(new RazorDocuments.Projection("c2", "<h2>     </h2>"), _proxy.Razor.Get(RazorUri));
    }

    [Fact]
    public async Task HtmlRequest_IsRelayedToTheShellWithTheProjectionAndTheShellsResultReturned()
    {
        await StartAsync();
        await PublishAsync("c1", "<h1>     </h1>");

        var answer = await HtmlRequestAsync("textDocument/completion", "c1");

        Assert.Equal("div", answer.GetProperty("items")[0].GetProperty("label").GetString());
        var relayed = await _htmlRequests.Reader.ReadAsync(Ct);
        Assert.Equal("textDocument/completion", relayed.GetProperty("method").GetString());
        Assert.Equal(RazorUri, relayed.GetProperty("uri").GetString());
        Assert.Equal("c1", relayed.GetProperty("checksum").GetString());
        Assert.Equal("<h1>     </h1>", relayed.GetProperty("text").GetString());
        Assert.Equal(_proxy.Generation, relayed.GetProperty(LspProxy.GenerationProperty).GetInt64());
        Assert.Equal(RazorUri, relayed.GetProperty("params").GetProperty("textDocument").GetProperty("uri").GetString());
        Assert.Equal(1, relayed.GetProperty("params").GetProperty("position").GetProperty("character").GetInt32());
        Assert.Equal(1, _proxy.Razor.Relayed);
    }

    [Fact]
    public async Task HtmlRequest_WithoutTheProjectionOrWithAnotherChecksum_AnswersNullWithoutAskingTheShell()
    {
        await StartAsync();

        var none = await HtmlRequestAsync("textDocument/hover", "c1");
        await PublishAsync("c2", "<p></p>");
        var other = await HtmlRequestAsync("textDocument/hover", "c1");

        Assert.Equal(JsonValueKind.Null, none.ValueKind);
        Assert.Equal(JsonValueKind.Null, other.ValueKind);
        Assert.Equal(0, _proxy.Razor.Relayed);
        Assert.False(_htmlRequests.Reader.TryRead(out _));
    }

    [Fact]
    public async Task HtmlRequest_ShellErrorAnswersNull()
    {
        await StartAsync();
        await PublishAsync("c1", "<p></p>");
        _answer = (_, _) => throw new LocalRpcException("no html server") { ErrorCode = -32603 };

        var answer = await HtmlRequestAsync("textDocument/formatting", "c1");

        Assert.Equal(JsonValueKind.Null, answer.ValueKind);
        Assert.Equal(1, _proxy.Razor.Relayed);
    }

    [Fact]
    public async Task HtmlRequest_AGenerationChangeWhileInFlightAnswersNull()
    {
        await StartAsync();
        await PublishAsync("c1", "<p></p>");
        var held = new TaskCompletionSource<JsonElement>(TaskCreationOptions.RunContinuationsAsynchronously);
        _answer = (_, ct) => held.Task.WaitAsync(ct);

        var pending = HtmlRequestAsync("textDocument/hover", "c1");
        await _htmlRequests.Reader.ReadAsync(Ct);
        _proxy.BumpGeneration();
        var answer = await pending.WaitAsync(TimeSpan.FromSeconds(10), Ct);

        Assert.Equal(JsonValueKind.Null, answer.ValueKind);
    }

    [Fact]
    public async Task DidClose_OfARazorDocument_SendsRazorDocumentClosedAndForgetsItsProjection()
    {
        await StartAsync();
        await PublishAsync("c1", "<p></p>");
        await _client.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri = RazorUri, languageId = "aspnetcorerazor", version = 1, text = "<p>@x</p>" } });
        await _client.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri = "file:///w/A.cs", languageId = "csharp", version = 1, text = "class A {}" } });

        await _client.NotifyWithParameterObjectAsync("textDocument/didClose", new { textDocument = new { uri = "file:///w/A.cs" } });
        await _client.NotifyWithParameterObjectAsync("textDocument/didClose", new { textDocument = new { uri = RazorUri } });
        await _fake.WaitForAsync("razor/documentClosed");
        await _fake.WaitForAsync("textDocument/didClose", 2);

        Assert.Equal(1, _fake.Count("razor/documentClosed"));
        Assert.Equal(RazorUri, _fake.Last("razor/documentClosed")!.Value.GetProperty("uri").GetString());
        var order = _fake.Snapshot().Where(m => m is "textDocument/didClose" or "razor/documentClosed").ToList();
        Assert.Equal(["textDocument/didClose", "textDocument/didClose", "razor/documentClosed"], order);
        Assert.Equal("aspnetcorerazor", _fake.Messages().First(m => m.Method == "textDocument/didOpen").Params.GetProperty("textDocument").GetProperty("languageId").GetString());
        Assert.Null(_proxy.Razor.Get(RazorUri));
    }

    [Fact]
    public async Task RazorForwardedRequests_ReachTheLanguageServer()
    {
        await StartAsync();

        foreach (var method in RazorDocuments.ForwardedRequests)
        {
            // The fake does not implement them, so the forwarded call fails upstream; the host itself knew the method
            // (its own MethodNotFound is -32601).
            var (code, _) = await TestRpc.ErrorOfAsync(() => _client.InvokeWithParameterObjectAsync<JsonElement>(
                method, new { textDocument = new { uri = RazorUri }, eluditeGeneration = _proxy.Generation }, Ct));
            Assert.NotEqual(-32601, code);
        }

        var (unknown, _) = await TestRpc.ErrorOfAsync(() => _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/unknownThing", new { textDocument = new { uri = RazorUri }, eluditeGeneration = _proxy.Generation }, Ct));
        Assert.Equal(-32601, unknown);
    }

    public async ValueTask DisposeAsync()
    {
        _client.Dispose();
        await _server.WaitAsync(TimeSpan.FromSeconds(10));
        await _proxy.DisposeAsync();
    }
}
