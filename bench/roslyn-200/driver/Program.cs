// Brief 0002 bench driver, updated for the brief 0007 contract. See ../run.sh. Measures, per run (host killed between runs):
//   T0     process start -> eludite-host `eludite/host/initialize` response
//   T1     initialize response -> first textDocument/documentSymbol with >= 1 symbol
//   T2     initialize response -> first textDocument/completion containing the expected member
//   Tload  initialize response -> eludite/solution/status "loaded" (informational)
//   warming completion latency while the host's warming diagnostics pull is in flight
//   T3     latency of N sequential completion requests after warm-up (p50, p95, p99, max)
//   cancel completion / workspace/symbol requests canceled with $/cancelRequest: time to response, outcome
//   peak   VmHWM of eludite-host, of the Roslyn LS child, and the sampled peak RSS of the whole tree (Linux)
using System.Diagnostics;
using System.Globalization;
using System.Text.Json;
using System.Text.Json.Nodes;
using Bench.Driver;

var opts = ParseArgs(args);
var probe = JsonNode.Parse(File.ReadAllText(opts["probe"]))!.AsObject();
var resultsDir = Path.GetFullPath(opts["results"]);
Directory.CreateDirectory(resultsDir);
var cold = int.Parse(opts.GetValueOrDefault("cold", "10"), CultureInfo.InvariantCulture);
var warm = int.Parse(opts.GetValueOrDefault("warm", "10"), CultureInfo.InvariantCulture);
var t3Count = int.Parse(opts.GetValueOrDefault("t3", "1000"), CultureInfo.InvariantCulture);
var cancelTrials = int.Parse(opts.GetValueOrDefault("cancel-trials", "50"), CultureInfo.InvariantCulture);
var resetPaths = opts.GetValueOrDefault("cold-reset-paths", "").Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries);

var runs = new JsonArray();
for (var i = 0; i < cold + warm; i++)
{
    var kind = i < cold ? "cold" : "warm";
    if (kind == "cold")
    {
        ColdReset(resetPaths);
    }

    Console.Error.WriteLine($"--- {kind} run {i + 1 - (kind == "cold" ? 0 : cold)} ---");
    var run = await RunOnceAsync(kind, i);
    runs.Add(run);
    Console.Error.WriteLine(run.ToJsonString());
    File.WriteAllText(Path.Combine(resultsDir, "runs.json"), runs.ToJsonString(new JsonSerializerOptions { WriteIndented = true }));
}

Console.WriteLine(Summarize(runs));
File.WriteAllText(Path.Combine(resultsDir, "summary.md"), Summarize(runs));
return 0;

