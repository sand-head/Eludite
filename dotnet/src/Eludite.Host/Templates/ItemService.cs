using System.Diagnostics;
using System.Text;
using Eludite.Host.Legacy;
using Eludite.Host.Projects;
using Eludite.Host.Rpc;

namespace Eludite.Host.Templates;

/// <summary>
/// <c>eludite/project/addItem</c> and <c>eludite/project/addExisting</c> (brief 0072; host-rpc.md, "Templates and
/// items"). An item comes from <see cref="BuiltInItems"/> (written here with <see cref="ProjectNamespace"/>'s
/// namespace) or from an SDK item template (<c>dotnet new &lt;shortName&gt; --project</c>). A legacy project gains a
/// <c>Compile</c> or <c>Content</c> item per new file through <see cref="ProjectItemWriter"/>; an SDK project is not
/// written (its globs list the files), except for links. When the project file changed and the project belongs to the
/// open solution, the solution reloads and the result carries the new generation. Writes run one at a time.
/// </summary>
public sealed class ItemService : IDisposable
{
    private static readonly StringComparer PathComparer = OperatingSystem.IsWindows() ? StringComparer.OrdinalIgnoreCase : StringComparer.Ordinal;
    private static readonly string[] CodeExtensions = [".cs", ".vb", ".fs"];

    private readonly TemplateCatalog _catalog;
    private readonly Func<(long Generation, string? Path, CancellationToken GenerationChanged)> _current;
    private readonly Func<string, long> _reload;
    private readonly TextWriter _log;
    private readonly SemaphoreSlim _writes = new(1, 1);

    /// <param name="catalog">The SDK's item templates.</param>
    /// <param name="current">The current generation and the open solution.</param>
    /// <param name="reload">Reopens the solution after a project write (as <c>eludite/solution/open</c>); returns the new generation.</param>
    public ItemService(TemplateCatalog catalog, Func<(long Generation, string? Path, CancellationToken GenerationChanged)> current, Func<string, long> reload, TextWriter log)
    {
        _catalog = catalog;
        _current = current;
        _reload = reload;
        _log = log;
    }

    public void Dispose() => _writes.Dispose();

