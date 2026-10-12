using System.Collections.Concurrent;
using System.Diagnostics;
using System.Text;
using System.Text.Json;
using Eludite.Host.Lsp;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0079's proving test for Razor and Blazor (ADR-0015): the pinned Roslyn language server, whose MEF composition
/// holds the Razor cohost extension (<c>Microsoft.VisualStudioCode.RazorExtension</c>), through the real eludite-host
/// process on a copy of <c>corpus/web/blazor-app</c>. The project is opened with <c>eludite/solution/open</c>, its
/// <c>.razor</c> documents with <c>textDocument/didOpen</c> and the language id <c>aspnetcorerazor</c>, and Razor
/// answers completion (C# in <c>@code</c>, component tags and their parameters, directive attributes), hover, go to
/// definition, diagnostics and Format Document; the test plays the shell, answering the host's
/// <c>eludite/razor/html</c> as the HTML server would. Skipped with a message when the language server is not built.
/// </summary>
public sealed class RazorTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private const string BrokenRazor = """
        @page "/broken"

        <h1>Broken</h1>

        @code {
            private int count = "three";
        }
        """;

    private const string MessyRazor = """
        @page "/messy"

        <div>
        <p>@title</p>
        </div>

        @code {
        private string title = "Messy";
              private void Shout()
         {
        title = title.ToUpperInvariant();
                }
        }
        """;

    [Fact]
    public async Task RealRoslyn_ServesRazorCompletionHoverDefinitionDiagnosticsAndFormatting()
    {
        var roslyn = RoslynProcessLauncher.Locate(null);
        Assert.SkipWhen(roslyn is null, "Roslyn language server not built; run tools/roslyn-pin/build.sh (or set ELUDITE_ROSLYN_LS).");
        var source = FindCorpus();
        Assert.SkipWhen(source is null, "corpus/web/blazor-app not found above the test assembly.");

        var temp = Path.Combine(Path.GetTempPath(), "eludite-razor-" + Guid.NewGuid().ToString("N"));
        CopyDirectory(source!, temp);
        var project = Path.Combine(temp, "BlazorApp.csproj");
        var counter = Path.Combine(temp, "Components", "Pages", "Counter.razor");
        var home = Path.Combine(temp, "Components", "Pages", "Home.razor");
        var broken = Path.Combine(temp, "Components", "Pages", "Broken.razor");
        var messy = Path.Combine(temp, "Components", "Pages", "Messy.razor");
        await File.WriteAllTextAsync(broken, BrokenRazor, Ct);
        await File.WriteAllTextAsync(messy, MessyRazor, Ct);
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
        var stderr = new StringBuilder();
        var stderrPump = Task.Run(async () =>
        {
            while (await host.StandardError.ReadLineAsync(Ct) is { } line)
            {
                lock (stderr)
                {
                    stderr.AppendLine(line);
                }
            }
        }, Ct);
        var htmlRequests = new ConcurrentQueue<JsonElement>();
        var notes = new List<string>();
        try
        {
            using var rpc = TestRpc.Create(host.StandardInput.BaseStream, host.StandardOutput.BaseStream);
            var loaded = new TaskCompletionSource<JsonElement>(TaskCreationOptions.RunContinuationsAsynchronously);
            var published = new ConcurrentDictionary<string, JsonElement>(StringComparer.Ordinal);
            TestRpc.On(rpc, "eludite/solution/status", s =>
            {
                if (s.GetProperty("state").GetString() is "loaded" or "failed")
                {
                    loaded.TrySetResult(s.Clone());
                }
            });
            TestRpc.On(rpc, "textDocument/publishDiagnostics", d => published[d.GetProperty("uri").GetString()!] = d.Clone());

            // The shell's side of eludite/razor/html: what vscode-html-language-server would answer on the projection.
            var answerHtml = new Func<JsonElement, JsonElement>(p =>
            {
                htmlRequests.Enqueue(p.Clone());
                return p.GetProperty("method").GetString() switch
                {
                    "textDocument/completion" => JsonSerializer.SerializeToElement(new { isIncomplete = false, items = new[] { new { label = "eludite-html-item", kind = 10 } } }),
                    "textDocument/formatting" or "textDocument/rangeFormatting" => JsonSerializer.SerializeToElement(Array.Empty<object>()),
                    _ => JsonSerializer.SerializeToElement<object?>(null),
                };
            });
            rpc.AddLocalRpcMethod(answerHtml.Method, answerHtml.Target, new JsonRpcMethodAttribute(RazorDocuments.ShellHtmlRequest) { UseSingleObjectParameterDeserialization = true });
            rpc.StartListening();

            await rpc.InvokeWithParameterObjectAsync<JsonElement>("eludite/host/initialize", new { clientName = "razor-test", clientVersion = "0" }, Ct);
            var opened = await rpc.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/open", new { path = project }, Ct);
            var generation = opened.GetProperty("generation").GetInt64();

            var documents = new Dictionary<string, (string Uri, string[] Lines)>();
            foreach (var path in new[] { counter, home, broken, messy })
            {
                var text = await File.ReadAllTextAsync(path, Ct);
                var uri = new Uri(path).AbsoluteUri;
                documents[Path.GetFileName(path)] = (uri, text.Split('\n'));
                await rpc.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri, languageId = "aspnetcorerazor", version = 1, text } });
            }

            var status = await loaded.Task.WaitAsync(TimeSpan.FromMinutes(5), Ct);
            Assert.Equal("loaded", status.GetProperty("state").GetString());

            (int Line, int Character) Find(string file, string marker, int offset = 0)
            {
                var lines = documents[file].Lines;
                var line = Array.FindIndex(lines, l => l.Contains(marker, StringComparison.Ordinal));
                Assert.True(line >= 0, $"{file} has no line containing {marker}");
                return (line, lines[line].IndexOf(marker, StringComparison.Ordinal) + offset);
            }

            async Task<JsonElement> Request(string method, string file, (int Line, int Character)? at, object? extra = null, CancellationToken? token = null)
            {
                var p = new Dictionary<string, object?>
                {
                    ["textDocument"] = new { uri = documents[file].Uri },
                    ["eluditeGeneration"] = generation,
                };
                if (at is { } position)
                {
                    p["position"] = new { line = position.Line, character = position.Character };
                }

                if (extra is not null)
                {
                    foreach (var property in JsonSerializer.SerializeToElement(extra).EnumerateObject())
                    {
                        p[property.Name] = property.Value;
                    }
                }

                return await rpc.InvokeWithParameterObjectAsync<JsonElement>(method, p, token ?? Ct);
            }

            async Task<List<string>> Labels(string file, (int Line, int Character) at, object context, Func<List<string>, bool> until)
            {
                // Roslyn's project load (and Razor's generator run) finishes after the status; ask until it answers.
                using var wait = CancellationTokenSource.CreateLinkedTokenSource(Ct);
                wait.CancelAfter(TimeSpan.FromMinutes(3));
                while (true)
                {
                    var result = await Request("textDocument/completion", file, at, new { context }, wait.Token);
                    var items = result.ValueKind switch
                    {
                        JsonValueKind.Object when result.TryGetProperty("items", out var i) => i.EnumerateArray().ToList(),
                        JsonValueKind.Array => result.EnumerateArray().ToList(),
                        _ => [],
                    };
                    var labels = items.Select(i => i.GetProperty("label").GetString()!).ToList();
                    if (until(labels))
                    {
                        return labels;
                    }

                    await Task.Delay(500, wait.Token);
                }
            }

            var invoked = new { triggerKind = 1 };

            // C# completion inside @code lists the component's members.
            var watch = Stopwatch.StartNew();
            var codeLabels = await Labels("Counter.razor", Find("Counter.razor", "currentCount += IncrementAmount", "currentCount += ".Length), invoked,
                l => l.Contains("IncrementAmount") && l.Contains("currentCount"));
            notes.Add($"@code completion ready after {watch.ElapsedMilliseconds} ms ({codeLabels.Count} items)");

            // A component tag after '<' lists Counter.
            var tagLabels = await Labels("Home.razor", Find("Home.razor", "<Counter", 1), new { triggerKind = 2, triggerCharacter = "<" }, l => l.Contains("Counter"));
            notes.Add($"tag completion: {tagLabels.Count} items, HTML's item {(tagLabels.Contains("eludite-html-item") ? "merged" : "absent")}");

            // Inside <Counter ...> the component's parameters.
            var parameterLabels = await Labels("Home.razor", Find("Home.razor", "IncrementAmount=\"2\""), invoked, l => l.Contains("IncrementAmount"));
            notes.Add($"parameter completion: {parameterLabels.Count} items");

            // Directive attributes on an element.
            var directiveLabels = await Labels("Home.razor", Find("Home.razor", "@bind=", 1), invoked, l => l.Contains("@bind") || l.Contains("bind"));
            notes.Add($"directive attribute completion: [{string.Join(", ", directiveLabels.Where(l => l.Contains("bind") || l.Contains("onclick")).Take(8))}]");
            Assert.True(directiveLabels.Contains("@bind") || directiveLabels.Contains("bind"));
            Assert.True(directiveLabels.Contains("@onclick") || directiveLabels.Contains("onclick"), $"no onclick in [{string.Join(", ", directiveLabels.Take(40))}]");

            // Warm completion latency in @code (the brief's budget: p95 under 150 ms from the host).
            // Five warm-up runs, then fifty measured; asserted at the end, after every behavior.
            var times = new List<double>();
            var codePosition = Find("Counter.razor", "currentCount += IncrementAmount", "currentCount += ".Length);
            for (var i = 0; i < 55; i++)
            {
                watch.Restart();
                await Request("textDocument/completion", "Counter.razor", codePosition, new { context = invoked });
                if (i >= 5)
                {
                    times.Add(watch.Elapsed.TotalMilliseconds);
                }
            }

            times.Sort();
            var p95 = times[(int)Math.Ceiling(times.Count * 0.95) - 1];
            notes.Add($"warm @code completion ({codeLabels.Count} items) p50 {times[times.Count / 2]:F1} ms, p95 {p95:F1} ms over {times.Count} runs");

            // Hover on a C# field used in markup.
            var hover = await Request("textDocument/hover", "Counter.razor", Find("Counter.razor", "@currentCount", 2));
            Assert.Equal(JsonValueKind.Object, hover.ValueKind);
            var hoverText = TextOf(hover.GetProperty("contents"));
            Assert.Contains("currentCount", hoverText);
            Assert.Contains("int", hoverText);
            notes.Add($"hover: {hoverText.ReplaceLineEndings(" ").Trim()}");

            // Go to definition from the component tag lands in Counter.razor.
            var definition = await Request("textDocument/definition", "Home.razor", Find("Home.razor", "<Counter", 2));
            var target = definition.ValueKind == JsonValueKind.Array ? definition[0] : definition;
            var targetUri = (target.TryGetProperty("targetUri", out var t) ? t : target.GetProperty("uri")).GetString();
            Assert.EndsWith("Counter.razor", targetUri, StringComparison.OrdinalIgnoreCase);
            notes.Add($"definition: {targetUri}");

            // A C# error inside @code, by pull (the host's warming pull publishes the same).
            JsonElement? error = null;
            using (var wait = CancellationTokenSource.CreateLinkedTokenSource(Ct))
            {
                wait.CancelAfter(TimeSpan.FromMinutes(3));
                while (error is null)
                {
                    var report = await Request("textDocument/diagnostic", "Broken.razor", null, token: wait.Token);
                    if (report.ValueKind == JsonValueKind.Object && report.TryGetProperty("items", out var items))
                    {
                        error = items.EnumerateArray().Select(i => (JsonElement?)i).FirstOrDefault(i => CodeOf(i!.Value) == "CS0029");
                    }

                    if (error is null)
                    {
                        await Task.Delay(500, wait.Token);
                    }
                }
            }

            Assert.Equal(Find("Broken.razor", "\"three\"").Line, error!.Value.GetProperty("range").GetProperty("start").GetProperty("line").GetInt32());
            notes.Add($"diagnostic: {CodeOf(error.Value)} {error.Value.GetProperty("message").GetString()}");

            // Format Document keeps the code block and every statement, and indents it.
            var messyText = string.Join('\n', documents["Messy.razor"].Lines);
            var edits = await Request("textDocument/formatting", "Messy.razor", null, new { options = new { tabSize = 4, insertSpaces = true } });
            Assert.Equal(JsonValueKind.Array, edits.ValueKind);
            var formatted = Apply(messyText, edits);
            notes.Add($"formatting: {edits.GetArrayLength()} edits");
            foreach (var statement in new[] { "@code {", "private string title = \"Messy\";", "private void Shout()", "title = title.ToUpperInvariant();", "<p>@title</p>" })
            {
                Assert.Contains(statement, formatted);
            }

            Assert.Equal(messyText.Count(c => c == '{'), formatted.Count(c => c == '{'));
            Assert.Equal(messyText.Count(c => c == '}'), formatted.Count(c => c == '}'));
            Assert.Contains("    private string title = \"Messy\";", formatted);
            Assert.Contains("        title = title.ToUpperInvariant();", formatted);

            // Razor asked the shell for the HTML parts through eludite/razor/html, with the projection.
            var methods = htmlRequests.Select(r => r.GetProperty("method").GetString()).Distinct().ToList();
            notes.Add($"eludite/razor/html methods: [{string.Join(", ", methods)}]");
            Assert.NotEmpty(htmlRequests);
            var first = htmlRequests.First();
            Assert.True(first.GetProperty("text").GetString()!.Length > 0);
            Assert.Equal(generation, first.GetProperty("eluditeGeneration").GetInt64());

            TestContext.Current.SendDiagnosticMessage("Razor through the host: " + string.Join("; ", notes));
            Budget.Assert("razor completion p95 (ms)", p95, 150);

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

            await stderrPump.WaitAsync(TimeSpan.FromSeconds(10), CancellationToken.None).ContinueWith(_ => { }, TaskScheduler.Default);
            string log;
            lock (stderr)
            {
                log = stderr.ToString();
            }

            TestContext.Current.TestOutputHelper?.WriteLine(string.Join("\n", notes));
            TestContext.Current.TestOutputHelper?.WriteLine(log.Length > 8000 ? log[^8000..] : log);
            TestDirectory.Delete(temp);
        }
    }

    private static string? FindCorpus()
    {
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, "corpus", "web", "blazor-app");
            if (File.Exists(Path.Combine(candidate, "BlazorApp.csproj")))
            {
                return candidate;
            }
        }

        return null;
    }

    private static void CopyDirectory(string from, string to)
    {
        foreach (var file in Directory.EnumerateFiles(from, "*", SearchOption.AllDirectories))
        {
            var relative = Path.GetRelativePath(from, file);
            if (relative.StartsWith("bin", StringComparison.Ordinal) || relative.StartsWith("obj", StringComparison.Ordinal))
            {
                continue;
            }

            var target = Path.Combine(to, relative);
            Directory.CreateDirectory(Path.GetDirectoryName(target)!);
            File.Copy(file, target);
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

    /// <summary>Applies LSP text edits (positions in UTF-16 code units, no overlaps) to <paramref name="text"/>.</summary>
    internal static string Apply(string text, JsonElement edits)
    {
        var lineStarts = new List<int> { 0 };
        for (var i = 0; i < text.Length; i++)
        {
            if (text[i] == '\n')
            {
                lineStarts.Add(i + 1);
            }
        }

        int Offset(JsonElement position)
        {
            var line = position.GetProperty("line").GetInt32();
            var character = position.GetProperty("character").GetInt32();
            return line >= lineStarts.Count ? text.Length : Math.Min(lineStarts[line] + character, text.Length);
        }

        var ordered = edits.EnumerateArray()
            .Select(e => (Start: Offset(e.GetProperty("range").GetProperty("start")), End: Offset(e.GetProperty("range").GetProperty("end")), Text: e.GetProperty("newText").GetString()!))
            .OrderByDescending(e => e.Start)
            .ThenByDescending(e => e.End);
        var builder = new StringBuilder(text);
        foreach (var (start, end, newText) in ordered)
        {
            builder.Remove(start, end - start).Insert(start, newText);
        }

        return builder.ToString();
    }

    private static string? CodeOf(JsonElement diagnostic) =>
        diagnostic.TryGetProperty("code", out var code) ? code.ValueKind == JsonValueKind.String ? code.GetString() : code.GetRawText() : null;

    private static string TextOf(JsonElement contents) => contents.ValueKind switch
    {
        JsonValueKind.String => contents.GetString()!,
        JsonValueKind.Array => string.Join("\n", contents.EnumerateArray().Select(TextOf)),
        JsonValueKind.Object => contents.GetProperty("value").GetString()!,
        _ => string.Empty,
    };
}