async Task<JsonObject> RunOnceAsync(string kind, int index)
{
    var psi = new ProcessStartInfo("dotnet")
    {
        RedirectStandardInput = true,
        RedirectStandardOutput = true,
        RedirectStandardError = true,
        UseShellExecute = false,
    };
    psi.ArgumentList.Add(opts["host"]);
    psi.ArgumentList.Add("--stdio");
    if (opts.TryGetValue("roslyn-ls", out var ls))
    {
        psi.ArgumentList.Add("--roslyn-ls");
        psi.ArgumentList.Add(ls);
    }

    var sw = Stopwatch.StartNew();
    using var proc = Process.Start(psi)!;
    var stderrPath = Path.Combine(resultsDir, $"{kind}-{index:00}.host-stderr.log");
    var stderrCopy = Task.Run(async () =>
    {
        await using var f = new FileStream(stderrPath, FileMode.Create, FileAccess.Write, FileShare.Read, 1);
        await proc.StandardError.BaseStream.CopyToAsync(f);
    });
    using var sampling = new CancellationTokenSource();
    var sampler = Task.Run(() => SampleTreeRss(proc.Id, sampling.Token));
    var client = new LspClient(proc.StandardInput.BaseStream, proc.StandardOutput.BaseStream);
    // Brief 0007 contract (protocol/schemas/host-rpc.md): eludite/host/initialize, then eludite/solution/open; readiness
    // is eludite/solution/status "loaded"; every forwarded request carries eluditeGeneration.
    var loaded = client.WaitForNotification("eludite/solution/status", p => p?["state"]?.GetValue<string>() is "loaded" or "failed");

    var result = new JsonObject { ["kind"] = kind, ["index"] = index };
    var init = await client.RequestAsync("eludite/host/initialize", new JsonObject
    {
        ["clientName"] = "bench-roslyn-200",
        ["clientVersion"] = "0.1.0",
    });
    result["t0Ms"] = sw.Elapsed.TotalMilliseconds;
    if (init.ContainsKey("error"))
    {
        throw new InvalidOperationException(init.ToJsonString());
    }

    var afterInit = Stopwatch.StartNew();
    var opened = await client.RequestAsync("eludite/solution/open", new JsonObject { ["path"] = probe["solution"]!.GetValue<string>() });
    if (opened.ContainsKey("error"))
    {
        throw new InvalidOperationException(opened.ToJsonString());
    }

    var generation = opened["result"]!["generation"]!.GetValue<long>();
    var uri = new Uri(probe["file"]!.GetValue<string>()).AbsoluteUri;
    await client.NotifyAsync("textDocument/didOpen", new JsonObject
    {
        ["textDocument"] = new JsonObject
        {
            ["uri"] = uri, ["languageId"] = "csharp", ["version"] = 1,
            ["text"] = File.ReadAllText(probe["file"]!.GetValue<string>()),
        },
    });

    // The shell sends no diagnostic pulls of its own: eludite-host warms semantics itself (pull on open, 150 ms after
    // a change, and for every open document when the solution loads), which is what brief 0002's bench did by hand.
    var docId = new JsonObject { ["uri"] = uri };
    var timeout = TimeSpan.FromMinutes(double.Parse(opts.GetValueOrDefault("timeout-min", "10"), CultureInfo.InvariantCulture));
    var t1 = PollAsync(() => client.RequestAsync("textDocument/documentSymbol", new JsonObject { ["textDocument"] = new JsonObject { ["uri"] = uri }, ["eluditeGeneration"] = generation }),
        r => r["result"] is JsonArray { Count: > 0 }, afterInit, timeout);
    // T2 attempts model a user retyping '.': each attempt is a new document version (didChange), then completion.
    var fileText = File.ReadAllText(probe["file"]!.GetValue<string>());
    var t2Version = 1;
    var t2Attempts = 0;
    var t2 = PollAsync(async () =>
    {
        t2Attempts++;
        await client.NotifyAsync("textDocument/didChange", new JsonObject
        {
            ["textDocument"] = new JsonObject { ["uri"] = uri, ["version"] = ++t2Version },
            ["contentChanges"] = new JsonArray(new JsonObject { ["text"] = fileText }),
        });
        return await client.RequestAsync("textDocument/completion", CompletionParams());
    }, r => HasLabel(r, probe["expectLabel"]!.GetValue<string>()), afterInit, timeout);
    var tLoad = loaded.ContinueWith(_ => afterInit.Elapsed.TotalMilliseconds, TaskScheduler.Default);
    result["t1Ms"] = await t1;
    result["t2Ms"] = await t2;
    result["t2Attempts"] = t2Attempts;
    result["tLoadMs"] = await tLoad.WaitAsync(timeout);
    var loadedStatus = await loaded;
    if (loadedStatus?["state"]?.GetValue<string>() != "loaded")
    {
        throw new InvalidOperationException("solution did not load: " + loadedStatus?.ToJsonString());
    }

    // T3: warm-up, then N sequential completion requests.
    for (var w = 0; w < 50; w++)
    {
        await client.RequestAsync("textDocument/completion", CompletionParams());
    }

    var lat = new List<double>(t3Count);
    var empty = 0;
    for (var n = 0; n < t3Count; n++)
    {
        var t = Stopwatch.GetTimestamp();
        var r = await client.RequestAsync("textDocument/completion", CompletionParams());
        lat.Add(Stopwatch.GetElapsedTime(t).TotalMilliseconds);
        if (!HasLabel(r, "Compute"))
        {
            empty++;
        }
    }

    lat.Sort();
    result["t3"] = new JsonObject
    {
        ["count"] = lat.Count, ["missingExpectedItem"] = empty,
        ["p50Ms"] = Pct(lat, 50), ["p95Ms"] = Pct(lat, 95), ["p99Ms"] = Pct(lat, 99), ["maxMs"] = lat[^1],
    };

    // T3 typing: each completion follows a didChange (new document version), as when typing.
    var text = File.ReadAllText(probe["file"]!.GetValue<string>());
    var typed = new List<double>(t3Count);
    var typedEmpty = 0;
    for (var n = 0; n < t3Count; n++)
    {
        await client.NotifyAsync("textDocument/didChange", new JsonObject
        {
            ["textDocument"] = new JsonObject { ["uri"] = uri, ["version"] = 10_000 + n },
            ["contentChanges"] = new JsonArray(new JsonObject { ["text"] = text + "// " + n + "\n" }),
        });
        var t = Stopwatch.GetTimestamp();
        var r = await client.RequestAsync("textDocument/completion", CompletionParams());
        typed.Add(Stopwatch.GetElapsedTime(t).TotalMilliseconds);
        if (!HasLabel(r, "Compute"))
        {
            typedEmpty++;
        }
    }

    typed.Sort();
    result["t3Typing"] = new JsonObject
    {
        ["count"] = typed.Count, ["missingExpectedItem"] = typedEmpty,
        ["p50Ms"] = Pct(typed, 50), ["p95Ms"] = Pct(typed, 95), ["p99Ms"] = Pct(typed, 99), ["maxMs"] = typed[^1],
    };

    // Completion while a warming pull is in flight: a didChange schedules the host's pull 150 ms later; wait until
    // it has started, then complete. "Overlapped" counts completions answered before that pull was published.
    var warmLat = new List<double>();
    var pullMs = new List<double>();
    var overlapped = 0;
    var warmEmpty = 0;
    var warmTrials = int.Parse(opts.GetValueOrDefault("warm-trials", "50"), CultureInfo.InvariantCulture);
    var debounceMs = double.Parse(opts.GetValueOrDefault("debounce-ms", "150"), CultureInfo.InvariantCulture);
    for (var n = 0; n < warmTrials; n++)
    {
        var version = 50_000 + n;
        var published = client.WaitForNotification("textDocument/publishDiagnostics", p => p?["version"]?.GetValue<int>() == version);
        await client.NotifyAsync("textDocument/didChange", new JsonObject
        {
            ["textDocument"] = new JsonObject { ["uri"] = uri, ["version"] = version },
            ["contentChanges"] = new JsonArray(new JsonObject { ["text"] = text + "// w" + n + "\n" }),
        });
        var changedAt = Stopwatch.GetTimestamp();
        await Task.Delay(TimeSpan.FromMilliseconds(debounceMs + 5));
        var t = Stopwatch.GetTimestamp();
        var r = await client.RequestAsync("textDocument/completion", CompletionParams());
        var done = Stopwatch.GetTimestamp();
        warmLat.Add(Stopwatch.GetElapsedTime(t, done).TotalMilliseconds);
        if (!HasLabel(r, "Compute"))
        {
            warmEmpty++;
        }

        if (!published.IsCompleted)
        {
            overlapped++;
        }

        await published.WaitAsync(TimeSpan.FromSeconds(30));
        var publishedAt = client.Diagnostics.Where(d => d.Params?["version"]?.GetValue<int>() == version).Select(d => d.Timestamp).DefaultIfEmpty(done).Max();
        pullMs.Add(Stopwatch.GetElapsedTime(changedAt, publishedAt).TotalMilliseconds - debounceMs);
    }

    warmLat.Sort();
    pullMs.Sort();
    result["completionDuringWarming"] = new JsonObject
    {
        ["count"] = warmLat.Count, ["overlappedWithPull"] = overlapped, ["missingExpectedItem"] = warmEmpty,
        ["p50Ms"] = Pct(warmLat, 50), ["p95Ms"] = Pct(warmLat, 95), ["maxMs"] = warmLat[^1],
        ["pullP50Ms"] = Pct(pullMs, 50), ["pullP95Ms"] = Pct(pullMs, 95),
    };

    result["cancelCompletion"] = await CancelTrialsAsync(client, "textDocument/completion", CompletionParams, cancelTrials);
    result["cancelWorkspaceSymbol"] = await CancelTrialsAsync(client, "workspace/symbol", () => new JsonObject { ["query"] = "Widget", ["eluditeGeneration"] = generation }, cancelTrials);
    var diagVersion = 100_000;
    result["cancelDiagnostic"] = await CancelTrialsAsync(client, "textDocument/diagnostic", () =>
    {
        // A fresh document version so the pull has real work to cancel.
        _ = client.NotifyAsync("textDocument/didChange", new JsonObject
        {
            ["textDocument"] = new JsonObject { ["uri"] = uri, ["version"] = ++diagVersion },
            ["contentChanges"] = new JsonArray(new JsonObject { ["text"] = text + "// d" + diagVersion + "\n" }),
        });
        return new JsonObject { ["textDocument"] = docId.DeepClone(), ["eluditeGeneration"] = generation };
    }, cancelTrials);
    await Task.Delay(500);
    result["lateResponses"] = client.Unexpected.Count(m => m.ContainsKey("result"));

    // Peak memory, read before the kill.
    var tree = ProcessTree(proc.Id);
    result["hostPeakMb"] = HwmMb(proc.Id);
    result["childPeakMb"] = new JsonArray(tree.Where(p => p != proc.Id).Select(p => (JsonNode)new JsonObject { ["pid"] = p, ["comm"] = Comm(p), ["peakMb"] = HwmMb(p) }).ToArray());
    sampling.Cancel();
    result["treePeakRssMb"] = await sampler;

    proc.Kill(entireProcessTree: true);
    await proc.WaitForExitAsync();
    await stderrCopy;
    await client.DisposeAsync();
    return result;

    JsonObject CompletionParams() => new()
    {
        ["textDocument"] = new JsonObject { ["uri"] = uri },
        ["position"] = new JsonObject { ["line"] = probe["line"]!.GetValue<int>(), ["character"] = probe["character"]!.GetValue<int>() },
        ["context"] = new JsonObject { ["triggerKind"] = 2, ["triggerCharacter"] = "." },
        ["eluditeGeneration"] = generation,
    };
}

