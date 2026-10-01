using System.Diagnostics;
using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;

namespace Eludite.Host.Legacy;

/// <summary>How to start a command-line MSBuild.</summary>
/// <param name="Kind">An <see cref="EvaluatorKind"/> name.</param>
/// <param name="FileName">Executable (<c>mono</c>, <c>MSBuild.exe</c>, <c>dotnet</c>).</param>
/// <param name="PrefixArguments">Arguments before the MSBuild switches (<c>MSBuild.dll</c> for Mono, <c>msbuild</c> for dotnet).</param>
/// <param name="Environment">Extra environment variables.</param>
public sealed record MsBuildCommand(string Kind, string FileName, IReadOnlyList<string> PrefixArguments, IReadOnlyDictionary<string, string> Environment)
{
    public static MsBuildCommand ForMono(MonoInstallation mono)
    {
        ArgumentNullException.ThrowIfNull(mono);
        var env = new Dictionary<string, string>(mono.Environment) { ["MONO_GC_PARAMS"] = "nursery-size=64m" };
        return new MsBuildCommand(EvaluatorKind.Mono, mono.MonoExecutable, ["--assembly-loader=strict", mono.MsBuildDll], env);
    }

    public static MsBuildCommand ForBuildTools(BuildToolsInstallation tools)
    {
        ArgumentNullException.ThrowIfNull(tools);
        return new MsBuildCommand(EvaluatorKind.BuildTools, tools.MsBuildExe, [], new Dictionary<string, string>());
    }

    public static MsBuildCommand ForDotnetSdk() =>
        new(EvaluatorKind.SdkCli, "dotnet", ["msbuild"], new Dictionary<string, string>());
}

/// <summary>
/// Evaluates projects by running a located MSBuild (Mono's on Linux, Build Tools' on Windows) with an injected
/// target that runs <c>ResolveReferences</c> and writes the design-time inputs to a file. That is a
/// design-time build of the reference-resolution targets only; nothing is compiled. One MSBuild process
/// evaluates a whole batch (a generated traversal project), so a solution costs one process start.
/// </summary>
public sealed partial class CommandLineMsBuildEvaluator
{
    private readonly MsBuildCommand _command;
    private readonly ReferenceAssemblies _referenceAssemblies;
    private readonly string _workDirectory;
    private readonly TextWriter? _log;

    public CommandLineMsBuildEvaluator(MsBuildCommand command, ReferenceAssemblies referenceAssemblies, string workDirectory, TextWriter? log = null)
    {
        _command = command;
        _referenceAssemblies = referenceAssemblies;
        _workDirectory = workDirectory;
        _log = log;
    }

    /// <summary>Extra targets file imported into every evaluated project (designer-partial injection).</summary>
    public string? ExtraTargets { get; init; }

    /// <summary>Upper bound for one MSBuild run. Failures and timeouts become diagnostics.</summary>
    public TimeSpan Timeout { get; init; } = TimeSpan.FromMinutes(5);

