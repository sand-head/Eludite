using System.Text.Json;
using System.Text.RegularExpressions;
using System.Xml.Linq;
using NuGet.Common;
using NuGet.ProjectModel;

namespace Eludite.Host.NuGet;

/// <summary>
/// A project's packages, read from disk only (host-rpc.md, "NuGet"): <c>obj/project.assets.json</c> when the project is
/// restored (resolved versions, transitive packages, requested ranges, Central Package Management's versions, NuGet
/// Audit's warnings), the global packages folder's <c>.nupkg.metadata</c> for the source, else the project file's
/// <c>PackageReference</c> items (and <c>Directory.Packages.props</c>'s <c>PackageVersion</c> items). A
/// <c>packages.config</c> project is read-only.
/// </summary>
public static partial class InstalledReader
{
    /// <summary>The note a packages.config project carries.</summary>
    public const string PackagesConfigNote =
        "packages.config is listed read-only: migrate it to PackageReference (Visual Studio: right-click packages.config > Migrate packages.config to PackageReference...) to manage its packages here.";

    [GeneratedRegex(@"https?://\S+")]
    private static partial Regex Url();

    /// <summary>Whether a target framework moniker is .NET (Core) 5 or later, or netcoreapp.</summary>
    public static bool IsNetCoreApp(string tfm)
    {
        ArgumentNullException.ThrowIfNull(tfm);
        return global::NuGet.Frameworks.NuGetFramework.Parse(tfm).Framework == global::NuGet.Frameworks.FrameworkConstants.FrameworkIdentifiers.NetCoreApp;
    }

    /// <summary>The project's assets file (MSBuild's default <c>obj/</c> folder).</summary>
    public static string AssetsPath(string projectPath) =>
        Path.Combine(Path.GetDirectoryName(Path.GetFullPath(projectPath))!, "obj", "project.assets.json");

    /// <summary>The <c>Directory.Packages.props</c> from the project's folder up, if any.</summary>
    public static string? FindPropsFile(string projectPath)
    {
        for (var dir = new DirectoryInfo(Path.GetDirectoryName(Path.GetFullPath(projectPath))!); dir is not null; dir = dir.Parent)
        {
            var p = Path.Combine(dir.FullName, "Directory.Packages.props");
            if (File.Exists(p))
            {
                return p;
            }
        }

        return null;
    }

    /// <summary>The project's packages.lock.json, if it has one.</summary>
    public static string? FindLockFile(string projectPath)
    {
        var p = Path.Combine(Path.GetDirectoryName(Path.GetFullPath(projectPath))!, "packages.lock.json");
        return File.Exists(p) ? p : null;
    }

    /// <summary>Reads one project.</summary>
    public static NuGetInstalledProject Read(string projectPath, bool includeTransitive)
    {
        var full = Path.GetFullPath(projectPath);
        var name = Path.GetFileNameWithoutExtension(full);
        var dir = Path.GetDirectoryName(full)!;
        var lockFile = FindLockFile(full);
        try
        {
            var packagesConfig = Path.Combine(dir, "packages.config");
            if (File.Exists(packagesConfig))
            {
                return ReadPackagesConfig(full, name, packagesConfig);
            }

            var assets = AssetsPath(full);
            if (File.Exists(assets))
            {
                return ReadAssets(full, name, assets, includeTransitive) with { LockFile = lockFile };
            }

            return ReadProjectFile(full, name) with { LockFile = lockFile };
        }
        catch (Exception ex) when (ex is IOException or System.Xml.XmlException or UnauthorizedAccessException or JsonException or InvalidDataException or ArgumentException)
        {
            return new NuGetInstalledProject(full, name, "none", false, false, [], []) { Error = ex.Message };
        }
    }