static async Task<JsonObject> CancelTrialsAsync(LspClient client, string method, Func<JsonObject> makeParams, int trials)
{
    var canceled = new List<double>();
    var completed = 0;
    var otherErrors = 0;
    for (var i = 0; i < trials; i++)
    {
        var (id, response) = client.Send(method, makeParams());
        var t = Stopwatch.GetTimestamp();
        await client.NotifyAsync("$/cancelRequest", new JsonObject { ["id"] = id });
        var r = await response;
        var ms = Stopwatch.GetElapsedTime(t).TotalMilliseconds;
        if (r["error"]?["code"]?.GetValue<int>() == -32800)
        {
            canceled.Add(ms);
        }
        else if (r.ContainsKey("result"))
        {
            completed++;
        }
        else
        {
            otherErrors++;
        }

        await Task.Delay(20);
    }

    canceled.Sort();
    return new JsonObject
    {
        ["trials"] = trials,
        ["canceled"] = canceled.Count,
        ["completedBeforeCancel"] = completed,
        ["otherErrors"] = otherErrors,
        ["cancelToResponseP50Ms"] = canceled.Count > 0 ? Pct(canceled, 50) : null,
        ["cancelToResponseMaxMs"] = canceled.Count > 0 ? canceled[^1] : null,
    };
}