    private const string DumpTargets = """
        <Project>
          <Import Project="$(EluditeExtraTargets)" Condition="'$(EluditeExtraTargets)' != '' and Exists('$(EluditeExtraTargets)')" />
          <!-- ResolveReferences as Visual Studio's design-time pass runs it (assembly, project and COM references,
               design-time facades). A failing step (COM off Windows) is logged as a warning, and the dump still runs. -->
          <Target Name="EluditeTryResolve">
            <CallTarget Targets="ResolveReferences" ContinueOnError="true" />
          </Target>
          <Target Name="EluditeDesignTimeDump" DependsOnTargets="EluditeTryResolve">
            <ItemGroup>
              <_EluditeLine Include="project&#9;$([MSBuild]::Escape($(MSBuildProjectFullPath)))" />
              <_EluditeLine Include="prop&#9;TargetFrameworkVersion&#9;$(TargetFrameworkVersion)" />
              <_EluditeLine Include="prop&#9;TargetFrameworkMoniker&#9;$([MSBuild]::Escape($(TargetFrameworkMoniker)))" />
              <_EluditeLine Include="prop&#9;AssemblyName&#9;$(AssemblyName)" />
              <_EluditeLine Include="prop&#9;OutputType&#9;$(OutputType)" />
              <_EluditeLine Include="prop&#9;LangVersion&#9;$(LangVersion)" />
              <_EluditeLine Include="prop&#9;AllowUnsafeBlocks&#9;$(AllowUnsafeBlocks)" />
              <_EluditeLine Include="prop&#9;DefineConstants&#9;$([MSBuild]::Escape($(DefineConstants)))" />
              <_EluditeLine Include="compile&#9;%(Compile.FullPath)" Condition="'@(Compile)' != ''" />
              <_EluditeLine Include="refpath&#9;%(ReferencePath.FullPath)&#9;%(ReferencePath.OriginalItemSpec)" Condition="'@(ReferencePath)' != ''" />
              <_EluditeLine Include="reference&#9;%(Reference.Identity)" Condition="'@(Reference)' != ''" />
              <_EluditeLine Include="projectref&#9;%(ProjectReference.FullPath)" Condition="'@(ProjectReference)' != ''" />
              <_EluditeLine Include="package&#9;%(PackageReference.Identity)" Condition="'@(PackageReference)' != ''" />
              <_EluditeLine Include="content&#9;%(Content.FullPath)" Condition="'@(Content)' != ''" />
            </ItemGroup>
            <WriteLinesToFile File="$(EluditeDumpDir)/$(MSBuildProjectFullPath.Replace('/', '_').Replace('\', '_').Replace(':', '_')).dump" Lines="@(_EluditeLine)" Overwrite="true" />
          </Target>
        </Project>
        """;

    /// <summary>Evaluates <paramref name="projectPaths"/> in one MSBuild process and returns one result per project.</summary>
    public async Task<IReadOnlyList<LegacyProjectEvaluation>> EvaluateAsync(IReadOnlyList<string> projectPaths, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(projectPaths);
        var run = Path.Combine(_workDirectory, "run-" + Guid.NewGuid().ToString("N")[..12]);
        var dumpDir = Path.Combine(run, "dump");
        Directory.CreateDirectory(dumpDir);
        var targets = Path.Combine(run, "eludite.designtime.targets");
        await File.WriteAllTextAsync(targets, DumpTargets, cancellationToken).ConfigureAwait(false);

        // One TargetFrameworkRootPath for the batch: the merged symlink root, else the first project's package root.
        var root = _referenceAssemblies.MergedRoot(_workDirectory)
            ?? projectPaths.Select(DesignTimeProperties.ReadTargetFrameworkVersion).OfType<string>().Select(_referenceAssemblies.RootFor).FirstOrDefault(r => r is not null);

        var traversal = Path.Combine(run, "traversal.proj");
        var sb = new StringBuilder("<Project DefaultTargets=\"Dump\">\n  <ItemGroup>\n");
        foreach (var p in projectPaths)
        {
            sb.Append(CultureInfo.InvariantCulture, $"    <P Include=\"{System.Security.SecurityElement.Escape(Path.GetFullPath(p))}\" />\n");
        }

        sb.Append("  </ItemGroup>\n  <Target Name=\"Dump\">\n    <MSBuild Projects=\"@(P)\" Targets=\"EluditeDesignTimeDump\" BuildInParallel=\"true\" ContinueOnError=\"true\" />\n  </Target>\n</Project>\n");
        await File.WriteAllTextAsync(traversal, sb.ToString(), cancellationToken).ConfigureAwait(false);

        var psi = new ProcessStartInfo(_command.FileName)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
            WorkingDirectory = run,
        };
        foreach (var name in DesignTimeProperties.LocatorVariables)
        {
            psi.Environment.Remove(name);
        }

        foreach (var (k, v) in _command.Environment)
        {
            psi.Environment[k] = v;
        }

        psi.Environment["MSBUILDDISABLENODEREUSE"] = "1";
        foreach (var a in _command.PrefixArguments)
        {
            psi.ArgumentList.Add(a);
        }

        psi.ArgumentList.Add(traversal);
        foreach (var a in new[] { "-nologo", "-noAutoResponse", "-nr:false", "-m", "-v:m", "-clp:NoSummary;ForceNoAlign;DisableConsoleColor" })
        {
            psi.ArgumentList.Add(a);
        }

