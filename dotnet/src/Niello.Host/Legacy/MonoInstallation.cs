namespace Niello.Host.Legacy;

/// <summary>
/// A located Mono runtime with Mono's MSBuild (PLAN.md D4: located, not bundled, unless brief 0003 says otherwise).
/// <see cref="Environment"/> holds the variables a relocated (user-space) Mono needs to find its own libraries.
/// </summary>
public sealed record MonoInstallation(string Prefix, string MonoExecutable, string MsBuildDll, string Source, IReadOnlyDictionary<string, string> Environment)
{
    /// <summary>Default user-space prefix used by tools/legacy-load (extracted Arch packages; see its README).</summary>
    public static string UserSpacePrefix => Path.Combine(
        System.Environment.GetFolderPath(System.Environment.SpecialFolder.UserProfile), ".local", "opt", "mono-root", "usr");

    /// <summary>
    /// Finds Mono and its MSBuild: <c>NIELLO_MONO_PREFIX</c>, then <c>mono</c> on <c>PATH</c>, then
    /// <see cref="UserSpacePrefix"/>, then <c>/usr</c>, <c>/usr/local</c> and the macOS framework. Returns null on Windows
    /// or when none has <c>lib/mono/msbuild/{Current,15.0}/bin/MSBuild.dll</c>.
    /// </summary>
    public static MonoInstallation? Locate()
    {
        if (OperatingSystem.IsWindows())
        {
            return null;
        }

        var candidates = new List<(string Prefix, string Source)>();
        if (System.Environment.GetEnvironmentVariable("NIELLO_MONO_PREFIX") is { Length: > 0 } explicitPrefix)
        {
            candidates.Add((explicitPrefix, "NIELLO_MONO_PREFIX"));
        }

        foreach (var dir in (System.Environment.GetEnvironmentVariable("PATH") ?? string.Empty).Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            var mono = Path.Combine(dir, "mono");
            if (File.Exists(mono))
            {
                var real = new FileInfo(mono).ResolveLinkTarget(returnFinalTarget: true)?.FullName ?? mono;
                candidates.Add((Path.GetFullPath(Path.Combine(Path.GetDirectoryName(real)!, "..")), "PATH"));
            }
        }

        candidates.Add((UserSpacePrefix, "user-space default"));
        candidates.Add(("/usr", "system"));
        candidates.Add(("/usr/local", "system"));
        candidates.Add(("/Library/Frameworks/Mono.framework/Versions/Current", "system"));

        foreach (var (prefix, source) in candidates)
        {
            var monoExe = Path.Combine(prefix, "bin", "mono");
            var msbuild = new[] { "Current", "15.0" }
                .Select(v => Path.Combine(prefix, "lib", "mono", "msbuild", v, "bin", "MSBuild.dll"))
                .FirstOrDefault(File.Exists);
            if (File.Exists(monoExe) && msbuild is not null)
            {
                return new MonoInstallation(prefix, monoExe, msbuild, source, EnvironmentFor(prefix));
            }
        }

        return null;
    }

    /// <summary>Variables for running a Mono that is not installed at its configured prefix (/usr).</summary>
    public static IReadOnlyDictionary<string, string> EnvironmentFor(string prefix)
    {
        var bin = Path.Combine(prefix, "bin");
        var path = System.Environment.GetEnvironmentVariable("PATH") ?? string.Empty;
        var env = new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["PATH"] = path.Split(Path.PathSeparator).Contains(bin) ? path : bin + Path.PathSeparator + path,
        };
        if (!string.Equals(Path.GetFullPath(prefix).TrimEnd('/'), "/usr", StringComparison.Ordinal))
        {
            var lib = Path.Combine(prefix, "lib");
            var ld = System.Environment.GetEnvironmentVariable("LD_LIBRARY_PATH");
            env["LD_LIBRARY_PATH"] = string.IsNullOrEmpty(ld) ? lib : lib + Path.PathSeparator + ld;
            env["MONO_CFG_DIR"] = Path.GetFullPath(Path.Combine(prefix, "..", "etc"));
            env["MONO_GAC_PREFIX"] = prefix;
        }

        return env;
    }
}