static async Task<double> PollAsync(Func<Task<JsonObject>> request, Func<JsonObject, bool> ok, Stopwatch since, TimeSpan timeout)
{
    JsonObject? last = null;
    while (since.Elapsed < timeout)
    {
        var r = await request();
        if (ok(r))
        {
            return since.Elapsed.TotalMilliseconds;
        }

        last = r;
        await Task.Delay(10);
    }

    var text = last?.ToJsonString() ?? "<none>";
    throw new TimeoutException("poll timed out; last response: " + text[..Math.Min(text.Length, 2000)]);
}

static bool HasLabel(JsonObject response, string label)
{
    var items = response["result"] switch
    {
        JsonArray a => a,
        JsonObject o => o["items"] as JsonArray,
        _ => null,
    };
    return items?.Any(i => i?["label"]?.GetValue<string>() == label) == true;
}

static double Pct(List<double> sorted, double p)
{
    var rank = (int)Math.Ceiling(p / 100.0 * sorted.Count) - 1;
    return sorted[Math.Clamp(rank, 0, sorted.Count - 1)];
}

static void ColdReset(string[] paths)
{
    foreach (var p in paths)
    {
        if (Directory.Exists(p))
        {
            Directory.Delete(p, recursive: true);
        }
    }

    using var shutdown = Process.Start(new ProcessStartInfo("dotnet", "build-server shutdown") { RedirectStandardOutput = true, RedirectStandardError = true })!;
    shutdown.WaitForExit();
}

