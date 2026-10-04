using System.Xml.Linq;
using NuGet.Common;
using NuGet.Configuration;

namespace Eludite.Host.NuGet;

/// <summary>
/// The NuGet.config chain as NuGet reads it (host-rpc.md, "NuGet"): the files from the solution's folder up (the first
/// of <c>NuGet.Config</c>, <c>nuget.config</c>, <c>NuGet.config</c> in each folder, closest first), then the user's file,
/// then the machine-wide ones. Changes (add, remove, enable, disable) are written to the user's file only.
/// </summary>
public sealed class NuGetConfigChain
{
    private static readonly string[] FileNames = ["NuGet.Config", "nuget.config", "NuGet.config"];
    private readonly bool _machineWide;

    /// <param name="userConfig">The user's NuGet.config; default NuGet's own (<c>~/.nuget/NuGet/NuGet.Config</c>, <c>%APPDATA%\NuGet\NuGet.Config</c>).</param>
    /// <param name="machineWide">Read the machine-wide files (tests turn it off).</param>
    public NuGetConfigChain(string? userConfig = null, bool machineWide = true)
    {
        UserConfig = userConfig ?? Path.Combine(NuGetEnvironment.GetFolderPath(NuGetFolderPath.UserSettingsDirectory), "NuGet.Config");
        _machineWide = machineWide;
    }

    /// <summary>The user's NuGet.config, where changes are written.</summary>
    public string UserConfig { get; }

    /// <summary>The files from <paramref name="root"/> up, closest first.</summary>
    public static IReadOnlyList<string> SolutionFiles(string root)
    {
        var files = new List<string>();
        for (var dir = new DirectoryInfo(Path.GetFullPath(root)); dir is not null; dir = dir.Parent)
        {
            foreach (var candidate in new[] { dir.FullName, Path.Combine(dir.FullName, ".nuget") })
            {
                var found = FileNames.Select(n => Path.Combine(candidate, n)).FirstOrDefault(File.Exists);
                if (found is not null)
                {
                    files.Add(found);
                }
            }
        }

        return files;
    }

    /// <summary>The machine-wide files NuGet reads, highest priority first.</summary>
    public IReadOnlyList<string> MachineFiles()
    {
        if (!_machineWide)
        {
            return [];
        }

        try
        {
            return [.. new XPlatMachineWideSetting().Settings.GetConfigFilePaths()];
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or NuGetConfigurationException)
        {
            return [];
        }
    }

    /// <summary>Every file of the chain that exists, highest priority first.</summary>
    public IReadOnlyList<string> Files(string root)
    {
        var all = new List<string>(SolutionFiles(root));
        var user = Path.GetFullPath(UserConfig);
        if (File.Exists(user) && !all.Contains(user, StringComparer.Ordinal))
        {
            all.Add(user);
        }

        all.AddRange(MachineFiles().Where(f => File.Exists(f) && !all.Contains(f, StringComparer.Ordinal)));
        return all;
    }

    /// <summary>NuGet's settings for the chain.</summary>
    public ISettings Load(string root)
    {
        var files = Files(root);
        return files.Count == 0 ? NullSettings.Instance : Settings.LoadSettingsGivenConfigPaths([.. files]);
    }

    /// <summary>The enabled sources, in configuration order.</summary>
    public IReadOnlyList<PackageSource> Enabled(string root) =>
        [.. new PackageSourceProvider(Load(root)).LoadPackageSources().Where(s => s.IsEnabled)];

    /// <summary><c>eludite/nuget/sources</c> <c>list</c>.</summary>
    public NuGetSourcesResult List(string root, bool changed = false)
    {
        var files = Files(root);
        var solutionFiles = SolutionFiles(root);
        var machine = MachineFiles();
        var user = Path.GetFullPath(UserConfig);
        var sources = new List<NuGetSourceInfo>();
        foreach (var s in new PackageSourceProvider(Load(root)).LoadPackageSources())
        {
            var file = files.FirstOrDefault(f => Defines(f, s.Name));
            var scope = file is null ? "other"
                : string.Equals(file, user, StringComparison.Ordinal) ? "user"
                : solutionFiles.Contains(file, StringComparer.Ordinal) ? "solution"
                : machine.Contains(file, StringComparer.Ordinal) ? "machine"
                : "other";
            sources.Add(new NuGetSourceInfo(s.Name, s.Source, s.IsEnabled, s.IsLocal, scope) { ConfigFile = file });
        }

        return new NuGetSourcesResult(sources, files, user, changed);
    }

