using System.ComponentModel;
using System.Diagnostics;

namespace Eludite.TestBridge;

/// <summary>Finds the vstest.console.dll that runs VSTest projects (the setting test.vstestConsolePath, else the newest SDK's).</summary>
public static class VsTestConsoleLocator
{
    /// <summary>
    /// <paramref name="configured"/> when it exists; else <c>vstest.console.dll</c> of the newest SDK that
    /// <c>dotnet --list-sdks</c> lists; else the newest under the <c>sdk</c> folder beside <paramref name="dotnet"/>.
    /// Null when there is none.
    /// </summary>
    public static string? Locate(string? configured, string? dotnet = null)
    {
        if (configured is { Length: > 0 })
        {
            return File.Exists(configured) ? Path.GetFullPath(configured) : null;
        }

        foreach (var (version, sdkDir) in ListSdks(dotnet ?? "dotnet").OrderByDescending(s => s.Version))
        {
            var candidate = Path.Combine(sdkDir, version.ToString(), "vstest.console.dll");
            if (File.Exists(candidate))
            {
                return candidate;
            }

            // Prerelease SDKs keep their full folder name (10.0.100-rc.1.xxx).
            var prefixed = Directory.Exists(sdkDir)
                ? Directory.GetDirectories(sdkDir, version + "*").Select(d => Path.Combine(d, "vstest.console.dll")).FirstOrDefault(File.Exists)
                : null;
            if (prefixed is not null)
            {
                return prefixed;
            }
        }

        return null;
    }

    /// <summary>Parses <c>dotnet --list-sdks</c> lines (<c>10.0.302 [/usr/share/dotnet/sdk]</c>).</summary>
    public static IReadOnlyList<(Version Version, string Directory)> ParseListSdks(string text)
    {
        ArgumentNullException.ThrowIfNull(text);
        var list = new List<(Version, string)>();
        foreach (var raw in text.Split('\n'))
        {
            var line = raw.Trim();
            var open = line.IndexOf('[', StringComparison.Ordinal);
            var close = line.LastIndexOf(']');
            if (open <= 0 || close <= open)
            {
                continue;
            }

            var versionText = line[..open].Trim();
            var dash = versionText.IndexOf('-', StringComparison.Ordinal);
            if (Version.TryParse(dash < 0 ? versionText : versionText[..dash], out var version))
            {
                list.Add((version, line[(open + 1)..close]));
            }
        }

        return list;
    }

    private static IReadOnlyList<(Version Version, string Directory)> ListSdks(string dotnet)
    {
        try
        {
            using var process = Process.Start(new ProcessStartInfo(dotnet, ["--list-sdks"])
            {
                UseShellExecute = false,
                CreateNoWindow = true,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                Environment = { ["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1", ["DOTNET_NOLOGO"] = "1" },
            });
            if (process is null)
            {
                return [];
            }

            var text = process.StandardOutput.ReadToEnd();
            process.WaitForExit(10_000);
            return ParseListSdks(text);
        }
        catch (Win32Exception)
        {
            return [];
        }
    }
}