        var props = new Dictionary<string, string>(DesignTimeProperties.Create(root), StringComparer.OrdinalIgnoreCase)
        {
            ["CustomAfterMicrosoftCommonTargets"] = targets,
            ["EluditeDumpDir"] = dumpDir,
        };
        if (ExtraTargets is not null)
        {
            props["EluditeExtraTargets"] = ExtraTargets;
        }

        foreach (var (k, v) in props)
        {
            psi.ArgumentList.Add($"-p:{k}={v}");
        }

        var sw = Stopwatch.StartNew();
        var output = new StringBuilder();
        string failure;
        using (var process = new Process { StartInfo = psi })
        {
            process.OutputDataReceived += (_, e) => { if (e.Data is not null) { lock (output) { output.AppendLine(e.Data); } } };
            process.ErrorDataReceived += (_, e) => { if (e.Data is not null) { lock (output) { output.AppendLine(e.Data); } } };
            try
            {
                process.Start();
            }
            catch (System.ComponentModel.Win32Exception ex)
            {
                return [.. projectPaths.Select(p => Failed(p, $"could not start {_command.FileName}: {ex.Message}", FailureClassifier.Other, sw.Elapsed))];
            }

            process.BeginOutputReadLine();
            process.BeginErrorReadLine();
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            timeout.CancelAfter(Timeout);
            try
            {
                await process.WaitForExitAsync(timeout.Token).ConfigureAwait(false);
                failure = $"{_command.Kind} exited with code {process.ExitCode}";
            }
            catch (OperationCanceledException)
            {
                process.Kill(entireProcessTree: true);
                failure = cancellationToken.IsCancellationRequested ? "canceled" : $"timed out after {Timeout.TotalSeconds:0} s";
            }
        }

        var elapsed = sw.Elapsed;
        var diagnostics = ParseDiagnostics(output.ToString());
        _log?.WriteLine($"[legacy] {_command.Kind}: {projectPaths.Count} project(s) in {elapsed.TotalMilliseconds:0} ms");

        var results = new List<LegacyProjectEvaluation>();
        foreach (var project in projectPaths)
        {
            var full = Path.GetFullPath(project);
            var mine = diagnostics.Where(d => d.Project is null ? projectPaths.Count == 1 : PathEquals(d.Project, full)).Select(d => d.Diagnostic).ToList();
            var dump = Path.Combine(dumpDir, DumpName(full));
            if (!File.Exists(dump))
            {
                var firstError = mine.FirstOrDefault(d => d.Severity == "error");
                results.Add(Failed(full, firstError?.Message ?? failure, firstError is null ? FailureClassifier.Other : FailureClassifier.Classify(firstError.Code, firstError.Message), elapsed) with
                {
                    Diagnostics = mine,
                    Evaluator = _command.Kind,
                });
                continue;
            }

            results.Add(ReadDump(await File.ReadAllLinesAsync(dump, cancellationToken).ConfigureAwait(false), full, _command.Kind, mine, elapsed));
        }

