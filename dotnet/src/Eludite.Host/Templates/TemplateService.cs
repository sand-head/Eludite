using System.ComponentModel;
using System.Diagnostics;
using System.Globalization;
using System.Text;
using Eludite.Host.Build;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;

namespace Eludite.Host.Templates;

/// <summary>
/// <c>eludite/templates/list</c> and <c>eludite/templates/create</c> (brief 0072; host-rpc.md, "Templates and items").
/// The list is the <see cref="TemplateCatalog"/>'s reading plus <see cref="BuiltInItems"/>; a create runs
/// <c>dotnet new sln</c>, <c>dotnet new &lt;shortName&gt;</c> and <c>dotnet sln add</c> out of process, one after the
/// other, a failing step stopping the rest. Neither needs an open solution.
/// </summary>
public sealed class TemplateService
{
    private readonly TemplateCatalog _catalog;
    private readonly TextWriter _log;

    public TemplateService(TemplateCatalog catalog, TextWriter log)
    {
        _catalog = catalog;
        _log = log;
    }

    /// <summary>The catalog the service reads.</summary>
    public TemplateCatalog Catalog => _catalog;

    public Task<TemplatesListResult> ListAsync(TemplatesListParams? parameters, CancellationToken cancellationToken) =>
        Task.Run(() => List(parameters, cancellationToken), cancellationToken);

    /// <summary>The templates of <paramref name="parameters"/>' type, the SDK's and the user's first, then the built-ins.</summary>
    public TemplatesListResult List(TemplatesListParams? parameters, CancellationToken cancellationToken = default)
    {
        var sw = Stopwatch.StartNew();
        var reads = _catalog.Reads;
        var set = _catalog.Read(parameters?.Refresh == true, cancellationToken);
        var type = parameters?.Type;
        var templates = set.Entries.Select(e => e.Info).Concat(BuiltInItems.Templates)
            .Where(t => type is null || string.Equals(t.Type, type, StringComparison.OrdinalIgnoreCase))
            .ToList();
        if (_catalog.Reads != reads)
        {
            _log.WriteLine($"[templates] read {set.Entries.Count} templates from {set.TemplatesFolder ?? "(no SDK templates folder)"} in {sw.ElapsedMilliseconds} ms ({set.Skipped.Count} skipped)");
        }

        return new TemplatesListResult(templates) { TemplatesFolder = set.TemplatesFolder, Skipped = set.Skipped.Count > 0 ? set.Skipped : null };
    }