static double SampleTreeRss(int root, CancellationToken ct)
{
    double peak = 0;
    while (!ct.IsCancellationRequested)
    {
        if (OperatingSystem.IsLinux())
        {
            var sum = ProcessTree(root).Sum(p => StatusKb(p, "VmRSS:")) / 1024.0;
            peak = Math.Max(peak, sum);
        }

        Thread.Sleep(50);
    }

    return peak;
}

static List<int> ProcessTree(int root)
{
    var result = new List<int> { root };
    if (!OperatingSystem.IsLinux())
    {
        return result;
    }

    var parents = new Dictionary<int, int>();
    foreach (var dir in Directory.EnumerateDirectories("/proc"))
    {
        if (!int.TryParse(Path.GetFileName(dir), out var pid))
        {
            continue;
        }

        try
        {
            var stat = File.ReadAllText(Path.Combine(dir, "stat"));
            var fields = stat[(stat.LastIndexOf(')') + 2)..].Split(' ');
            parents[pid] = int.Parse(fields[1], CultureInfo.InvariantCulture);
        }
        catch (IOException)
        {
        }
        catch (UnauthorizedAccessException)
        {
        }
    }

    for (var i = 0; i < result.Count; i++)
    {
        result.AddRange(parents.Where(kv => kv.Value == result[i]).Select(kv => kv.Key));
    }

    return result;
}

static long StatusKb(int pid, string key)
{
    try
    {
        var line = File.ReadLines($"/proc/{pid}/status").FirstOrDefault(l => l.StartsWith(key, StringComparison.Ordinal));
        return line is null ? 0 : long.Parse(line[key.Length..].Trim().Split(' ')[0], CultureInfo.InvariantCulture);
    }
    catch (IOException)
    {
        return 0;
    }
}

static double HwmMb(int pid)
{
    if (OperatingSystem.IsLinux())
    {
        return StatusKb(pid, "VmHWM:") / 1024.0;
    }

    try
    {
        using var p = Process.GetProcessById(pid);
        return p.PeakWorkingSet64 / (1024.0 * 1024.0);
    }
    catch (ArgumentException)
    {
        return 0;
    }
}

static string Comm(int pid)
{
    try
    {
        return File.ReadAllText($"/proc/{pid}/cmdline").Replace('\0', ' ').Trim();
    }
    catch (IOException)
    {
        return "?";
    }
}

