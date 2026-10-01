using System.Diagnostics;
using System.Text.Json;
using System.Threading.Channels;
using Eludite.Host.Lsp;
using Eludite.Host.Rpc;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0002 and 0007 proving test: drives the real eludite-host process, with the Roslyn language server built by
/// tools/roslyn-pin, against the generated 200-project solution from bench/roslyn-200, through the bridge contract
/// (protocol/schemas/host-rpc.md): renamed lifecycle, solution generations, host-side semantics warming.
/// Skipped (with a message) when either is absent, so a fresh clone stays green.
/// </summary>
public sealed class RoslynIntegrationTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    [Fact]
    public async Task Host_LoadsGeneratedSolution_AndServesCompletionSixLayersDeep()
    {
        var roslyn = RoslynProcessLauncher.Locate(null);
        Assert.SkipWhen(roslyn is null, "Roslyn language server not built; run tools/roslyn-pin/build.sh (or set ELUDITE_ROSLYN_LS).");
        var probePath = FindProbe();
        Assert.SkipWhen(probePath is null, "Generated solution not found; run bench/roslyn-200/run.sh --prepare-only (or set ELUDITE_BENCH_PROBE).");
        using var probe = JsonDocument.Parse(await File.ReadAllTextAsync(probePath!, Ct));
        var p = probe.RootElement;
        var file = p.GetProperty("file").GetString()!;
        var assets = Path.Combine(Path.GetDirectoryName(file)!, "obj", "project.assets.json");
        Assert.SkipWhen(!File.Exists(assets), $"Generated solution is not restored ({assets} missing); run bench/roslyn-200/run.sh --prepare-only.");

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
            var published = Channel.CreateUnbounded<(JsonElement Params, bool AfterLoad)>();
            TestRpc.On(rpc, "eludite/solution/status", s =>
            {
                if (s.GetProperty("state").GetString() is "loaded" or "failed")
                {
                    loaded.TrySetResult(s.Clone());
                }
            });
            TestRpc.On(rpc, "textDocument/publishDiagnostics", d => published.Writer.TryWrite((d.Clone(), loaded.Task.IsCompleted)));
            rpc.StartListening();

            var init = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
                "eludite/host/initialize", new { clientName = "integration-test", clientVersion = "0" }, Ct);
            Assert.Equal("eludite-host", init.GetProperty("hostName").GetString());
            Assert.True(init.GetProperty("capabilities").GetProperty("languageServer").GetBoolean());
            var ping = await rpc.InvokeWithCancellationAsync<JsonElement>("eludite/ping", [], Ct);
            Assert.True(ping.GetProperty("pong").GetBoolean());
            var info = await rpc.InvokeWithCancellationAsync<JsonElement>("eludite/host/info", [], Ct);
            Assert.Equal(JsonValueKind.Array, info.GetProperty("dotnetSdks").ValueKind);
            var opened = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
                "eludite/solution/open", new { path = p.GetProperty("solution").GetString() }, Ct);
            var generation = opened.GetProperty("generation").GetInt64();
            Assert.Equal(1, generation);

            var uri = new Uri(file).AbsoluteUri;
            var text = await File.ReadAllTextAsync(file, Ct);
            await rpc.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri, languageId = "csharp", version = 1, text } });
            var status = await loaded.Task.WaitAsync(TimeSpan.FromMinutes(5), Ct);
            Assert.Equal("loaded", status.GetProperty("state").GetString());
            Assert.Equal(generation, status.GetProperty("generation").GetInt64());
            Assert.Equal(200, status.GetProperty("counts").GetProperty("projects").GetInt32());

            // Brief 0002's failure case: the shell sends no diagnostic pull of its own. The host's warming pull after
            // the load (published as textDocument/publishDiagnostics) is what gives Roslyn's frozen-partial completion
            // the referenced projects' compilations, so the very first completion on a type from a project six
            // references away must list its members.
            using (var wait = CancellationTokenSource.CreateLinkedTokenSource(Ct))
            {
                wait.CancelAfter(TimeSpan.FromMinutes(2));
                while (true)
                {
                    var (d, afterLoad) = await published.Reader.ReadAsync(wait.Token);
                    if (afterLoad && d.GetProperty("uri").GetString() == uri)
                    {
                        Assert.Equal(1, d.GetProperty("version").GetInt32());
                        Assert.Equal(generation, d.GetProperty("eluditeGeneration").GetInt64());
                        break;
                    }
                }
            }

            var labels = await CompleteAsync(rpc, uri, p, generation);
            Assert.Contains(p.GetProperty("expectLabel").GetString(), labels);

            // A new document version keeps working without waiting for its own (debounced) warming pull.
            await rpc.NotifyWithParameterObjectAsync("textDocument/didChange", new { textDocument = new { uri, version = 2 }, contentChanges = new[] { new { text = text + "// edit\n" } } });
            Assert.Contains(p.GetProperty("expectLabel").GetString(), await CompleteAsync(rpc, uri, p, generation));

            // A stale generation is rejected without being forwarded.
            var (staleCode, staleData) = await TestRpc.ErrorOfAsync(() => CompleteAsync(rpc, uri, p, generation - 1));
            Assert.Equal(HostErrors.ContentModified, staleCode);
            Assert.Equal(generation, staleData!.Value.GetProperty("currentGeneration").GetInt64());

            // A canceled request (a diagnostic pull on a new document version) comes back within 50 ms, as an error.
            await rpc.NotifyWithParameterObjectAsync("textDocument/didChange", new { textDocument = new { uri, version = 3 }, contentChanges = new[] { new { text = text + "// edit 2\n" } } });
            using var cts = CancellationTokenSource.CreateLinkedTokenSource(Ct);
            var slow = rpc.InvokeWithParameterObjectAsync<JsonElement>("textDocument/diagnostic", new { textDocument = new { uri }, eluditeGeneration = generation }, cts.Token);
            var sw = Stopwatch.StartNew();
            await cts.CancelAsync();
            await Assert.ThrowsAnyAsync<OperationCanceledException>(() => slow);
            Assert.True(sw.ElapsedMilliseconds < 50, $"cancellation took {sw.ElapsedMilliseconds} ms");

            await rpc.InvokeWithCancellationAsync<JsonElement>("eludite/host/shutdown", [], Ct);
            await rpc.NotifyAsync("eludite/host/exit");
            await host.WaitForExitAsync(Ct).WaitAsync(TimeSpan.FromSeconds(30), Ct);
            Assert.Equal(0, host.ExitCode);
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

    private static async Task<List<string?>> CompleteAsync(JsonRpc rpc, string uri, JsonElement probe, long generation)
    {
        var completion = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/completion",
            new
            {
                textDocument = new { uri },
                position = new { line = probe.GetProperty("line").GetInt32(), character = probe.GetProperty("character").GetInt32() },
                context = new { triggerKind = 2, triggerCharacter = "." },
                eluditeGeneration = generation,
            },
            Ct);
        Assert.Equal(JsonValueKind.Object, completion.ValueKind);
        return completion.GetProperty("items").EnumerateArray().Select(i => i.GetProperty("label").GetString()).ToList();
    }

    private static string? FindProbe()
    {
        var fromEnv = Environment.GetEnvironmentVariable("ELUDITE_BENCH_PROBE");
        if (!string.IsNullOrEmpty(fromEnv))
        {
            return File.Exists(fromEnv) ? fromEnv : null;
        }

        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, "bench", "roslyn-200", "out", "probe.json");
            if (File.Exists(candidate))
            {
                return candidate;
            }
        }

        return null;
    }
}