    public async Task<TemplatesCreateResult> CreateAsync(TemplatesCreateParams? parameters, CancellationToken cancellationToken)
    {
        if (parameters is null || string.IsNullOrWhiteSpace(parameters.ShortName) || string.IsNullOrWhiteSpace(parameters.Name))
        {
            throw HostErrors.BadParams("shortName, name and output are required");
        }

        var output = ProjectNamespace.Absolute(parameters.Output, "output");
        var solution = parameters.Solution is null ? null : ProjectNamespace.Absolute(parameters.Solution, "solution");
        var newSolution = parameters.NewSolution is null ? null : ProjectNamespace.Absolute(parameters.NewSolution, "newSolution");
        if (solution is not null && newSolution is not null)
        {
            throw HostErrors.BadParams("solution and newSolution exclude each other");
        }

        if (solution is not null && !File.Exists(solution))
        {
            throw HostErrors.BadParams($"{solution} does not exist");
        }

        if (newSolution is not null && File.Exists(newSolution))
        {
            throw HostErrors.BadParams($"{newSolution} exists");
        }

        var before = Snapshot(output);
        var log = new StringBuilder();
        var commandLines = new List<string>();
        var dotnet = _catalog.DotnetExecutable;
        TemplatesCreateResult Failed(int exitCode) =>
            new(exitCode, NewFiles(output, before), log.ToString(), commandLines) { Solution = solution ?? newSolution };

        if (newSolution is not null)
        {
            var dir = Path.GetDirectoryName(newSolution)!;
            var name = Path.GetFileNameWithoutExtension(newSolution);
            var siblings = Snapshot(dir, recursive: false);
            var code = await RunStepAsync(dotnet, ["new", "sln", "--name", name, "--output", dir], dir, log, commandLines, cancellationToken).ConfigureAwait(false);
            if (code != 0)
            {
                return Failed(code);
            }

            if (!File.Exists(newSolution))
            {
                // The SDK wrote the other format (.sln before .NET 10, .slnx since): ask for this one by name.
                foreach (var made in Snapshot(dir, recursive: false).Except(siblings).Where(f => Path.GetFileNameWithoutExtension(f) == name))
                {
                    File.Delete(made);
                }

                code = await RunStepAsync(dotnet, ["new", "sln", "--name", name, "--output", dir, "--format", Path.GetExtension(newSolution).TrimStart('.').ToLowerInvariant()], dir, log, commandLines, cancellationToken).ConfigureAwait(false);
                if (code != 0)
                {
                    return Failed(code);
                }
            }
        }

        var args = new List<string> { "new", parameters.ShortName, "--name", parameters.Name, "--output", output };
        if (!string.IsNullOrEmpty(parameters.Language))
        {
            args.AddRange(["--language", parameters.Language]);
        }

        var entry = FindEntry(parameters.ShortName, parameters.Language);
        foreach (var (key, value) in parameters.Parameters ?? new Dictionary<string, string>())
        {
            args.AddRange([entry?.Option(key) ?? (key.StartsWith('-') ? key : "--" + key), value]);
        }

        args.Add("--no-update-check");
        Directory.CreateDirectory(output);
        var exit = await RunStepAsync(dotnet, args, output, log, commandLines, cancellationToken).ConfigureAwait(false);
        var files = NewFiles(output, before);
        var project = files.Where(SolutionProjects.IsProjectFile)
            .OrderBy(f => Path.GetFileNameWithoutExtension(f) == parameters.Name ? 0 : 1)
            .ThenBy(f => f, StringComparer.Ordinal)
            .FirstOrDefault();
        if (exit != 0)
        {
            return Failed(exit) with { Project = project };
        }

        var target = solution ?? newSolution;
        if (target is not null && project is not null)
        {
            exit = await RunStepAsync(dotnet, ["sln", target, "add", project], Path.GetDirectoryName(target)!, log, commandLines, cancellationToken).ConfigureAwait(false);
            files = NewFiles(output, before);
        }

        return new TemplatesCreateResult(exit, files, log.ToString(), commandLines)
        {
            Project = project,
            PrimaryFile = PrimaryFile(entry, parameters.Name, output, files, project),
            Solution = target,
        };
    }

    /// <summary>The catalog's template with this short name and language (C# first when none is given).</summary>
    private TemplateEntry? FindEntry(string shortName, string? language)
    {
        var matches = _catalog.Read().Entries
            .Where(e => e.Info.ShortNames.Contains(shortName, StringComparer.OrdinalIgnoreCase))
            .ToList();
        return matches.FirstOrDefault(e => string.Equals(e.Info.Language, language ?? "C#", StringComparison.OrdinalIgnoreCase))
            ?? matches.FirstOrDefault();
    }

    /// <summary>
    /// The template's first non-project primary output (its <c>sourceName</c> replaced by the name) that was created;
    /// else <c>Program.*</c>, <c>Class1.*</c> or the first code file created; else the project.
    /// </summary>
    internal static string? PrimaryFile(TemplateEntry? entry, string name, string output, IReadOnlyList<string> files, string? project)
    {
        var created = new HashSet<string>(files, OperatingSystem.IsWindows() ? StringComparer.OrdinalIgnoreCase : StringComparer.Ordinal);
        if (entry is not null)
        {
            foreach (var primary in entry.PrimaryOutputs)
            {
                var path = entry.SourceName is { Length: > 0 } source ? primary.Replace(source, name, StringComparison.Ordinal) : primary;
                var full = Path.GetFullPath(Path.Combine(output, path.Replace('\\', Path.DirectorySeparatorChar).Replace('/', Path.DirectorySeparatorChar)));
                if (!SolutionProjects.IsProjectFile(full) && created.Contains(full))
                {
                    return full;
                }
            }
        }

        string[] code = [".cs", ".vb", ".fs"];
        bool IsCode(string f) => code.Contains(Path.GetExtension(f), StringComparer.OrdinalIgnoreCase);
        return files.FirstOrDefault(f => Path.GetFileNameWithoutExtension(f) == "Program" && IsCode(f))
            ?? files.FirstOrDefault(f => Path.GetFileNameWithoutExtension(f) == "Class1" && IsCode(f))
            ?? files.FirstOrDefault(IsCode)
            ?? project;
    }