static string Summarize(JsonArray runs)
{
    var sb = new System.Text.StringBuilder();
    foreach (var kind in new[] { "cold", "warm" })
    {
        var rs = runs.Where(r => r!["kind"]!.GetValue<string>() == kind).Select(r => r!.AsObject()).ToList();
        if (rs.Count == 0)
        {
            continue;
        }

        sb.AppendLine(CultureInfo.InvariantCulture, $"### {kind} ({rs.Count} runs)");
        sb.AppendLine();
        sb.AppendLine("| Metric | Median | Min | Max |");
        sb.AppendLine("|---|---|---|---|");
        void Row(string name, Func<JsonObject, double> f)
        {
            var v = rs.Select(f).Order().ToList();
            var med = v.Count % 2 == 1 ? v[v.Count / 2] : (v[v.Count / 2 - 1] + v[v.Count / 2]) / 2;
            sb.AppendLine(CultureInfo.InvariantCulture, $"| {name} | {med:F1} | {v[0]:F1} | {v[^1]:F1} |");
        }

        Row("T0 start->initialize (ms)", r => r["t0Ms"]!.GetValue<double>());
        Row("T1 init->documentSymbol (ms)", r => r["t1Ms"]!.GetValue<double>());
        Row("T2 init->completion, depth-6 project (ms)", r => r["t2Ms"]!.GetValue<double>());
        Row("Tload init->eludite/solution/status loaded (ms)", r => r["tLoadMs"]!.GetValue<double>());
        Row("T3 completion p50 (ms)", r => r["t3"]!["p50Ms"]!.GetValue<double>());
        Row("T3 completion p95 (ms)", r => r["t3"]!["p95Ms"]!.GetValue<double>());
        Row("T3 completion p99 (ms)", r => r["t3"]!["p99Ms"]!.GetValue<double>());
        Row("T3 completion max (ms)", r => r["t3"]!["maxMs"]!.GetValue<double>());
        Row("T3 missing expected item (of N)", r => r["t3"]!["missingExpectedItem"]!.GetValue<int>());
        Row("T3-typing (didChange+completion) p50 (ms)", r => r["t3Typing"]!["p50Ms"]!.GetValue<double>());
        Row("T3-typing p95 (ms)", r => r["t3Typing"]!["p95Ms"]!.GetValue<double>());
        Row("T3-typing p99 (ms)", r => r["t3Typing"]!["p99Ms"]!.GetValue<double>());
        Row("T3-typing missing expected item (of N)", r => r["t3Typing"]!["missingExpectedItem"]!.GetValue<int>());
        Row("T2 - Tload: loaded to first member completion (ms)", r => r["t2Ms"]!.GetValue<double>() - r["tLoadMs"]!.GetValue<double>());
        Row("Completion during warming pull p50 (ms)", r => r["completionDuringWarming"]!["p50Ms"]!.GetValue<double>());
        Row("Completion during warming pull p95 (ms)", r => r["completionDuringWarming"]!["p95Ms"]!.GetValue<double>());
        Row("Completion during warming pull max (ms)", r => r["completionDuringWarming"]!["maxMs"]!.GetValue<double>());
        Row("Completions overlapping a pull (of N)", r => r["completionDuringWarming"]!["overlappedWithPull"]!.GetValue<int>());
        Row("Warming pull duration p50 (ms)", r => r["completionDuringWarming"]!["pullP50Ms"]!.GetValue<double>());
        Row("Warming pull duration p95 (ms)", r => r["completionDuringWarming"]!["pullP95Ms"]!.GetValue<double>());
        Row("eludite-host peak RSS (MB)", r => r["hostPeakMb"]!.GetValue<double>());
        Row("Roslyn LS peak RSS (MB)", r => r["childPeakMb"]!.AsArray().Where(c => c!["comm"]!.GetValue<string>().Contains("LanguageServer", StringComparison.Ordinal)).Sum(c => c!["peakMb"]!.GetValue<double>()));
        Row("Tree peak RSS, sampled 50 ms (MB)", r => r["treePeakRssMb"]!.GetValue<double>());
        Row("Cancel completion: canceled of trials", r => r["cancelCompletion"]!["canceled"]!.GetValue<int>());
        Row("Cancel completion: p50 cancel->response (ms)", r => r["cancelCompletion"]!["cancelToResponseP50Ms"]?.GetValue<double>() ?? double.NaN);
        Row("Cancel completion: max cancel->response (ms)", r => r["cancelCompletion"]!["cancelToResponseMaxMs"]?.GetValue<double>() ?? double.NaN);
        Row("Cancel workspace/symbol: canceled of trials", r => r["cancelWorkspaceSymbol"]!["canceled"]!.GetValue<int>());
        Row("Cancel workspace/symbol: max cancel->response (ms)", r => r["cancelWorkspaceSymbol"]!["cancelToResponseMaxMs"]?.GetValue<double>() ?? double.NaN);
        Row("Cancel diagnostic: canceled of trials", r => r["cancelDiagnostic"]!["canceled"]!.GetValue<int>());
        Row("Cancel diagnostic: p50 cancel->response (ms)", r => r["cancelDiagnostic"]!["cancelToResponseP50Ms"]?.GetValue<double>() ?? double.NaN);
        Row("Cancel diagnostic: max cancel->response (ms)", r => r["cancelDiagnostic"]!["cancelToResponseMaxMs"]?.GetValue<double>() ?? double.NaN);
        Row("Late result responses after cancel", r => r["lateResponses"]!.GetValue<int>());
        sb.AppendLine();
    }

    return sb.ToString();
}

static Dictionary<string, string> ParseArgs(string[] args)
{
    var d = new Dictionary<string, string>();
    for (var i = 0; i + 1 < args.Length; i += 2)
    {
        d[args[i].TrimStart('-')] = args[i + 1];
    }

    foreach (var required in new[] { "host", "probe", "results" })
    {
        if (!d.ContainsKey(required))
        {
            throw new ArgumentException($"missing --{required}");
        }
    }

    return d;
}
