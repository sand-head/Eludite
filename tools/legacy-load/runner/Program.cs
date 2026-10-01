// Brief 0003 runner. Throwaway spike code: loads each corpus project through eludite-host's legacy evaluators
// (cold, one process per project and per solution), compiles the evaluator output with Roslyn, loads the corpus in
// the Roslyn language server through eludite-host, and compares generated WebForms designer partials with the
// checked-in .designer.cs files. Writes one JSON file per phase plus matrix.md.
//
//   legacy-load run --manifest <manifest.json> --checkout <dir> --results <dir> --host <eludite-host.dll>
//                   [--roslyn-ls <dll>] [--evaluators k1,k2] [--phases restore,eval,getitem,compile,roslyn,designer]
//                   [--entries id1,id2] [--roslyn-modes mono,sdk,mono-fixed,sdk-fixed] [--max-docs N]
//   legacy-load eval --evaluator <kind> --work <dir> --out <file.json> <project>...   (internal: one cold process)
using System.Diagnostics;
using System.Globalization;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;
using StreamJsonRpc;

namespace LegacyLoad;

internal static partial class Program
{
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web) { WriteIndented = true };

    private static async Task<int> Main(string[] args)
    {
        if (args.Length == 0)
        {
            Console.Error.WriteLine("usage: legacy-load run|eval ...");
            return 2;
        }

        var opts = ParseOptions(args.Skip(1).ToArray(), out var positional);
        return args[0] switch
        {
            "eval" => await EvalChildAsync(opts, positional),
            "run" => await RunAsync(opts),
            _ => 2,
        };
    }

    // ---------------------------------------------------------------- eval (child process, cold)

    private static async Task<int> EvalChildAsync(Dictionary<string, string> o, List<string> projects)
    {
        var kind = o["evaluator"];
        var work = o["work"];
        Directory.CreateDirectory(work);
        var refs = new ReferenceAssemblies();
        var sw = Stopwatch.StartNew();
        IReadOnlyList<LegacyProjectEvaluation> results;
        switch (kind)
        {
            case EvaluatorKind.Mono:
                var mono = MonoInstallation.Locate() ?? throw new InvalidOperationException("Mono MSBuild not found");
                results = await new CommandLineMsBuildEvaluator(MsBuildCommand.ForMono(mono), refs, work).EvaluateAsync(projects, CancellationToken.None);
                break;
            case EvaluatorKind.BuildTools:
                var tools = BuildToolsInstallation.Locate() ?? throw new InvalidOperationException("Build Tools MSBuild not found");
                results = await new CommandLineMsBuildEvaluator(MsBuildCommand.ForBuildTools(tools), refs, work).EvaluateAsync(projects, CancellationToken.None);
                break;
            case EvaluatorKind.SdkCli:
                results = await new CommandLineMsBuildEvaluator(MsBuildCommand.ForDotnetSdk(), refs, work).EvaluateAsync(projects, CancellationToken.None);
                break;
            case EvaluatorKind.Sdk:
                results = new InProcessMsBuildEvaluator(refs, work).Evaluate(projects, CancellationToken.None);
                break;
            case EvaluatorKind.Sdk + "-strict":
                results = new InProcessMsBuildEvaluator(refs, work) { IgnoreMissingImports = false }.Evaluate(projects, CancellationToken.None);
                break;
            default:
                throw new ArgumentException($"unknown evaluator {kind}");
        }

        var doc = new JsonObject
        {
            ["evaluator"] = kind,
            ["processMs"] = sw.Elapsed.TotalMilliseconds,
            ["results"] = JsonSerializer.SerializeToNode(results, Json),
        };
        await File.WriteAllTextAsync(o["out"], doc.ToJsonString(Json));
        return 0;
    }

    // ---------------------------------------------------------------- run (orchestrator)

    private sealed record Entry(string Id, string Repo, string Path, string? Solution, string[] Covers);

    private static async Task<int> RunAsync(Dictionary<string, string> o)
    {
        var manifest = JsonNode.Parse(await File.ReadAllTextAsync(o["manifest"]))!;
        var checkout = Path.GetFullPath(o["checkout"]);
        var results = Path.GetFullPath(o["results"]);
        Directory.CreateDirectory(results);
        var phases = (o.GetValueOrDefault("phases") ?? "restore,eval,getitem,compile,roslyn,designer").Split(',');
        var evaluators = (o.GetValueOrDefault("evaluators") ?? DefaultEvaluators()).Split(',');
        var only = o.GetValueOrDefault("entries")?.Split(',');
        var maxDocs = int.Parse(o.GetValueOrDefault("max-docs") ?? "100000", CultureInfo.InvariantCulture);

        var entries = manifest["entries"]!.AsArray()
            .Select(e => new Entry(
                e!["id"]!.GetValue<string>(),
                e["repo"]!.GetValue<string>(),
                Path.Combine(checkout, e["repo"]!.GetValue<string>(), e["path"]!.GetValue<string>()),
                e["solution"] is { } s ? Path.Combine(checkout, e["repo"]!.GetValue<string>(), s.GetValue<string>()) : null,
                e["covers"]?.AsArray().Select(c => c!.GetValue<string>()).ToArray() ?? []))
            .Where(e => only is null || only.Contains(e.Id))
            .ToList();

        var missing = entries.Where(e => !File.Exists(e.Path)).ToList();
        foreach (var m in missing)
        {
            Console.Error.WriteLine($"[run] {m.Id}: {m.Path} not found (run corpus/legacy/fetch.sh)");
        }

        entries = entries.Except(missing).ToList();
        var summary = new JsonObject
        {
            ["machine"] = MachineInfo(),
            ["mono"] = MonoInstallation.Locate() is { } mi ? $"{mi.MsBuildDll} ({mi.Source})" : null,
            ["buildTools"] = BuildToolsInstallation.Locate()?.MsBuildExe,
            ["evaluators"] = new JsonArray([.. evaluators.Select(e => (JsonNode)e)]),
        };

        if (phases.Contains("restore"))
        {
            summary["restore"] = await RestoreAsync(entries, results);
        }

        var evals = new Dictionary<string, Dictionary<string, List<LegacyProjectEvaluation>>>(); // entry -> evaluator -> per project (cold)
        if (phases.Contains("eval"))
        {
            var evalNode = new JsonObject();
            foreach (var entry in entries)
            {
                evalNode[entry.Id] = await EvaluateEntryAsync(entry, evaluators, results, evals);
            }

            summary["eval"] = evalNode;
        }
        else
        {
            LoadEvaluations(results, evals);
        }

        var primary = evaluators.FirstOrDefault(e => e is EvaluatorKind.Mono or EvaluatorKind.BuildTools) ?? EvaluatorKind.Sdk;
        if (phases.Contains("getitem"))
        {
            var node = new JsonObject();
            foreach (var entry in entries)
            {
                if (evals.TryGetValue(entry.Id, out var byKind))
                {
                    node[entry.Id] = await GetItemCompareAsync(byKind, primary, results);
                }
            }

            summary["getitem"] = node;
            await File.WriteAllTextAsync(Path.Combine(results, "getitem.json"), node.ToJsonString(Json));
        }

        if (phases.Contains("compile"))
        {
            var node = new JsonObject();
            foreach (var entry in entries)
            {
                foreach (var kind in new[] { primary, EvaluatorKind.Sdk }.Distinct())
                {
                    if (evals.TryGetValue(entry.Id, out var byKind) && byKind.TryGetValue(kind, out var list))
                    {
                        node[$"{entry.Id}/{kind}"] = CompileEntry(list);
                    }
                }
            }

            summary["compile"] = node;
            await File.WriteAllTextAsync(Path.Combine(results, "compile.json"), node.ToJsonString(Json));
        }
        else if (File.Exists(Path.Combine(results, "compile.json")))
        {
            summary["compile"] = JsonNode.Parse(await File.ReadAllTextAsync(Path.Combine(results, "compile.json")));
        }

        // Earlier Roslyn results in the same directory are kept, so modes can be added one run at a time.
        var roslynFile = Path.Combine(results, "roslyn.json");
        if (File.Exists(roslynFile))
        {
            summary["roslyn"] = JsonNode.Parse(await File.ReadAllTextAsync(roslynFile));
        }

        if (phases.Contains("roslyn") && o.GetValueOrDefault("roslyn-ls") is { } roslynLs)
        {
            var node = summary["roslyn"]?.AsObject() ?? new JsonObject();
            foreach (var mode in (o.GetValueOrDefault("roslyn-modes") ?? (OperatingSystem.IsWindows() ? "sdk,sdk-fixed" : "mono,sdk,mono-fixed")).Split(','))
            {
                foreach (var entry in entries)
                {
                    var projectEvals = evals.GetValueOrDefault(entry.Id)?.GetValueOrDefault(primary) ?? [];
                    node[$"{entry.Id}/{mode}"] = await RoslynEntryAsync(entry, mode, o["host"], roslynLs, projectEvals, maxDocs, results);
                }
            }

            summary["roslyn"] = node;
            await File.WriteAllTextAsync(roslynFile, node.ToJsonString(Json));
        }

        if (phases.Contains("designer"))
        {
            var node = new JsonObject();
            foreach (var entry in entries.Where(e => e.Covers.Contains("webforms")))
            {
                var list = evals.GetValueOrDefault(entry.Id)?.GetValueOrDefault(primary) ?? [];
                node[entry.Id] = CompareDesigners(list, Path.Combine(results, "designer", entry.Id));
            }

            summary["designer"] = node;
        }

        await File.WriteAllTextAsync(Path.Combine(results, "summary.json"), summary.ToJsonString(Json));
        await File.WriteAllTextAsync(Path.Combine(results, "matrix.md"), Matrix(summary, entries, evals, primary));
        Console.WriteLine(await File.ReadAllTextAsync(Path.Combine(results, "matrix.md")));
        return 0;
    }

    private static string DefaultEvaluators() =>
        OperatingSystem.IsWindows()
            ? $"{EvaluatorKind.BuildTools},{EvaluatorKind.Sdk}"
            : $"{EvaluatorKind.Mono},{EvaluatorKind.Sdk},{EvaluatorKind.Sdk}-strict,{EvaluatorKind.SdkCli}";

    // ---------------------------------------------------------------- restore

    private static async Task<JsonObject> RestoreAsync(List<Entry> entries, string results)
    {
        var node = new JsonObject();
        MsBuildCommand? command = MonoInstallation.Locate() is { } mono ? MsBuildCommand.ForMono(mono)
            : BuildToolsInstallation.Locate() is { } tools ? MsBuildCommand.ForBuildTools(tools) : null;
        if (command is null)
        {
            node["skipped"] = "no Mono or Build Tools MSBuild";
            return node;
        }

        foreach (var entry in entries)
        {
            var env = new Dictionary<string, string>(command.Environment);
            if (command.Kind == EvaluatorKind.Mono)
            {
                // Mono reads trusted roots from $XDG_CONFIG_HOME/.mono/certs; tools/legacy-load/README.md syncs them there.
                env["XDG_CONFIG_HOME"] = Path.Combine(LegacyDesignTime.DefaultCacheDirectory(), "mono-config");
            }

            // Per project, so one project that cannot be evaluated does not abort the whole solution's restore.
            // SolutionDir puts packages.config packages where the projects' HintPaths expect them.
            var solutionDir = Path.GetDirectoryName(entry.Solution ?? (entry.Path.EndsWith(".sln", StringComparison.OrdinalIgnoreCase) ? entry.Path : Path.GetDirectoryName(entry.Path)!))!;
            var log = new StringBuilder();
            var failed = new JsonArray();
            var sw = Stopwatch.StartNew();
            foreach (var project in SolutionProjects.Read(entry.Path))
            {
                var (code, _, output) = await RunProcessAsync(command.FileName,
                    [.. command.PrefixArguments, project, "-nologo", "-v:m", "-nr:false", "-t:restore", "-p:RestorePackagesConfig=true", $"-p:SolutionDir={solutionDir}{Path.DirectorySeparatorChar}"],
                    env, Path.GetDirectoryName(project)!);
                log.AppendLine($"### {project} (exit {code})").AppendLine(output);
                if (code != 0)
                {
                    failed.Add(Path.GetFileName(project));
                }
            }

            await File.WriteAllTextAsync(Path.Combine(results, $"restore-{entry.Id}.log"), log.ToString());
            node[entry.Id] = new JsonObject { ["ms"] = Math.Round(sw.Elapsed.TotalMilliseconds), ["failedProjects"] = failed };
            Console.Error.WriteLine($"[restore] {entry.Id}: {failed.Count} project(s) failed in {sw.Elapsed.TotalMilliseconds:0} ms");
        }

        return node;
    }

    // ---------------------------------------------------------------- eval phase

    private static async Task<JsonObject> EvaluateEntryAsync(Entry entry, string[] evaluators, string results, Dictionary<string, Dictionary<string, List<LegacyProjectEvaluation>>> evals)
    {
        var projects = SolutionProjects.Read(entry.Path);
        var node = new JsonObject { ["projects"] = projects.Count };
        evals[entry.Id] = new();
        var self = Environment.ProcessPath!;
        var selfArgs = Path.GetFileName(self).StartsWith("dotnet", StringComparison.Ordinal) ? new[] { typeof(Program).Assembly.Location } : [];
        foreach (var kind in evaluators)
        {
            var dir = Path.Combine(results, "eval", entry.Id, kind);
            Directory.CreateDirectory(dir);
            var perProject = new List<LegacyProjectEvaluation>();
            var wall = new JsonObject();
            foreach (var project in projects)
            {
                var outFile = Path.Combine(dir, Path.GetFileNameWithoutExtension(project) + ".json");
                var (code, ms, output) = await RunProcessAsync(self, [.. selfArgs, "eval", "--evaluator", kind, "--work", Path.Combine(results, "work"), "--out", outFile, project], null, results);
                if (code != 0 || !File.Exists(outFile))
                {
                    perProject.Add(new LegacyProjectEvaluation { ProjectPath = project, Evaluator = kind, Loaded = false, FailureReason = "runner child failed: " + Tail(output), FailureClass = FailureClassifier.Other });
                    continue;
                }

                var doc = JsonNode.Parse(await File.ReadAllTextAsync(outFile))!;
                var r = doc["results"].Deserialize<List<LegacyProjectEvaluation>>(Json)![0];
                perProject.Add(r);
                wall[Path.GetFileName(project)] = Math.Round(ms);
                Console.Error.WriteLine($"[eval] {entry.Id} {kind} {Path.GetFileName(project)}: {(r.Loaded ? "loaded" : "FAILED " + r.FailureClass)} eval {r.ElapsedMs:0} ms, process {ms:0} ms, {r.CompileItems.Count} files, {r.ReferencePaths.Count} refs");
            }

            evals[entry.Id][kind] = perProject;
            var kindNode = new JsonObject { ["coldProcessMs"] = wall };
            if (projects.Count > 1)
            {
                var batchFile = Path.Combine(dir, "_solution.json");
                var (code, ms, _) = await RunProcessAsync(self, [.. selfArgs, "eval", "--evaluator", kind, "--work", Path.Combine(results, "work"), "--out", batchFile, .. projects], null, results);
                var loaded = code == 0 && File.Exists(batchFile)
                    ? JsonNode.Parse(await File.ReadAllTextAsync(batchFile))!["results"]!.AsArray().Count(r => r!["loaded"]!.GetValue<bool>())
                    : 0;
                kindNode["solutionProcessMs"] = Math.Round(ms);
                kindNode["solutionLoaded"] = loaded;
                Console.Error.WriteLine($"[eval] {entry.Id} {kind} whole solution: {loaded}/{projects.Count} loaded, {ms:0} ms in one cold process");
            }

            node[kind] = kindNode;
        }

        return node;
    }

    private static void LoadEvaluations(string results, Dictionary<string, Dictionary<string, List<LegacyProjectEvaluation>>> evals)
    {
        var root = Path.Combine(results, "eval");
        if (!Directory.Exists(root))
        {
            return;
        }

        foreach (var entryDir in Directory.GetDirectories(root))
        {
            var byKind = evals[Path.GetFileName(entryDir)] = new();
            foreach (var kindDir in Directory.GetDirectories(entryDir))
            {
                byKind[Path.GetFileName(kindDir)] = Directory.GetFiles(kindDir, "*.json")
                    .Where(f => !Path.GetFileName(f).StartsWith('_'))
                    .Select(f => JsonNode.Parse(File.ReadAllText(f))!["results"].Deserialize<List<LegacyProjectEvaluation>>(Json)![0])
                    .ToList();
            }
        }
    }

    // ---------------------------------------------------------------- getitem phase (reference item lists)

    // Brief 0003 asks for a comparison with real MSBuild's `-getItem` on Windows. Off Windows the closest independent
    // reference is the .NET SDK's MSBuild 17.8+ `-getItem:Compile` (pure evaluation, no injected targets). Web projects
    // need Microsoft.WebApplication.targets to evaluate at all; Mono's copy is passed as VSToolsPath when Mono exists.
    private static async Task<JsonObject> GetItemCompareAsync(Dictionary<string, List<LegacyProjectEvaluation>> byKind, string primary, string results)
    {
        var node = new JsonObject();
        var root = new ReferenceAssemblies().MergedRoot(Path.Combine(results, "work"));
        var vsTools = MonoInstallation.Locate() is { } mono
            ? Path.Combine(mono.Prefix, "lib", "mono", "xbuild", "Microsoft", "VisualStudio", "v16.0")
            : null;
        foreach (var p in byKind.GetValueOrDefault(primary) ?? [])
        {
            var args = new List<string> { "msbuild", p.ProjectPath, "-nologo", "-getItem:Compile", "-p:Configuration=Debug" };
            if (root is not null)
            {
                args.Add($"-p:TargetFrameworkRootPath={root}");
            }

            if (vsTools is not null && Directory.Exists(vsTools))
            {
                args.Add($"-p:VSToolsPath={vsTools}");
            }

            var (code, ms, output) = await RunProcessAsync("dotnet", args, null, Path.GetDirectoryName(p.ProjectPath)!);
            var name = Path.GetFileName(p.ProjectPath);
            HashSet<string>? reference = null;
            try
            {
                var json = output[output.IndexOf('{', StringComparison.Ordinal)..];
                reference = JsonNode.Parse(json)!["Items"]!["Compile"]!.AsArray().Select(i => i!["FullPath"]!.GetValue<string>()).ToHashSet(StringComparer.Ordinal);
            }
            catch (Exception ex) when (ex is JsonException or ArgumentOutOfRangeException or NullReferenceException or InvalidOperationException)
            {
                node[name] = new JsonObject { ["referenceFailed"] = Tail(output.Trim()), ["exit"] = code };
                Console.Error.WriteLine($"[getitem] {name}: reference evaluation failed (exit {code})");
                continue;
            }

            var entry = new JsonObject { ["reference"] = reference.Count, ["ms"] = Math.Round(ms) };
            foreach (var (kind, list) in byKind)
            {
                var ours = list.FirstOrDefault(x => x.ProjectPath == p.ProjectPath);
                if (ours is null || !ours.Loaded)
                {
                    continue;
                }

                var mine = ours.CompileItems.ToHashSet(StringComparer.Ordinal);
                var onlyReference = reference.Except(mine).Order(StringComparer.Ordinal).ToList();
                var onlyOurs = mine.Except(reference).Order(StringComparer.Ordinal).ToList();
                entry[kind] = new JsonObject
                {
                    ["count"] = mine.Count,
                    ["onlyInReference"] = new JsonArray([.. onlyReference.Take(5).Select(f => (JsonNode)Path.GetFileName(f))]),
                    ["onlyInEvaluator"] = new JsonArray([.. onlyOurs.Take(5).Select(f => (JsonNode)Path.GetFileName(f))]),
                    ["differences"] = onlyReference.Count + onlyOurs.Count,
                };
            }

            node[name] = entry;
            Console.Error.WriteLine($"[getitem] {name}: reference {reference.Count}, " + string.Join(", ", byKind.Keys.Select(k => $"{k} diff {entry[k]?["differences"]?.ToString() ?? "-"}")));
        }

        return node;
    }

    // ---------------------------------------------------------------- compile phase (evaluator output only)

    private static JsonObject CompileEntry(List<LegacyProjectEvaluation> projects)
    {
        var node = new JsonObject();
        var byPath = projects.Where(p => p.Loaded).ToDictionary(p => p.ProjectPath, StringComparer.Ordinal);
        var compilations = new Dictionary<string, CSharpCompilation>(StringComparer.Ordinal);
        var metadata = new Dictionary<string, MetadataReference>(StringComparer.Ordinal);

        CSharpCompilation? Build(string path, HashSet<string> visiting)
        {
            if (compilations.TryGetValue(path, out var done))
            {
                return done;
            }

            if (!byPath.TryGetValue(path, out var p) || !visiting.Add(path))
            {
                return null;
            }

            var lang = LanguageVersion.CSharp7_3; // legacy .NET Framework default when LangVersion is absent
            if (p.LangVersion is { } lv && LanguageVersionFacts.TryParse(lv, out var parsed))
            {
                lang = parsed;
            }

            var parse = new CSharpParseOptions(lang, preprocessorSymbols: p.DefineConstants);
            var trees = p.CompileItems.Where(File.Exists).AsParallel().AsOrdered()
                .Select(f => CSharpSyntaxTree.ParseText(File.ReadAllText(f), parse, f)).ToList();
            var refs = new List<MetadataReference>();
            foreach (var r in p.ReferencePaths.Where(File.Exists))
            {
                if (!metadata.TryGetValue(r, out var m))
                {
                    metadata[r] = m = MetadataReference.CreateFromFile(r);
                }

                refs.Add(m);
            }

            foreach (var pr in p.ProjectReferences)
            {
                if (Build(pr, visiting) is { } dep)
                {
                    refs.Add(dep.ToMetadataReference());
                }
            }

            var kind = p.OutputType?.ToLowerInvariant() switch
            {
                "exe" => OutputKind.ConsoleApplication,
                "winexe" => OutputKind.WindowsApplication,
                _ => OutputKind.DynamicallyLinkedLibrary,
            };
            var c = CSharpCompilation.Create(p.AssemblyName ?? Path.GetFileNameWithoutExtension(path), trees, refs,
                new CSharpCompilationOptions(kind, allowUnsafe: p.AllowUnsafeBlocks, concurrentBuild: true)
                    // csc on .NET Framework unifies retargetable (portable) identities such as mscorlib 2.0.5.0.
                    .WithAssemblyIdentityComparer(DesktopAssemblyIdentityComparer.Default));
            compilations[path] = c;
            return c;
        }

        foreach (var p in projects)
        {
            var name = Path.GetFileName(p.ProjectPath);
            if (!p.Loaded)
            {
                node[name] = new JsonObject { ["loaded"] = false };
                continue;
            }

            var sw = Stopwatch.StartNew();
            var c = Build(p.ProjectPath, new HashSet<string>(StringComparer.Ordinal))!;
            var errors = c.GetDiagnostics().Where(d => d.Severity == DiagnosticSeverity.Error).ToList();
            node[name] = new JsonObject
            {
                ["loaded"] = true,
                ["errors"] = errors.Count,
                ["topCodes"] = new JsonObject(errors.GroupBy(e => e.Id).OrderByDescending(g => g.Count()).Take(6)
                    .Select(g => KeyValuePair.Create(g.Key, (JsonNode?)g.Count()))),
                ["sample"] = new JsonArray([.. errors.GroupBy(e => e.Id).Take(8).Select(g => (JsonNode)(Path.GetFileName(g.First().Location.SourceTree?.FilePath) + ": " + g.First().GetMessage(CultureInfo.InvariantCulture)))]),
                ["ms"] = Math.Round(sw.Elapsed.TotalMilliseconds),
            };
            Console.Error.WriteLine($"[compile] {name} ({p.Evaluator}): {errors.Count} errors");
        }

        return node;
    }

    // ---------------------------------------------------------------- roslyn phase (through eludite-host)

    [GeneratedRegex(@"Successfully completed load of (?<p>.+?\.csproj)")]
    private static partial Regex LoadOk();

    [GeneratedRegex(@"(?<k>Error|Warning) while loading (?<p>.+?\.csproj): (?<m>.*)")]
    private static partial Regex LoadDiag();

    private static async Task<JsonObject> RoslynEntryAsync(Entry entry, string mode, string hostDll, string roslynLs, List<LegacyProjectEvaluation> projectEvals, int maxDocs, string results)
    {
        var open = entry.Solution ?? entry.Path;
        // Modes: "mono" and "sdk" load projects exactly as written (no designer partials, no case fixups);
        // "mono-fixed" and "sdk-fixed" add eludite-host's design-time corrections (LegacyDesignTime.PrepareAsync).
        var env = new Dictionary<string, string>
        {
            ["ELUDITE_CACHE_DIR"] = Path.Combine(results, "cache-" + mode),
            ["ELUDITE_LEGACY_MONO"] = mode.StartsWith("mono", StringComparison.Ordinal) ? "1" : "0",
            ["ELUDITE_LSP_TRACE"] = "0",
            ["ELUDITE_LEGACY_DESIGNERS"] = mode.EndsWith("-fixed", StringComparison.Ordinal) ? "1" : "0",
        };
        var psi = new ProcessStartInfo("dotnet")
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        foreach (var (k, v) in env)
        {
            psi.Environment[k] = v;
        }

        psi.ArgumentList.Add(hostDll);
        psi.ArgumentList.Add("--roslyn-ls");
        psi.ArgumentList.Add(roslynLs);
        using var host = Process.Start(psi)!;
        var log = new StringBuilder();
        var loadedAt = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var stderr = Task.Run(async () =>
        {
            while (await host.StandardError.ReadLineAsync() is { } line)
            {
                lock (log)
                {
                    log.AppendLine(line);
                }
            }
        });
        var node = new JsonObject { ["open"] = Path.GetFileName(open), ["mode"] = mode };
        var sw = Stopwatch.StartNew();
        var buildHosts = new HashSet<string>(StringComparer.Ordinal);
        using var sampling = new CancellationTokenSource();
        var sampler = Task.Run(async () =>
        {
            // Which build host Roslyn starts (mono BuildHost-net472 or dotnet BuildHost-netcore): sample /proc during the load.
            while (!sampling.IsCancellationRequested && OperatingSystem.IsLinux())
            {
                foreach (var dir in Directory.EnumerateDirectories("/proc"))
                {
                    try
                    {
                        var cmd = File.ReadAllText(Path.Combine(dir, "cmdline")).Replace('\0', ' ');
                        if (cmd.Contains("BuildHost", StringComparison.Ordinal))
                        {
                            lock (buildHosts)
                            {
                                buildHosts.Add(string.Join(' ', cmd.Split(' ').Take(2).Select(Path.GetFileName)));
                            }
                        }
                    }
                    catch (IOException)
                    {
                    }
                    catch (UnauthorizedAccessException)
                    {
                    }
                }

                try
                {
                    await Task.Delay(100, sampling.Token);
                }
                catch (OperationCanceledException)
                {
                }
            }
        });
        try
        {
            using var rpc = HostServer.CreateConnection(host.StandardInput.BaseStream, host.StandardOutput.BaseStream);
            var onStatus = new Action<JsonElement>(st =>
            {
                if (st.GetProperty("state").GetString() is "loaded" or "failed")
                {
                    loadedAt.TrySetResult();
                }
            });
            rpc.AddLocalRpcMethod(onStatus.Method, onStatus.Target, new JsonRpcMethodAttribute("eludite/solution/status") { UseSingleObjectParameterDeserialization = true });
            rpc.StartListening();
            await rpc.InvokeWithParameterObjectAsync<JsonElement>("eludite/host/initialize", new { clientName = "legacy-load", clientVersion = "0" });
            var opened = await rpc.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/open", new { path = open });
            var generation = opened.GetProperty("generation").GetInt32();
            var finished = await Task.WhenAny(loadedAt.Task, Task.Delay(TimeSpan.FromMinutes(10)));
            node["projectInitializationCompleteMs"] = finished == loadedAt.Task ? Math.Round(sw.Elapsed.TotalMilliseconds) : null;
            await sampling.CancelAsync();
            await sampler;
            lock (buildHosts)
            {
                node["buildHosts"] = new JsonArray([.. buildHosts.Select(b => (JsonNode)b)]);
            }

            var perProject = new JsonObject();
            using var gate = new SemaphoreSlim(8);
            foreach (var p in projectEvals.Where(p => p.Loaded))
            {
                var docs = p.CompileItems.Where(File.Exists).Take(maxDocs).ToList();
                var errors = 0;
                var codes = new Dictionary<string, int>();
                var samples = new Dictionary<string, string>();
                var pulled = 0;
                var psw = Stopwatch.StartNew();
                await Task.WhenAll(docs.Select(async file =>
                {
                    await gate.WaitAsync();
                    try
                    {
                        var uri = new Uri(file).AbsoluteUri;
                        await rpc.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri, languageId = "csharp", version = 1, text = await File.ReadAllTextAsync(file) } });
                        var r = await rpc.InvokeWithParameterObjectAsync<JsonElement>("textDocument/diagnostic", new { textDocument = new { uri }, eluditeGeneration = generation });
                        await rpc.NotifyWithParameterObjectAsync("textDocument/didClose", new { textDocument = new { uri } });
                        if (r.ValueKind == JsonValueKind.Object && r.TryGetProperty("items", out var items))
                        {
                            foreach (var d in items.EnumerateArray())
                            {
                                var code = d.TryGetProperty("code", out var c) ? c.ToString() : "?";
                                if (d.TryGetProperty("severity", out var s) && s.GetInt32() == 1 && code.StartsWith("CS", StringComparison.Ordinal))
                                {
                                    lock (codes)
                                    {
                                        errors++;
                                        codes[code] = codes.GetValueOrDefault(code) + 1;
                                        samples.TryAdd(code, Path.GetFileName(file) + ": " + (d.TryGetProperty("message", out var msg) ? msg.GetString() : "?"));
                                    }
                                }
                            }
                        }

                        Interlocked.Increment(ref pulled);
                    }
                    catch (RemoteInvocationException)
                    {
                    }
                    finally
                    {
                        gate.Release();
                    }
                }));
                perProject[Path.GetFileName(p.ProjectPath)] = new JsonObject
                {
                    ["documents"] = pulled,
                    ["errors"] = errors,
                    ["topCodes"] = new JsonObject(codes.OrderByDescending(kv => kv.Value).Take(6).Select(kv => KeyValuePair.Create(kv.Key, (JsonNode?)kv.Value))),
                    ["pullMs"] = Math.Round(psw.Elapsed.TotalMilliseconds),
                    ["samples"] = new JsonObject(samples.Take(8).Select(kv => KeyValuePair.Create(kv.Key, (JsonNode?)kv.Value))),
                };
                Console.Error.WriteLine($"[roslyn:{mode}] {entry.Id} {Path.GetFileName(p.ProjectPath)}: {errors} errors over {pulled} documents");
            }

            node["projects"] = perProject;
            using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(30));
            await rpc.InvokeWithCancellationAsync<JsonElement>("eludite/host/shutdown", [], cts.Token);
            await rpc.NotifyAsync("eludite/host/exit");
            await host.WaitForExitAsync(cts.Token);
        }
        catch (Exception ex) when (ex is not OutOfMemoryException)
        {
            node["error"] = ex.Message;
        }
        finally
        {
            if (!host.HasExited)
            {
                host.Kill(entireProcessTree: true);
            }

            await stderr;
        }

        var text = log.ToString();
        await File.WriteAllTextAsync(Path.Combine(results, $"roslyn-{entry.Id}-{mode}.log"), text);
        var loads = new JsonObject();
        // Roslyn (LanguageServerProjectLoader): any Error item means the project was not added to the workspace;
        // warnings only means it loaded with warnings; no items means "Successfully completed load".
        foreach (Match m in LoadOk().Matches(text))
        {
            loads[Path.GetFileName(m.Groups["p"].Value)] ??= new JsonObject { ["status"] = "loaded" };
        }

        foreach (Match m in LoadDiag().Matches(text))
        {
            var name = Path.GetFileName(m.Groups["p"].Value);
            var o = loads[name]?.AsObject() ?? new JsonObject { ["status"] = "loaded with warnings" };
            loads[name] = o;
            if (m.Groups["k"].Value == "Error")
            {
                o["status"] = "failed";
                o["failureClass"] = FailureClassifier.Classify(null, m.Groups["m"].Value);
            }

            var list = o["messages"]?.AsArray() ?? [];
            if (list.Count < 4)
            {
                list.Add(m.Groups["k"].Value + ": " + m.Groups["m"].Value.Trim());
            }

            o["messages"] = list;
        }

        node["loads"] = loads;
        node["monoBuildHostLines"] = new JsonArray([.. text.Split('\n').Where(l => l.Contains("Mono", StringComparison.Ordinal)).Take(5).Select(l => (JsonNode)l.Trim())]);
        node["legacyLines"] = new JsonArray([.. text.Split('\n').Where(l => l.Contains("[legacy]", StringComparison.Ordinal)).Take(10).Select(l => (JsonNode)l.Trim())]);
        return node;
    }

    // ---------------------------------------------------------------- designer phase

    [GeneratedRegex(@"protected\s+global::(?<t>[\w.]+)\s+(?<n>\w+)\s*;")]
    private static partial Regex DesignerField();

    private static JsonObject CompareDesigners(List<LegacyProjectEvaluation> projects, string outDir)
    {
        var node = new JsonObject();
        int pages = 0, exact = 0, fieldsReal = 0, fieldsMatched = 0, typeMismatch = 0, missing = 0, extra = 0;
        var notes = new JsonArray();
        var service = new WebFormsDesignerService(outDir);
        foreach (var p in projects.Where(p => p.Loaded))
        {
            foreach (var g in service.Generate(p))
            {
                if (g.ExistingDesignerFile is null)
                {
                    continue;
                }

                pages++;
                var real = DesignerField().Matches(File.ReadAllText(g.ExistingDesignerFile)).ToDictionary(m => m.Groups["n"].Value, m => m.Groups["t"].Value);
                var gen = g.MarkupFields.ToDictionary(f => f.Name, f => f.TypeName);
                fieldsReal += real.Count;
                var miss = real.Keys.Except(gen.Keys).ToList();
                var ext = gen.Keys.Except(real.Keys).ToList();
                var mism = real.Where(kv => gen.TryGetValue(kv.Key, out var t) && t != kv.Value).ToList();
                fieldsMatched += real.Count(kv => gen.TryGetValue(kv.Key, out var t) && t == kv.Value);
                missing += miss.Count;
                extra += ext.Count;
                typeMismatch += mism.Count;
                if (miss.Count == 0 && ext.Count == 0 && mism.Count == 0)
                {
                    exact++;
                }
                else if (notes.Count < 25)
                {
                    notes.Add($"{Path.GetFileName(g.Markup)}: missing [{string.Join(", ", miss)}] extra [{string.Join(", ", ext)}] type [{string.Join(", ", mism.Select(m => $"{m.Key}: real {m.Value} gen {gen[m.Key]}"))}]");
                }
            }
        }

        node["pagesWithDesigner"] = pages;
        node["pagesExact"] = exact;
        node["realFields"] = fieldsReal;
        node["fieldsMatchedWithType"] = fieldsMatched;
        node["missingFields"] = missing;
        node["extraFields"] = extra;
        node["typeMismatches"] = typeMismatch;
        node["differences"] = notes;
        Console.Error.WriteLine($"[designer] {pages} pages, {exact} exact, {fieldsMatched}/{fieldsReal} fields matched");
        return node;
    }

    // ---------------------------------------------------------------- matrix

    private static string Matrix(JsonObject summary, List<Entry> entries, Dictionary<string, Dictionary<string, List<LegacyProjectEvaluation>>> evals, string primary)
    {
        var sb = new StringBuilder();
        sb.AppendLine("# Brief 0003 legacy-load matrix").AppendLine();
        sb.AppendLine(CultureInfo.InvariantCulture, $"Mono: {summary["mono"]?.ToString() ?? "not found"}; Build Tools: {summary["buildTools"]?.ToString() ?? "not found"}").AppendLine();
        var kinds = evals.Values.SelectMany(v => v.Keys).Distinct().ToList();
        var modes = (summary["roslyn"]?.AsObject().Select(kv => kv.Key.Split('/')[1]).Distinct().ToList()) ?? [];
        sb.Append("| Entry | Project | ").AppendJoin(" | ", kinds).Append(" | Files | Refs (unresolved) | Compile errors (" + primary + " / sdk) | ")
            .AppendJoin(" | ", modes.Select(m => "Roslyn " + m)).AppendLine(" |");
        sb.Append("|---|---|").AppendJoin("", kinds.Select(_ => "---|")).Append("---|---|---|").AppendJoin("", modes.Select(_ => "---|")).AppendLine();
        foreach (var entry in entries)
        {
            if (!evals.TryGetValue(entry.Id, out var byKind))
            {
                continue;
            }

            var projects = byKind.Values.First().Select(p => p.ProjectPath).ToList();
            foreach (var path in projects)
            {
                var name = Path.GetFileName(path);
                sb.Append(CultureInfo.InvariantCulture, $"| {entry.Id} | {name} | ");
                foreach (var k in kinds)
                {
                    var r = byKind.GetValueOrDefault(k)?.FirstOrDefault(p => p.ProjectPath == path);
                    sb.Append(r is null ? "-"
                        : !r.Loaded ? $"FAIL ({r.FailureClass})"
                        : r.FailureClass is null ? $"ok {r.ElapsedMs:0} ms"
                        : $"ok {r.ElapsedMs:0} ms, {r.FailureClass} error").Append(" | ");
                }

                var pr = byKind.GetValueOrDefault(primary)?.FirstOrDefault(p => p.ProjectPath == path);
                sb.Append(CultureInfo.InvariantCulture, $"{pr?.CompileItems.Count} | {pr?.ReferencePaths.Count} ({pr?.UnresolvedReferences.Count}) | ");
                var c1 = summary["compile"]?[$"{entry.Id}/{primary}"]?[name]?["errors"]?.ToString() ?? "-";
                var c2 = summary["compile"]?[$"{entry.Id}/{EvaluatorKind.Sdk}"]?[name]?["errors"]?.ToString() ?? "-";
                sb.Append(CultureInfo.InvariantCulture, $"{c1} / {c2} | ");
                foreach (var mode in modes)
                {
                    var rn = summary["roslyn"]?[$"{entry.Id}/{mode}"];
                    var status = rn?["loads"]?[name]?["status"]?.ToString() ?? (rn is null ? "-" : "not reported");
                    if (rn?["loads"]?[name]?["failureClass"] is { } fc)
                    {
                        status += $" ({fc})";
                    }

                    var errs = rn?["projects"]?[name]?["errors"]?.ToString() ?? "-";
                    sb.Append(CultureInfo.InvariantCulture, $"{status}, {errs} err | ");
                }

                sb.AppendLine();
            }
        }

        return sb.ToString();
    }

    // ---------------------------------------------------------------- helpers

    private static Dictionary<string, string> ParseOptions(string[] args, out List<string> positional)
    {
        var o = new Dictionary<string, string>(StringComparer.Ordinal);
        positional = [];
        for (var i = 0; i < args.Length; i++)
        {
            if (args[i].StartsWith("--", StringComparison.Ordinal) && i + 1 < args.Length)
            {
                o[args[i][2..]] = args[++i];
            }
            else
            {
                positional.Add(args[i]);
            }
        }

        return o;
    }

    private static async Task<(int Code, double Ms, string Output)> RunProcessAsync(string file, IEnumerable<string> args, IReadOnlyDictionary<string, string>? env, string workingDirectory)
    {
        var psi = new ProcessStartInfo(file) { RedirectStandardOutput = true, RedirectStandardError = true, UseShellExecute = false, WorkingDirectory = workingDirectory };
        foreach (var a in args)
        {
            psi.ArgumentList.Add(a);
        }

        if (env is not null)
        {
            foreach (var (k, v) in env)
            {
                psi.Environment[k] = v;
            }
        }

        var sw = Stopwatch.StartNew();
        using var p = Process.Start(psi)!;
        var stdout = p.StandardOutput.ReadToEndAsync();
        var stderr = p.StandardError.ReadToEndAsync();
        await p.WaitForExitAsync();
        return (p.ExitCode, sw.Elapsed.TotalMilliseconds, await stdout + await stderr);
    }

    private static string Tail(string s) => s.Length > 400 ? s[^400..] : s;

    private static JsonObject MachineInfo() => new()
    {
        ["os"] = System.Runtime.InteropServices.RuntimeInformation.OSDescription,
        ["runtime"] = System.Runtime.InteropServices.RuntimeInformation.FrameworkDescription,
        ["cpus"] = Environment.ProcessorCount,
        ["date"] = DateTimeOffset.Now.ToString("O", CultureInfo.InvariantCulture),
    };
}