    /// <summary>The files under <paramref name="folder"/> (none when it does not exist); <c>bin</c> and <c>obj</c> folders are not entered.</summary>
    internal static HashSet<string> Snapshot(string folder, bool recursive = true)
    {
        var files = new HashSet<string>(OperatingSystem.IsWindows() ? StringComparer.OrdinalIgnoreCase : StringComparer.Ordinal);
        if (!Directory.Exists(folder))
        {
            return files;
        }

        var pending = new Stack<string>();
        pending.Push(folder);
        while (pending.Count > 0)
        {
            var dir = pending.Pop();
            foreach (var file in Directory.EnumerateFiles(dir))
            {
                files.Add(Path.GetFullPath(file));
            }

            if (!recursive)
            {
                continue;
            }

            foreach (var sub in Directory.EnumerateDirectories(dir))
            {
                var leaf = Path.GetFileName(sub);
                if (leaf is not ("bin" or "obj" or ".git" or ".vs") && new DirectoryInfo(sub).LinkTarget is null)
                {
                    pending.Push(sub);
                }
            }
        }

        return files;
    }

    /// <summary>The files under <paramref name="folder"/> not in <paramref name="before"/>, sorted.</summary>
    internal static List<string> NewFiles(string folder, HashSet<string> before) =>
        [.. Snapshot(folder).Where(f => !before.Contains(f)).Order(StringComparer.Ordinal)];

    private async Task<int> RunStepAsync(string dotnet, IReadOnlyList<string> args, string workingDirectory, StringBuilder log, List<string> commandLines, CancellationToken cancellationToken)
    {
        var line = CommandLine("dotnet", args);
        commandLines.Add(line);
        var sw = Stopwatch.StartNew();
        var code = await DotnetProcess.RunAsync(dotnet, args, workingDirectory, log, cancellationToken).ConfigureAwait(false);
        _log.WriteLine($"[templates] {line}: exit {code.ToString(CultureInfo.InvariantCulture)} in {sw.ElapsedMilliseconds} ms");
        return code;
    }

    /// <summary>A command line for display: arguments with spaces or quotes quoted.</summary>
    internal static string CommandLine(string program, IEnumerable<string> args) =>
        string.Join(' ', new[] { program }.Concat(args.Select(a => a.Length == 0 || a.Any(c => char.IsWhiteSpace(c) || c == '"') ? "\"" + a.Replace("\"", "\\\"", StringComparison.Ordinal) + "\"" : a)));
}

/// <summary>Runs <c>dotnet</c> with telemetry, the first-run banner and update checks off, collecting its output.</summary>
internal static class DotnetProcess
{
    /// <summary>
    /// Runs <paramref name="dotnet"/> with <paramref name="args"/>, appending standard output and error to
    /// <paramref name="output"/> as they arrive; returns the exit code. Cancellation kills the process tree and throws.
    /// </summary>
    public static async Task<int> RunAsync(string dotnet, IReadOnlyList<string> args, string workingDirectory, StringBuilder output, CancellationToken cancellationToken)
    {
        var startInfo = new ProcessStartInfo(dotnet)
        {
            WorkingDirectory = workingDirectory,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            RedirectStandardInput = true,
            UseShellExecute = false,
            CreateNoWindow = true,
            StandardOutputEncoding = Encoding.UTF8,
            StandardErrorEncoding = Encoding.UTF8,
        };
        foreach (var arg in args)
        {
            startInfo.ArgumentList.Add(arg);
        }

        startInfo.Environment["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1";
        startInfo.Environment["DOTNET_NOLOGO"] = "1";
        startInfo.Environment["DOTNET_SKIP_FIRST_TIME_EXPERIENCE"] = "1";

        using var process = new Process { StartInfo = startInfo };
        void Append(object sender, DataReceivedEventArgs e)
        {
            if (e.Data is { } data)
            {
                lock (output)
                {
                    output.Append(data).Append('\n');
                }
            }
        }

        process.OutputDataReceived += Append;
        process.ErrorDataReceived += Append;
        try
        {
            process.Start();
        }
        catch (Win32Exception ex)
        {
            lock (output)
            {
                output.Append(CultureInfo.InvariantCulture, $"{dotnet} could not be started: {ex.Message}\n");
            }

            return -1;
        }

        process.StandardInput.Close();
        process.BeginOutputReadLine();
        process.BeginErrorReadLine();
        try
        {
            await process.WaitForExitAsync(cancellationToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            ProcessTree.Kill(process);
            throw;
        }

        // WaitForExitAsync waits for the redirected streams' end too, so every line is in output.
        return process.ExitCode;
    }
}