    /// <summary>Add (or replace) a source in the user's file, enabled.</summary>
    public void Add(string name, string url)
    {
        var settings = UserSettings();
        settings.AddOrUpdate(ConfigurationConstants.PackageSources, new SourceItem(name, url));
        RemoveDisabled(settings, name);
        settings.SaveToDisk();
    }

    /// <summary>Remove a source the user's file defines. A source defined in another file cannot be removed here.</summary>
    public void Remove(string root, string name)
    {
        var settings = UserSettings();
        var item = settings.GetSection(ConfigurationConstants.PackageSources)?.Items.OfType<SourceItem>()
            .FirstOrDefault(i => string.Equals(i.Key, name, StringComparison.OrdinalIgnoreCase));
        if (item is null)
        {
            var file = Files(root).FirstOrDefault(f => Defines(f, name));
            throw new InvalidOperationException(file is null
                ? $"no package source is named {name}"
                : $"{name} is defined in {file}, not in the user's NuGet.config; disable it instead");
        }

        settings.Remove(ConfigurationConstants.PackageSources, item);
        RemoveDisabled(settings, name);
        settings.SaveToDisk();
    }

    /// <summary>Enable or disable a source, in the user's file (<c>disabledPackageSources</c>).</summary>
    public void SetEnabled(string root, string name, bool enabled)
    {
        if (!new PackageSourceProvider(Load(root)).LoadPackageSources().Any(s => string.Equals(s.Name, name, StringComparison.OrdinalIgnoreCase)))
        {
            throw new InvalidOperationException($"no package source is named {name}");
        }

        var settings = UserSettings();
        if (enabled)
        {
            RemoveDisabled(settings, name);
        }
        else
        {
            settings.AddOrUpdate(ConfigurationConstants.DisabledPackageSources, new AddItem(name, "true"));
        }

        settings.SaveToDisk();
        if (enabled && !new PackageSourceProvider(Load(root)).IsPackageSourceEnabled(name))
        {
            throw new InvalidOperationException($"{name} is disabled by another NuGet.config of the chain ({string.Join(", ", Files(root).Where(f => f != Path.GetFullPath(UserConfig)))})");
        }
    }

    private Settings UserSettings()
    {
        var full = Path.GetFullPath(UserConfig);
        var dir = Path.GetDirectoryName(full)!;
        Directory.CreateDirectory(dir);
        if (!File.Exists(full))
        {
            File.WriteAllText(full, "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<configuration>\n</configuration>\n");
        }

        return new Settings(dir, Path.GetFileName(full));
    }

    private static void RemoveDisabled(Settings settings, string name)
    {
        if (settings.GetSection(ConfigurationConstants.DisabledPackageSources)?.Items.OfType<AddItem>()
            .FirstOrDefault(i => string.Equals(i.Key, name, StringComparison.OrdinalIgnoreCase)) is { } disabled)
        {
            settings.Remove(ConfigurationConstants.DisabledPackageSources, disabled);
        }
    }

    /// <summary>Whether NuGet.config <paramref name="file"/> adds a source named <paramref name="name"/>.</summary>
    private static bool Defines(string file, string name)
    {
        try
        {
            return XDocument.Load(file).Root?.Element("packageSources")?.Elements("add")
                .Any(a => string.Equals((string?)a.Attribute("key"), name, StringComparison.OrdinalIgnoreCase)) ?? false;
        }
        catch (Exception ex) when (ex is IOException or System.Xml.XmlException or UnauthorizedAccessException)
        {
            return false;
        }
    }
}