        TryDelete(run);
        return results;
    }

    internal static string DumpName(string fullPath) => fullPath.Replace('/', '_').Replace('\\', '_').Replace(':', '_') + ".dump";

    private static LegacyProjectEvaluation Failed(string project, string reason, string failureClass, TimeSpan elapsed) => new()
    {
        ProjectPath = project,
        Evaluator = string.Empty,
        Loaded = false,
        FailureReason = reason,
        FailureClass = failureClass,
        UsesPackagesConfig = File.Exists(Path.Combine(Path.GetDirectoryName(project) ?? ".", "packages.config")),
        ElapsedMs = elapsed.TotalMilliseconds,
    };

    internal static LegacyProjectEvaluation ReadDump(IEnumerable<string> lines, string project, string kind, IReadOnlyList<EvaluationDiagnostic> diagnostics, TimeSpan elapsed)
    {
        var props = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var compile = new List<string>();
        var refPaths = new List<string>();
        var resolvedSpecs = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var references = new List<string>();
        var projectRefs = new List<string>();
        var packages = new List<string>();
        var markup = new List<string>();
        foreach (var line in lines)
        {
            var parts = line.Split('\t');
            switch (parts[0])
            {
                case "prop" when parts.Length >= 3:
                    props[parts[1]] = parts[2];
                    break;
                case "compile":
                    compile.Add(parts[1]);
                    break;
                case "refpath":
                    refPaths.Add(parts[1]);
                    if (parts.Length > 2)
                    {
                        resolvedSpecs.Add(parts[2]);
                    }

                    break;
                case "reference":
                    references.Add(parts[1]);
                    break;
                case "projectref":
                    projectRefs.Add(parts[1]);
                    break;
                case "package":
                    packages.Add(parts[1]);
                    break;
                case "content" when DesignTimeProperties.IsMarkup(parts[1]):
                    markup.Add(parts[1]);
                    break;
                default:
                    break;
            }
        }

        // Errors from reference resolution (COM off Windows, for example) leave the item lists intact.
        var firstError = diagnostics.FirstOrDefault(d => d.Severity == "error");
        return new LegacyProjectEvaluation
        {
            ProjectPath = project,
            Evaluator = kind,
            Loaded = true,
            FailureReason = firstError?.Message,
            FailureClass = firstError is null ? null : FailureClassifier.Classify(firstError.Code, firstError.Message),
            TargetFrameworkVersion = props.GetValueOrDefault("TargetFrameworkVersion"),
            TargetFrameworkMoniker = props.GetValueOrDefault("TargetFrameworkMoniker"),
            AssemblyName = props.GetValueOrDefault("AssemblyName"),
            OutputType = props.GetValueOrDefault("OutputType"),
            LangVersion = NullIfEmpty(props.GetValueOrDefault("LangVersion")),
            AllowUnsafeBlocks = string.Equals(props.GetValueOrDefault("AllowUnsafeBlocks"), "true", StringComparison.OrdinalIgnoreCase),
            DefineConstants = SplitDefines(props.GetValueOrDefault("DefineConstants")),
            CompileItems = compile.Distinct(StringComparer.Ordinal).ToList(),
            ReferencePaths = refPaths.Distinct(StringComparer.Ordinal).ToList(),
            UnresolvedReferences = references.Where(r => !resolvedSpecs.Contains(r)).ToList(),
            ProjectReferences = projectRefs,
            PackageReferences = packages,
            UsesPackagesConfig = File.Exists(Path.Combine(Path.GetDirectoryName(project) ?? ".", "packages.config")),
            MarkupItems = markup,
            MissingImports = diagnostics.Where(d => d.Code == "MSB4019").Select(d => d.Message).ToList(),
            Diagnostics = diagnostics,
            ElapsedMs = elapsed.TotalMilliseconds,
        };
    }

    internal static IReadOnlyList<string> SplitDefines(string? value) =>
        string.IsNullOrWhiteSpace(value) ? [] : value.Split([';', ','], StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);

    private static string? NullIfEmpty(string? s) => string.IsNullOrWhiteSpace(s) ? null : s;

    [GeneratedRegex(@"^\s*(?<file>[^\s].*?)(?:\((?<pos>[\d,\-]+)\))?\s*:\s*(?<sev>error|warning)\s*(?<code>[A-Za-z]+\d+)?\s*:\s*(?<msg>.*?)(?:\s+\[(?<proj>[^\]]+)\])?\s*$")]
    private static partial Regex DiagnosticLine();

    internal static List<(string? Project, EvaluationDiagnostic Diagnostic)> ParseDiagnostics(string output)
    {
        var result = new List<(string?, EvaluationDiagnostic)>();
        var seen = new HashSet<string>(StringComparer.Ordinal);
        foreach (var line in output.Split('\n'))
        {
            var m = DiagnosticLine().Match(line.TrimEnd('\r'));
            if (!m.Success || !seen.Add(line))
            {
                continue;
            }

            var code = m.Groups["code"].Success ? m.Groups["code"].Value : null;
            var proj = m.Groups["proj"].Success ? m.Groups["proj"].Value : null;
            if (proj is not null && proj.EndsWith("traversal.proj", StringComparison.OrdinalIgnoreCase))
            {
                proj = null;
            }

            result.Add((proj, new EvaluationDiagnostic(m.Groups["sev"].Value, code, m.Groups["msg"].Value, m.Groups["file"].Value)));
        }

        return result;
    }

    private static bool PathEquals(string a, string b) =>
        string.Equals(Path.GetFullPath(a), b, OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal);

    private static void TryDelete(string dir)
    {
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch (IOException)
        {
        }
        catch (UnauthorizedAccessException)
        {
        }
    }
}
