using System.Globalization;

namespace Eludite.Host.Legacy;

/// <summary>
/// .NET Framework reference assemblies from the <c>Microsoft.NETFramework.ReferenceAssemblies.net4*</c> NuGet packages
/// (PLAN.md D4), so design-time analysis works where no targeting pack is installed. MSBuild picks them up through
/// <c>TargetFrameworkRootPath</c>, which must contain <c>.NETFramework/vX.Y</c>.
/// </summary>
public sealed class ReferenceAssemblies
{
    private readonly string _packagesRoot;

    public ReferenceAssemblies(string? packagesRoot = null)
    {
        var fromEnv = Environment.GetEnvironmentVariable("NUGET_PACKAGES");
        _packagesRoot = packagesRoot
            ?? (string.IsNullOrEmpty(fromEnv)
                ? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".nuget", "packages")
                : fromEnv);
    }

    /// <summary>
    /// <c>TargetFrameworkRootPath</c> for one framework version (e.g. <c>v4.7.2</c>): the package's <c>build</c>
    /// directory. Null when the package is not in the NuGet cache.
    /// </summary>
    public string? RootFor(string targetFrameworkVersion)
    {
        ArgumentNullException.ThrowIfNull(targetFrameworkVersion);
        var id = "microsoft.netframework.referenceassemblies.net" + targetFrameworkVersion.TrimStart('v', 'V').Replace(".", string.Empty, StringComparison.Ordinal);
        var dir = Path.Combine(_packagesRoot, id);
        if (!Directory.Exists(dir))
        {
            return null;
        }

        foreach (var version in Directory.GetDirectories(dir).OrderByDescending(d => Version.TryParse(Path.GetFileName(d), out var v) ? v : new Version(0, 0)))
        {
            var build = Path.Combine(version, "build");
            if (Directory.Exists(Path.Combine(build, ".NETFramework", NormalizeVersion(targetFrameworkVersion))))
            {
                return build + Path.DirectorySeparatorChar;
            }
        }

        return null;
    }

    /// <summary>
    /// One <c>TargetFrameworkRootPath</c> covering every installed version, for processes that load many projects
    /// (the Roslyn language server's build host). Built as symlinks under <paramref name="cacheDirectory"/>.
    /// Returns null when no package is installed or symlinks cannot be created.
    /// </summary>
    public string? MergedRoot(string cacheDirectory)
    {
        ArgumentNullException.ThrowIfNull(cacheDirectory);
        if (!Directory.Exists(_packagesRoot))
        {
            return null;
        }

        var root = Path.Combine(cacheDirectory, "refasm");
        var framework = Path.Combine(root, ".NETFramework");
        Directory.CreateDirectory(framework);
        var any = false;
        foreach (var package in Directory.GetDirectories(_packagesRoot, "microsoft.netframework.referenceassemblies.net4*"))
        {
            var digits = Path.GetFileName(package)["microsoft.netframework.referenceassemblies.net".Length..];
            var tfv = "v" + string.Join('.', digits.Select(c => c.ToString(CultureInfo.InvariantCulture)));
            if (RootFor(tfv) is not { } build)
            {
                continue;
            }

            var link = Path.Combine(framework, tfv);
            var target = Path.Combine(build, ".NETFramework", tfv);
            try
            {
                if (Directory.Exists(link) || File.Exists(link))
                {
                    if (new DirectoryInfo(link).LinkTarget == target)
                    {
                        any = true;
                        continue;
                    }

                    Directory.Delete(link);
                }

                Directory.CreateSymbolicLink(link, target);
                any = true;
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                // Windows without symlink rights: callers fall back to per-project roots.
                return null;
            }
        }

        return any ? root + Path.DirectorySeparatorChar : null;
    }

    private static string NormalizeVersion(string tfv) => tfv.StartsWith('v') ? tfv : "v" + tfv;
}
