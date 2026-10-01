using System.Diagnostics;

namespace Niello.Host.Sdk;

/// <summary>Discovers SDKs by running <c>dotnet --list-sdks</c> and parsing its output.</summary>
public sealed class DotnetCliSdkDiscoverer : ISdkDiscoverer
{
    private readonly string _dotnetPath;

    public DotnetCliSdkDiscoverer(string dotnetPath = "dotnet")
    {
        _dotnetPath = dotnetPath;
    }

    public async Task<IReadOnlyList<DotnetSdk>> DiscoverAsync(CancellationToken cancellationToken)
    {
        var startInfo = new ProcessStartInfo(_dotnetPath, "--list-sdks")
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };

        using var process = new Process { StartInfo = startInfo };
        try
        {
            process.Start();
        }
        catch (System.ComponentModel.Win32Exception)
        {
            // dotnet is not on PATH; report no SDKs rather than failing the request.
            return [];
        }

        var stdout = process.StandardOutput.ReadToEndAsync(cancellationToken);
        var stderr = process.StandardError.ReadToEndAsync(cancellationToken);
        await process.WaitForExitAsync(cancellationToken).ConfigureAwait(false);
        await stderr.ConfigureAwait(false);

        return process.ExitCode == 0 ? Parse(await stdout.ConfigureAwait(false)) : [];
    }

    /// <summary>Parses lines of the form <c>10.0.302 [/usr/share/dotnet/sdk]</c>; malformed lines are skipped.</summary>
    public static IReadOnlyList<DotnetSdk> Parse(string output)
    {
        ArgumentNullException.ThrowIfNull(output);

        var sdks = new List<DotnetSdk>();
        foreach (var rawLine in output.Split('\n'))
        {
            var line = rawLine.Trim();
            var open = line.IndexOf(" [", StringComparison.Ordinal);
            if (open <= 0 || !line.EndsWith(']'))
            {
                continue;
            }

            var version = line[..open].Trim();
            var path = line[(open + 2)..^1].Trim();
            if (version.Length > 0 && path.Length > 0)
            {
                sdks.Add(new DotnetSdk(version, path));
            }
        }

        return sdks;
    }
}
