using System.Diagnostics;
using System.Text.Json;
using Niello.Host.Lsp;
using Niello.Host.Rpc;
using StreamJsonRpc;

namespace Niello.Host.Tests;

/// <summary>
/// Brief 0002 proving test: drives the real niello-host process, with the Roslyn language server built by
/// tools/roslyn-pin, against the generated 200-project solution from bench/roslyn-200.
/// Skipped (with a message) when either is absent, so a fresh clone stays green.
/// </summary>
public sealed class RoslynIntegrationTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    [Fact]
    public async Task Host_LoadsGeneratedSolution_AndServesCompletionSixLayersDeep()
    {
        var roslyn = RoslynProcessLauncher.Locate(null);
        Assert.SkipWhen(roslyn is null, "Roslyn language server not built; run tools/roslyn-pin/build.sh (or set NIELLO_ROSLYN_LS).");
        var probePath = FindProbe();
        Assert.SkipWhen(probePath is null, "Generated solution not found; run bench/roslyn-200/run.sh --prepare-only (or set NIELLO_BENCH_PROBE).");
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
        psi.ArgumentList.Add(Path.Combine(AppContext.BaseDirectory, "niello-host.dll"));
        psi.ArgumentList.Add("--stdio");
        psi.ArgumentList.Add("--roslyn-ls");
        psi.ArgumentList.Add(roslyn!);
        using var host = Process.Start(psi)!;
        var stderr = host.StandardError.ReadToEndAsync(Ct);
        try
        {
            using var rpc = HostServer.CreateConnection(host.StandardInput.BaseStream, host.StandardOutput.BaseStream);
            var loaded = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
            rpc.AddLocalRpcMethod("workspace/projectInitializationComplete", new Action(() => loaded.TrySetResult()));
            rpc.StartListening();

            var init = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
                "initialize", new { clientName = "integration-test", clientVersion = "0", solutionPath = p.GetProperty("solution").GetString() }, Ct);
            Assert.Equal("niello-host", init.GetProperty("hostName").GetString());
            var ping = await rpc.InvokeWithCancellationAsync<JsonElement>("niello/ping", [], Ct);
            Assert.True(ping.GetProperty("pong").GetBoolean());
            var info = await rpc.InvokeWithCancellationAsync<JsonElement>("niello/host/info", [], Ct);
            Assert.Equal(JsonValueKind.Array, info.GetProperty("dotnetSdks").ValueKind);

            var uri = new Uri(file).AbsoluteUri;
            var text = await File.ReadAllTextAsync(file, Ct);
            await rpc.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri, languageId = "csharp", version = 1, text } });
            await loaded.Task.WaitAsync(TimeSpan.FromMinutes(5), Ct);

            // Editor behavior: pull diagnostics for the open document once the solution is loaded. Roslyn's
            // completion runs on frozen-partial semantics; this pull is what compiles the dependency chain.
            await rpc.NotifyWithParameterObjectAsync("textDocument/didChange", new { textDocument = new { uri, version = 2 }, contentChanges = new[] { new { text } } });
            await rpc.InvokeWithParameterObjectAsync<JsonElement>("textDocument/diagnostic", new { textDocument = new { uri } }, Ct);

            var completion = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
                "textDocument/completion",
                new
                {
                    textDocument = new { uri },
                    position = new { line = p.GetProperty("line").GetInt32(), character = p.GetProperty("character").GetInt32() },
                    context = new { triggerKind = 2, triggerCharacter = "." },
                },
                Ct);
            Assert.Equal(JsonValueKind.Object, completion.ValueKind);
            var labels = completion.GetProperty("items").EnumerateArray().Select(i => i.GetProperty("label").GetString()).ToList();
            Assert.Contains(p.GetProperty("expectLabel").GetString(), labels);

            // A canceled request (a diagnostic pull on a new document version) comes back within 50 ms, as an error.
            await rpc.NotifyWithParameterObjectAsync("textDocument/didChange", new { textDocument = new { uri, version = 3 }, contentChanges = new[] { new { text = text + "// edit\n" } } });
            using var cts = CancellationTokenSource.CreateLinkedTokenSource(Ct);
            var slow = rpc.InvokeWithParameterObjectAsync<JsonElement>("textDocument/diagnostic", new { textDocument = new { uri } }, cts.Token);
            var sw = Stopwatch.StartNew();
            await cts.CancelAsync();
            await Assert.ThrowsAnyAsync<OperationCanceledException>(() => slow);
            Assert.True(sw.ElapsedMilliseconds < 50, $"cancellation took {sw.ElapsedMilliseconds} ms");

            await rpc.InvokeWithCancellationAsync<JsonElement>("shutdown", [], Ct);
            await rpc.NotifyAsync("exit");
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

    private static string? FindProbe()
    {
        var fromEnv = Environment.GetEnvironmentVariable("NIELLO_BENCH_PROBE");
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
