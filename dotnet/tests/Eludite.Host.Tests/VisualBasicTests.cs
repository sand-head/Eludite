using System.Diagnostics;
using System.Text.Json;
using System.Threading.Channels;
using Eludite.Host.Lsp;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0057's proving test for Visual Basic (PLAN.md section 4.3, Spike 2): the pinned Roslyn language server, built
/// by tools/roslyn-pin with vb.patch so its MEF composition holds the Visual Basic feature assemblies, through the real
/// eludite-host process on <c>corpus/projects/VisualBasic</c>. The <c>.vbproj</c> is opened with
/// <c>eludite/solution/open</c>, <c>Program.vb</c> with <c>textDocument/didOpen</c> and the language id <c>vb</c>, and
/// Roslyn answers hover, completion, a diagnostic (BC30512), go to definition and rename. Without the patch the server
/// drops the project (no <c>ICommandLineParserService</c> for Visual Basic) and none of these come back. Skipped with
/// a message when the language server is not built.
/// </summary>
public sealed class VisualBasicTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    [Fact]
    public async Task RealRoslyn_ServesHoverCompletionDiagnosticsDefinitionAndRenameOnAVisualBasicProject()
    {
        var roslyn = RoslynProcessLauncher.Locate(null);
        Assert.SkipWhen(roslyn is null, "Roslyn language server not built; run tools/roslyn-pin/build.sh (or set ELUDITE_ROSLYN_LS).");

        using var corpus = new ProjectCorpus();
        var project = corpus.Path("VisualBasic/VisualBasic.vbproj");
        var program = corpus.Path("VisualBasic/Program.vb");
        var text = await File.ReadAllTextAsync(program, Ct);
        var lines = text.Split('\n');
        int LineOf(string marker) => Array.FindIndex(lines, l => l.Contains(marker, StringComparison.Ordinal)) is var line and >= 0
            ? line
            : throw new InvalidOperationException($"Program.vb has no line containing {marker}");
        var propertyLine = LineOf("Public Property Name As String");
        var propertyCharacter = lines[propertyLine].IndexOf("Name", StringComparison.Ordinal);
        var callLine = LineOf("Console.WriteLine(greeter.Greet())");
        var callCharacter = lines[callLine].IndexOf("Console.", StringComparison.Ordinal) + "Console.".Length;
        var errorLine = LineOf("Dim n As Integer = \"x\"");
        var classLine = LineOf("Public Class Greeter");
        var constructorLine = LineOf("Public Sub New(name As String)");
        var newLine = LineOf("Dim greeter As New Greeter(");
        var newCharacter = lines[newLine].IndexOf("Greeter(", StringComparison.Ordinal);

        // Roslyn loads the project through its MSBuild build host, which needs the restore's assets file.
        await RestoreAsync(project);

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
            var published = Channel.CreateUnbounded<JsonElement>();
            TestRpc.On(rpc, "eludite/solution/status", s =>
            {
                if (s.GetProperty("state").GetString() is "loaded" or "failed")
                {
                    loaded.TrySetResult(s.Clone());
                }
            });
            TestRpc.On(rpc, "textDocument/publishDiagnostics", d => published.Writer.TryWrite(d.Clone()));
            rpc.StartListening();

            await rpc.InvokeWithParameterObjectAsync<JsonElement>("eludite/host/initialize", new { clientName = "vb-test", clientVersion = "0" }, Ct);
            var opened = await rpc.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/open", new { path = project }, Ct);
            var generation = opened.GetProperty("generation").GetInt64();
            var uri = new Uri(program).AbsoluteUri;
            await rpc.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri, languageId = "vb", version = 1, text } });

            var status = await loaded.Task.WaitAsync(TimeSpan.FromMinutes(5), Ct);
            Assert.Equal("loaded", status.GetProperty("state").GetString());
            Assert.Equal(1, status.GetProperty("counts").GetProperty("projects").GetInt32());

            // Diagnostics: the Option Strict error on its line, by pull (the host's own warming pull publishes the same
            // items). Roslyn's project load finishes after the status, so the pull is repeated until the error shows.
            var watch = Stopwatch.StartNew();
            JsonElement? error = null;
            var pulls = 0;
            using (var wait = CancellationTokenSource.CreateLinkedTokenSource(Ct))
            {
                wait.CancelAfter(TimeSpan.FromMinutes(3));
                while (error is null)
                {
                    pulls++;
                    var report = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
                        "textDocument/diagnostic", new { textDocument = new { uri }, eluditeGeneration = generation }, wait.Token);
                    if (report.ValueKind == JsonValueKind.Object && report.TryGetProperty("items", out var items))
                    {
                        error = items.EnumerateArray().Select(i => (JsonElement?)i).FirstOrDefault(i => CodeOf(i!.Value) == "BC30512");
                    }

                    if (error is null)
                    {
                        await Task.Delay(500, wait.Token);
                    }
                }
            }

            var diagnosticsMs = watch.Elapsed.TotalMilliseconds;
            Assert.Equal(errorLine, error!.Value.GetProperty("range").GetProperty("start").GetProperty("line").GetInt32());
            Assert.Equal(1, error.Value.GetProperty("severity").GetInt32());
            Assert.Contains("Option Strict On", error.Value.GetProperty("message").GetString());

            // Hover on the property's name reads as Visual Basic.
            watch.Restart();
            var hover = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
                "textDocument/hover",
                new { textDocument = new { uri }, position = new { line = propertyLine, character = propertyCharacter }, eluditeGeneration = generation },
                Ct);
            var hoverMs = watch.Elapsed.TotalMilliseconds;
            Assert.Equal(JsonValueKind.Object, hover.ValueKind);
            var hoverText = TextOf(hover.GetProperty("contents"));
            Assert.Contains("Name", hoverText);
            Assert.True(hoverText.Contains("Property", StringComparison.Ordinal) || hoverText.Contains("As String", StringComparison.Ordinal), $"hover reads {hoverText}");

            // Completion after "Console." lists WriteLine.
            watch.Restart();
            var completion = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
                "textDocument/completion",
                new
                {
                    textDocument = new { uri },
                    position = new { line = callLine, character = callCharacter },
                    context = new { triggerKind = 2, triggerCharacter = "." },
                    eluditeGeneration = generation,
                },
                Ct);
            var completionMs = watch.Elapsed.TotalMilliseconds;
            Assert.Equal(JsonValueKind.Object, completion.ValueKind);
            var labels = completion.GetProperty("items").EnumerateArray().Select(i => i.GetProperty("label").GetString()).ToList();
            Assert.Contains("WriteLine", labels);

            // Go to definition from "New Greeter(" lands in the same document, on the constructor (Roslyn's target for
            // an object creation) or the class.
            watch.Restart();
            var definition = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
                "textDocument/definition",
                new { textDocument = new { uri }, position = new { line = newLine, character = newCharacter }, eluditeGeneration = generation },
                Ct);
            var definitionMs = watch.Elapsed.TotalMilliseconds;
            var target = definition.ValueKind == JsonValueKind.Array ? definition[0] : definition;
            Assert.Equal(uri, (target.TryGetProperty("targetUri", out var targetUri) ? targetUri : target.GetProperty("uri")).GetString());
            var targetRange = target.TryGetProperty("targetSelectionRange", out var selection) ? selection : target.GetProperty("range");
            var definitionLine = targetRange.GetProperty("start").GetProperty("line").GetInt32();
            Assert.Contains(definitionLine, new[] { constructorLine, classLine });

            // Rename of the property edits its declaration and its three uses, all in this document.
            watch.Restart();
            var rename = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
                "textDocument/rename",
                new { textDocument = new { uri }, position = new { line = propertyLine, character = propertyCharacter }, newName = "Person", eluditeGeneration = generation },
                Ct);
            var renameMs = watch.Elapsed.TotalMilliseconds;
            var edits = EditsOf(rename, uri);
            Assert.True(edits.Count >= 3, $"rename produced {edits.Count} edits");
            Assert.All(edits, e => Assert.Equal("Person", e.GetProperty("newText").GetString()));
            Assert.Contains(edits, e => e.GetProperty("range").GetProperty("start").GetProperty("line").GetInt32() == propertyLine);

            TestContext.Current.SendDiagnosticMessage(
                $"Visual Basic through the host: BC30512 after {pulls} pull(s) in {diagnosticsMs:F0} ms, hover {hoverMs:F1} ms ({hoverText.ReplaceLineEndings(" ").Trim()}), "
                + $"completion {completionMs:F1} ms ({labels.Count} items), definition {definitionMs:F1} ms (line {definitionLine}), rename {renameMs:F1} ms ({edits.Count} edits)");

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

    private static async Task RestoreAsync(string project)
    {
        var psi = new ProcessStartInfo("dotnet")
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        psi.ArgumentList.Add("restore");
        psi.ArgumentList.Add(project);
        psi.Environment["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1";
        psi.Environment["DOTNET_NOLOGO"] = "1";
        using var restore = Process.Start(psi)!;
        var output = restore.StandardOutput.ReadToEndAsync(Ct);
        var errors = restore.StandardError.ReadToEndAsync(Ct);
        await restore.WaitForExitAsync(Ct).WaitAsync(TimeSpan.FromMinutes(5), Ct);
        Assert.True(restore.ExitCode == 0, $"dotnet restore failed ({restore.ExitCode}):\n{await output}\n{await errors}");
    }

    /// <summary>A diagnostic's code as text, whether Roslyn sent a string or a number.</summary>
    private static string? CodeOf(JsonElement diagnostic) =>
        diagnostic.TryGetProperty("code", out var code) ? code.ValueKind == JsonValueKind.String ? code.GetString() : code.GetRawText() : null;

    /// <summary>The text of hover contents in any of LSP's shapes: a string, MarkupContent, MarkedString, or an array of them.</summary>
    private static string TextOf(JsonElement contents) => contents.ValueKind switch
    {
        JsonValueKind.String => contents.GetString()!,
        JsonValueKind.Array => string.Join("\n", contents.EnumerateArray().Select(TextOf)),
        JsonValueKind.Object => contents.GetProperty("value").GetString()!,
        _ => string.Empty,
    };

    /// <summary>The text edits a WorkspaceEdit makes to one document, from <c>changes</c> or <c>documentChanges</c>.</summary>
    private static List<JsonElement> EditsOf(JsonElement workspaceEdit, string uri)
    {
        var edits = new List<JsonElement>();
        if (workspaceEdit.TryGetProperty("changes", out var changes) && changes.ValueKind == JsonValueKind.Object)
        {
            foreach (var change in changes.EnumerateObject().Where(c => string.Equals(c.Name, uri, StringComparison.OrdinalIgnoreCase)))
            {
                edits.AddRange(change.Value.EnumerateArray());
            }
        }

        if (workspaceEdit.TryGetProperty("documentChanges", out var documentChanges) && documentChanges.ValueKind == JsonValueKind.Array)
        {
            foreach (var change in documentChanges.EnumerateArray())
            {
                if (change.TryGetProperty("textDocument", out var document)
                    && string.Equals(document.GetProperty("uri").GetString(), uri, StringComparison.OrdinalIgnoreCase))
                {
                    edits.AddRange(change.GetProperty("edits").EnumerateArray());
                }
            }
        }

        return edits;
    }
}