    public async Task<ProjectAddItemResult> AddItemAsync(ProjectAddItemParams? parameters, CancellationToken cancellationToken)
    {
        if (parameters is null || string.IsNullOrWhiteSpace(parameters.Template) || string.IsNullOrWhiteSpace(parameters.Name))
        {
            throw HostErrors.BadParams("project, folder, template and name are required");
        }

        var (project, folder) = Validate(parameters.Project, parameters.Folder);
        var language = BuiltInItems.LanguageOf(project);
        var template = Resolve(parameters.Template, language)
            ?? throw HostErrors.BadParams($"{parameters.Template}: no item template has this identity or short name");
        var extension = BuiltInItems.Extension(template.Language ?? language);
        var name = parameters.Name.Trim();
        if (extension is not null && name.EndsWith(extension, StringComparison.OrdinalIgnoreCase) && name.Length > extension.Length)
        {
            name = name[..^extension.Length];
        }

        if (name.IndexOfAny(Path.GetInvalidFileNameChars()) >= 0 || name.Contains('/', StringComparison.Ordinal) || name.Contains('\\', StringComparison.Ordinal))
        {
            throw HostErrors.BadParams($"{parameters.Name} is not a file name");
        }

        if (extension is not null && File.Exists(Path.Combine(folder, name + extension)))
        {
            throw HostErrors.BadParams($"{Path.Combine(folder, name + extension)} exists");
        }

        var ns = ProjectNamespace.For(project, folder);
        await _writes.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            Directory.CreateDirectory(folder);
            List<string> files;
            string? output = null;
            if (template.Source == BuiltInItems.Source)
            {
                var path = Path.Combine(folder, name + extension);
                var fileNamespace = template.Language == "VB" ? string.Join('.', ProjectNamespace.FolderSegments(project, folder)) : ns;
                var bytes = BuiltInItems.Bytes(BuiltInItems.Render(template, ProjectNamespace.Identifier(name), fileNamespace));
                try
                {
                    using var stream = new FileStream(path, FileMode.CreateNew, FileAccess.Write);
                    await stream.WriteAsync(bytes, cancellationToken).ConfigureAwait(false);
                }
                catch (IOException) when (File.Exists(path))
                {
                    throw HostErrors.BadParams($"{path} exists");
                }

                files = [path];
            }
            else
            {
                (files, output) = await RunItemTemplateAsync(template, name, folder, project, parameters.Parameters, cancellationToken).ConfigureAwait(false);
                files = [.. files.OrderBy(f => Path.GetFileNameWithoutExtension(f) == name ? 0 : 1).ThenBy(f => f, StringComparer.Ordinal)];
            }

            var written = false;
            if (SolutionProjects.IsLegacy(project))
            {
                var dir = Path.GetDirectoryName(project)!;
                var items = files.Select(f => new ProjectItemToAdd(IsCode(f) ? "Compile" : "Content", Relative(dir, f))).ToList();
                written = await Task.Run(() => ProjectItemWriter.Add(project, items, cancellationToken), cancellationToken).ConfigureAwait(false);
            }

            _log.WriteLine($"[items] {template.Identity} {name} in {folder}: {files.Count} file(s){(written ? ", project written" : string.Empty)}");
            return new ProjectAddItemResult(files, ns, written, Generation(project, written)) { Output = output };
        }
        finally
        {
            _writes.Release();
        }
    }

    public async Task<ProjectAddExistingResult> AddExistingAsync(ProjectAddExistingParams? parameters, CancellationToken cancellationToken)
    {
        if (parameters?.Files is not { Count: > 0 } requested)
        {
            throw HostErrors.BadParams("project, folder and files are required");
        }

        var (project, folder) = Validate(parameters.Project, parameters.Folder);
        var sources = requested.Select(f => ProjectNamespace.Absolute(f, "files[]")).ToList();
        foreach (var source in sources.Where(s => !File.Exists(s)))
        {
            throw HostErrors.BadParams($"{source} does not exist");
        }

        var link = parameters.Link == true;
        var dir = Path.GetDirectoryName(project)!;
        await _writes.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            var items = new List<ProjectItemToAdd>();
            var files = new List<string>();
            if (link)
            {
                var folderRelative = Path.GetRelativePath(dir, folder);
                foreach (var source in sources)
                {
                    var linkPath = folderRelative == "." ? Path.GetFileName(source) : Path.Combine(folderRelative, Path.GetFileName(source));
                    items.Add(new ProjectItemToAdd(IsCode(source) ? "Compile" : "None", Relative(dir, source)) { Link = linkPath.Replace('/', '\\') });
                    files.Add(source);
                }
            }
            else
            {
                var targets = sources.Select(s => (Source: s, Target: Path.Combine(folder, Path.GetFileName(s)))).ToList();
                foreach (var (source, target) in targets)
                {
                    if (!PathComparer.Equals(source, target) && File.Exists(target))
                    {
                        throw HostErrors.BadParams($"{target} exists");
                    }
                }

                Directory.CreateDirectory(folder);
                foreach (var (source, target) in targets)
                {
                    cancellationToken.ThrowIfCancellationRequested();
                    if (!PathComparer.Equals(source, target))
                    {
                        File.Copy(source, target, overwrite: false);
                    }

                    files.Add(target);
                }

                if (SolutionProjects.IsLegacy(project))
                {
                    items.AddRange(files.Select(f => new ProjectItemToAdd(IsCode(f) ? "Compile" : "Content", Relative(dir, f))));
                }
            }

            var written = items.Count > 0
                && await Task.Run(() => ProjectItemWriter.Add(project, items, cancellationToken), cancellationToken).ConfigureAwait(false);
            _log.WriteLine($"[items] {(link ? "linked" : "added")} {files.Count} file(s) in {folder}{(written ? ", project written" : string.Empty)}");
            return new ProjectAddExistingResult(files, written, Generation(project, written));
        }
        finally
        {
            _writes.Release();
        }
    }

    /// <summary>
    /// The template <paramref name="template"/> names: an identity first; else a short name, the built-in for the
    /// project's language first, then an item template in that language, then any.
    /// </summary>
    internal TemplateInfo? Resolve(string template, string? language)
    {
        var sdk = _catalog.Read().Entries.Select(e => e.Info).ToList();
        if (BuiltInItems.Find(template) is { } builtIn)
        {
            return builtIn;
        }

        if (sdk.FirstOrDefault(t => string.Equals(t.Identity, template, StringComparison.Ordinal)) is { } exact)
        {
            return exact;
        }

        var named = sdk.Where(t => t.ShortNames.Contains(template, StringComparer.OrdinalIgnoreCase)).ToList();
        return BuiltInItems.FindShortName(template, language)
            ?? named.FirstOrDefault(t => t.Type == "item" && t.Language == language)
            ?? named.FirstOrDefault(t => t.Language == language)
            ?? named.FirstOrDefault(t => t.Type == "item")
            ?? named.FirstOrDefault();
    }

    private async Task<(List<string> Files, string Output)> RunItemTemplateAsync(TemplateInfo template, string name, string folder, string project, IReadOnlyDictionary<string, string>? parameters, CancellationToken cancellationToken)
    {
        var before = TemplateService.Snapshot(folder);
        var args = new List<string> { "new", template.ShortNames[0], "--name", name, "--output", folder };
        if (template.Language is { } language && _catalog.Read().Entries.Count(e => e.Info.ShortNames.Contains(template.ShortNames[0], StringComparer.OrdinalIgnoreCase)) > 1)
        {
            args.AddRange(["--language", language]);
        }

        var entry = _catalog.Read().Entries.FirstOrDefault(e => e.Info.Identity == template.Identity);
        foreach (var (key, value) in parameters ?? new Dictionary<string, string>())
        {
            args.AddRange([entry?.Option(key) ?? (key.StartsWith('-') ? key : "--" + key), value]);
        }

        var output = new StringBuilder();
        var sw = Stopwatch.StartNew();
        var code = await DotnetProcess.RunAsync(_catalog.DotnetExecutable, [.. args, "--project", project, "--no-update-check"], folder, output, cancellationToken).ConfigureAwait(false);
        if (code != 0 && TemplateService.NewFiles(folder, before).Count == 0)
        {
            // A template that does not take --project: once more without it.
            output.Append("(retrying without --project)\n");
            code = await DotnetProcess.RunAsync(_catalog.DotnetExecutable, [.. args, "--no-update-check"], folder, output, cancellationToken).ConfigureAwait(false);
        }

        if (code != 0
            && output.ToString().Contains("constraints are not met", StringComparison.Ordinal)
            && TemplateService.NewFiles(folder, before).Count == 0
            && !Directory.EnumerateFiles(folder).Any(f => Path.GetFileName(f).StartsWith(name + ".", StringComparison.OrdinalIgnoreCase)))
        {
            // The SDK's item templates require a restored project (their project-capability constraint); a project
            // just created is not. No file of this name is in the folder, so --force overwrites nothing of the item's.
            output.Append("(retrying with --force: the project is not restored)\n");
            code = await DotnetProcess.RunAsync(_catalog.DotnetExecutable, [.. args, "--no-update-check", "--force"], folder, output, cancellationToken).ConfigureAwait(false);
        }

        _log.WriteLine($"[items] {TemplateService.CommandLine("dotnet", args)}: exit {code} in {sw.ElapsedMilliseconds} ms");
        if (code != 0)
        {
            throw HostErrors.BadParams($"dotnet new {template.ShortNames[0]} failed (exit {code}):\n{output}");
        }

        return (TemplateService.NewFiles(folder, before), output.ToString());
    }

    /// <summary>The generation after a change: a reload's when the project file changed and the open solution holds the project.</summary>
    private long Generation(string project, bool written)
    {
        var (generation, solution, _) = _current();
        if (!written || solution is null)
        {
            return generation;
        }

        var belongs = PathComparer.Equals(Path.GetFullPath(solution), project) || SolutionProjects.Read(solution).Contains(project, PathComparer);
        return belongs ? _reload(solution) : generation;
    }

    private static (string Project, string Folder) Validate(string? project, string? folder)
    {
        var p = ProjectNamespace.Absolute(project, "project");
        var f = ProjectNamespace.Absolute(folder, "folder");
        if (!SolutionProjects.IsProjectFile(p) || !File.Exists(p))
        {
            throw HostErrors.BadParams($"{p}: expected an existing .csproj, .vbproj or .fsproj file");
        }

        if (!ProjectNamespace.IsInProject(p, f))
        {
            throw HostErrors.BadParams($"{f} is outside the project's folder");
        }

        return (p, f.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar));
    }

    private static bool IsCode(string path) => CodeExtensions.Contains(Path.GetExtension(path), StringComparer.OrdinalIgnoreCase);

    /// <summary>The project-relative path with backslashes, as project files write it.</summary>
    private static string Relative(string projectDirectory, string path) =>
        Path.GetRelativePath(projectDirectory, path).Replace('/', '\\');
}