    private static NuGetInstalledProject ReadAssets(string full, string name, string assetsPath, bool includeTransitive)
    {
        var lockFile = new LockFileFormat().Read(assetsPath, NullLogger.Instance);
        var spec = lockFile.PackageSpec;
        var cpm = spec?.RestoreMetadata?.CentralPackageVersionsEnabled ?? false;
        var packagesFolder = lockFile.PackageFolders.FirstOrDefault()?.Path;
        var vulnerabilities = AuditVulnerabilities(lockFile);
        var written = WrittenVersions(full);
        var top = new Dictionary<string, (string? Requested, string? Version, List<string> Frameworks, bool Auto, List<NuGetPackageDependency> Deps)>(StringComparer.OrdinalIgnoreCase);
        var transitive = new Dictionary<string, (string Version, List<string> Frameworks, List<NuGetPackageDependency> Deps)>(StringComparer.OrdinalIgnoreCase);
        var frameworks = new List<string>();
        foreach (var tfm in spec?.TargetFrameworks ?? [])
        {
            var alias = string.IsNullOrEmpty(tfm.TargetAlias) ? tfm.FrameworkName.GetShortFolderName() : tfm.TargetAlias;
            frameworks.Add(alias);
            var target = lockFile.Targets.FirstOrDefault(t => t.RuntimeIdentifier is null && t.TargetFramework == tfm.FrameworkName);
            var libraries = target?.Libraries.Where(l => l.Type == "package").ToDictionary(l => l.Name!, StringComparer.OrdinalIgnoreCase)
                ?? new Dictionary<string, LockFileTargetLibrary>(StringComparer.OrdinalIgnoreCase);
            var direct = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            foreach (var dep in tfm.Dependencies)
            {
                if (!dep.LibraryRange.TypeConstraintAllows(global::NuGet.LibraryModel.LibraryDependencyTarget.Package))
                {
                    continue;
                }

                direct.Add(dep.Name);
                string? requested = written.GetValueOrDefault(dep.Name)
                    ?? dep.VersionOverride?.OriginalString
                    ?? (dep.VersionCentrallyManaged && tfm.CentralPackageVersions.TryGetValue(dep.Name, out var central) ? central.VersionRange.OriginalString : null)
                    ?? dep.LibraryRange.VersionRange?.OriginalString
                    ?? dep.LibraryRange.VersionRange?.ToNormalizedString();
                libraries.TryGetValue(dep.Name, out var lib);
                if (!top.TryGetValue(dep.Name, out var entry))
                {
                    entry = (requested, lib?.Version?.ToNormalizedString(), [], dep.AutoReferenced, Dependencies(lib));
                }

                entry.Frameworks.Add(alias);
                top[dep.Name] = entry;
            }

            foreach (var lib in libraries.Values.Where(l => !direct.Contains(l.Name!)))
            {
                if (!transitive.TryGetValue(lib.Name!, out var entry))
                {
                    entry = (lib.Version!.ToNormalizedString(), [], Dependencies(lib));
                }

                entry.Frameworks.Add(alias);
                transitive[lib.Name!] = entry;
            }
        }

        var packages = new List<NuGetInstalledPackage>();
        foreach (var (id, e) in top.OrderBy(k => k.Key, StringComparer.OrdinalIgnoreCase))
        {
            packages.Add(new NuGetInstalledPackage(id, e.Frameworks, false)
            {
                Requested = e.Requested,
                Version = e.Version,
                Source = e.Version is null ? null : SourceOf(packagesFolder, id, e.Version),
                AutoReferenced = e.Auto,
                Dependencies = e.Deps.Count > 0 ? e.Deps : null,
                Vulnerabilities = vulnerabilities.GetValueOrDefault(id),
            });
        }

        if (includeTransitive)
        {
            foreach (var (id, e) in transitive.Where(t => !top.ContainsKey(t.Key)).OrderBy(k => k.Key, StringComparer.OrdinalIgnoreCase))
            {
                packages.Add(new NuGetInstalledPackage(id, e.Frameworks, true)
                {
                    Version = e.Version,
                    Source = SourceOf(packagesFolder, id, e.Version),
                    Dependencies = e.Deps.Count > 0 ? e.Deps : null,
                    Vulnerabilities = vulnerabilities.GetValueOrDefault(id),
                });
            }
        }

        return new NuGetInstalledProject(full, name, packages.Count > 0 || top.Count > 0 ? "packageReference" : "none", true, cpm, frameworks, packages)
        {
            AssetsFile = assetsPath,
            PropsFile = cpm ? FindPropsFile(full) : null,
        };
    }

    private static List<NuGetPackageDependency> Dependencies(LockFileTargetLibrary? lib) =>
        lib is null ? [] : [.. lib.Dependencies.Select(d => new NuGetPackageDependency(d.Id) { Range = d.VersionRange?.OriginalString ?? d.VersionRange?.ToNormalizedString() })];

    /// <summary>NuGet Audit's NU1901 to NU1904 warnings, by package id.</summary>
    public static Dictionary<string, IReadOnlyList<NuGetVulnerability>> AuditVulnerabilities(LockFile lockFile)
    {
        ArgumentNullException.ThrowIfNull(lockFile);
        var found = new Dictionary<string, IReadOnlyList<NuGetVulnerability>>(StringComparer.OrdinalIgnoreCase);
        foreach (var m in lockFile.LogMessages)
        {
            var severity = m.Code switch
            {
                NuGetLogCode.NU1901 => "low",
                NuGetLogCode.NU1902 => "moderate",
                NuGetLogCode.NU1903 => "high",
                NuGetLogCode.NU1904 => "critical",
                _ => null,
            };
            if (severity is null || string.IsNullOrEmpty(m.LibraryId))
            {
                continue;
            }

            var url = Url().Match(m.Message ?? string.Empty) is { Success: true } u ? u.Value.TrimEnd('.', ',', ')') : string.Empty;
            var list = found.TryGetValue(m.LibraryId, out var l) ? [.. l] : new List<NuGetVulnerability>();
            if (!list.Any(v => v.AdvisoryUrl == url))
            {
                list.Add(new NuGetVulnerability(severity, url));
            }

            found[m.LibraryId] = list;
        }

        return found;
    }

    /// <summary>The source a package was restored from, from the global packages folder's <c>.nupkg.metadata</c>.</summary>
    public static string? SourceOf(string? packagesFolder, string id, string version)
    {
        if (string.IsNullOrEmpty(packagesFolder))
        {
            return null;
        }

        var file = Path.Combine(packagesFolder, id.ToLowerInvariant(), version.ToLowerInvariant(), ".nupkg.metadata");
        try
        {
            using var doc = JsonDocument.Parse(File.ReadAllText(file));
            return doc.RootElement.TryGetProperty("source", out var s) ? s.GetString() : null;
        }
        catch (Exception ex) when (ex is IOException or JsonException or UnauthorizedAccessException)
        {
            return null;
        }
    }

    /// <summary>The versions the project file (or its Directory.Packages.props) writes for its PackageReference items.</summary>
    private static Dictionary<string, string> WrittenVersions(string full)
    {
        var result = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        try
        {
            var doc = XDocument.Load(full);
            var props = FindPropsFile(full);
            var central = ManagesCentrally(doc, props) && props is not null ? CentralVersions(props) : [];
            foreach (var item in doc.Descendants().Where(e => e.Name.LocalName == "PackageReference"))
            {
                if (((string?)item.Attribute("Include") ?? (string?)item.Attribute("Update")) is { Length: > 0 } id
                    && (Metadata(item, "VersionOverride") ?? Metadata(item, "Version") ?? central.GetValueOrDefault(id)) is { } v)
                {
                    result[id] = v;
                }
            }
        }
        catch (Exception ex) when (ex is IOException or System.Xml.XmlException or UnauthorizedAccessException)
        {
            // The assets file's ranges stand.
        }

        return result;
    }

    private static NuGetInstalledProject ReadProjectFile(string full, string name)
    {
        var doc = XDocument.Load(full);
        var props = FindPropsFile(full);
        var cpm = ManagesCentrally(doc, props);
        var central = cpm && props is not null ? CentralVersions(props) : new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var frameworks = (doc.Descendants().FirstOrDefault(e => e.Name.LocalName is "TargetFramework" or "TargetFrameworks")?.Value ?? string.Empty)
            .Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
        var packages = new List<NuGetInstalledPackage>();
        foreach (var item in doc.Descendants().Where(e => e.Name.LocalName == "PackageReference"))
        {
            var id = (string?)item.Attribute("Include") ?? (string?)item.Attribute("Update");
            if (string.IsNullOrEmpty(id) || packages.Any(p => string.Equals(p.Id, id, StringComparison.OrdinalIgnoreCase)))
            {
                continue;
            }

            var requested = Metadata(item, "VersionOverride") ?? Metadata(item, "Version") ?? central.GetValueOrDefault(id);
            packages.Add(new NuGetInstalledPackage(id, frameworks, false) { Requested = requested });
        }

        return new NuGetInstalledProject(full, name, packages.Count > 0 ? "packageReference" : "none", false, cpm, frameworks, packages)
        {
            PropsFile = cpm ? props : null,
        };
    }

    private static NuGetInstalledProject ReadPackagesConfig(string full, string name, string packagesConfig)
    {
        var doc = XDocument.Load(packagesConfig);
        var packages = doc.Root?.Elements("package")
            .Select(p => new NuGetInstalledPackage((string?)p.Attribute("id") ?? "?", (string?)p.Attribute("targetFramework") is { } tf ? [tf] : [], false)
            {
                Requested = (string?)p.Attribute("version"),
                Version = (string?)p.Attribute("version"),
            })
            .ToList() ?? [];
        return new NuGetInstalledProject(full, name, "packagesConfig", false, false, [], packages) { Note = PackagesConfigNote };
    }

    /// <summary>A metadata value written as an attribute or a child element.</summary>
    public static string? Metadata(XElement item, string name)
    {
        ArgumentNullException.ThrowIfNull(item);
        return (string?)item.Attribute(name) ?? item.Elements().FirstOrDefault(e => e.Name.LocalName == name)?.Value;
    }

    /// <summary>Whether ManagePackageVersionsCentrally is true in the project or its Directory.Packages.props.</summary>
    public static bool ManagesCentrally(XDocument project, string? propsFile)
    {
        ArgumentNullException.ThrowIfNull(project);
        static bool? Flag(XDocument d) => d.Descendants().LastOrDefault(e => e.Name.LocalName == "ManagePackageVersionsCentrally") is { } e
            ? string.Equals(e.Value.Trim(), "true", StringComparison.OrdinalIgnoreCase)
            : null;
        if (Flag(project) is { } own)
        {
            return own;
        }

        return propsFile is not null && (Flag(XDocument.Load(propsFile)) ?? false);
    }

    /// <summary>Directory.Packages.props's PackageVersion items.</summary>
    public static Dictionary<string, string> CentralVersions(string propsFile)
    {
        var result = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (var item in XDocument.Load(propsFile).Descendants().Where(e => e.Name.LocalName == "PackageVersion"))
        {
            if ((string?)item.Attribute("Include") is { } id && Metadata(item, "Version") is { } v)
            {
                result[id] = v;
            }
        }

        return result;
    }
}
