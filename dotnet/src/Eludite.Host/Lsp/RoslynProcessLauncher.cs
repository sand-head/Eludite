using System.Diagnostics;

namespace Eludite.Host.Lsp;

/// <summary>
/// Spawns <c>Microsoft.CodeAnalysis.LanguageServer.dll</c> (built from source, see tools/roslyn-pin)
/// as a child process speaking LSP over its stdio. The child's stderr is copied to the host log;
/// its stdout never reaches the host's stdout.
/// </summary>
public sealed class RoslynProcessLauncher : ILanguageServerLauncher
{
    private readonly string _serverDllPath;
    private readonly TextWriter _log;
    private readonly string _logDirectory;
    private readonly IReadOnlyDictionary<string, string>? _environment;

    /// <param name="environment">
    /// Extra environment for the server process (brief 0003: Mono on PATH, reference-assembly root, designer injection).
    /// MSBuild in Roslyn's build hosts reads these as properties.
    /// </param>
    public RoslynProcessLauncher(string serverDllPath, TextWriter log, string? logDirectory = null, IReadOnlyDictionary<string, string>? environment = null)
    {
        _serverDllPath = serverDllPath;
        _log = log;
        _logDirectory = logDirectory ?? Path.Combine(Path.GetTempPath(), "eludite-host", "roslyn-logs");
        _environment = environment;
    }

    /// <summary>The server's file name.</summary>
    public const string ServerDll = "Microsoft.CodeAnalysis.LanguageServer.dll";

    /// <summary>The folder beside the host's own assembly that a package ships the server in (brief 0080, ADR-0015).</summary>
    public const string BundledFolder = "roslyn";

    /// <summary>What the person reads when no server was found (ELUDITE0001 and the language server status).</summary>
    public const string NotFoundMessage =
        "C# and Visual Basic IntelliSense is unavailable: the Roslyn language server was not found beside eludite-host " +
        "(eludite-host/roslyn/), at the setting languageServers.roslynPath (Tools > Options > Language servers) or at " +
        "ELUDITE_ROSLYN_LS. See docs/user/troubleshooting.md#roslyn.";

    /// <summary>Where <see cref="Locate(string?)"/> found the server.</summary>
    public enum Source
    {
        /// <summary><c>--roslyn-ls</c>: the shell passes the setting <c>languageServers.roslynPath</c> this way.</summary>
        Argument,

        /// <summary><c>ELUDITE_ROSLYN_LS</c>.</summary>
        Environment,

        /// <summary><c>roslyn/</c> beside the host (the package layout, tools/package/companions.sh).</summary>
        BesideHost,

        /// <summary>The tools/roslyn-pin output under <c>ROSLYN_SRC_DIR</c> (default ~/.cache/eludite/roslyn).</summary>
        Cache,
    }

    /// <summary>
    /// Resolves the language server DLL (brief 0080): <c>--roslyn-ls</c>, then <c>ELUDITE_ROSLYN_LS</c>, then
    /// <c>roslyn/</c> beside the host's own assembly, then the default tools/roslyn-pin output under
    /// <c>ROSLYN_SRC_DIR</c> (default ~/.cache/eludite/roslyn). The explicit choices win over the shipped copy; a named
    /// path that does not exist is null rather than a silent fallback. Returns null when none exists.
    /// </summary>
    public static string? Locate(string? explicitPath) =>
        Locate(explicitPath, AppContext.BaseDirectory, System.Environment.GetEnvironmentVariable, out _);

    /// <summary>The search with its inputs given (tests), and where the server was found.</summary>
    public static string? Locate(string? explicitPath, string? hostDirectory, Func<string, string?> environment, out Source source)
    {
        source = Source.Argument;
        if (!string.IsNullOrEmpty(explicitPath))
        {
            return File.Exists(explicitPath) ? Path.GetFullPath(explicitPath) : null;
        }

        source = Source.Environment;
        var fromEnv = environment("ELUDITE_ROSLYN_LS");
        if (!string.IsNullOrEmpty(fromEnv))
        {
            return File.Exists(fromEnv) ? Path.GetFullPath(fromEnv) : null;
        }

        source = Source.BesideHost;
        if (!string.IsNullOrEmpty(hostDirectory))
        {
            var bundled = Path.Combine(hostDirectory, BundledFolder, ServerDll);
            if (File.Exists(bundled))
            {
                return Path.GetFullPath(bundled);
            }
        }

        source = Source.Cache;
        var src = environment("ROSLYN_SRC_DIR");
        if (string.IsNullOrEmpty(src))
        {
            src = Path.Combine(System.Environment.GetFolderPath(System.Environment.SpecialFolder.UserProfile), ".cache", "eludite", "roslyn");
        }

        var candidate = Path.Combine(
            src, "artifacts", "bin", "Microsoft.CodeAnalysis.LanguageServer", "Release", "net10.0", ServerDll);
        return File.Exists(candidate) ? candidate : null;
    }

    public Task<LanguageServerConnection> LaunchAsync(CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(_logDirectory);
        var psi = new ProcessStartInfo(DotnetMuxer())
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        // Variables Microsoft.Build.Locator sets in this process (or a parent) would point the server's build hosts
        // at the wrong MSBuild; the build hosts locate their own.
        foreach (var name in Legacy.DesignTimeProperties.LocatorVariables)
        {
            psi.Environment.Remove(name);
        }

        foreach (var (key, value) in _environment ?? new Dictionary<string, string>())
        {
            psi.Environment[key] = value;
        }

        psi.ArgumentList.Add(_serverDllPath);
        psi.ArgumentList.Add("--stdio");
        psi.ArgumentList.Add("--logLevel");
        psi.ArgumentList.Add(Environment.GetEnvironmentVariable("ELUDITE_ROSLYN_LOGLEVEL") is { Length: > 0 } level ? level : "Information");
        psi.ArgumentList.Add("--telemetryLevel");
        psi.ArgumentList.Add("off");
        psi.ArgumentList.Add("--extensionLogDirectory");
        psi.ArgumentList.Add(_logDirectory);
        psi.ArgumentList.Add("--clientProcessId");
        psi.ArgumentList.Add(Environment.ProcessId.ToString(System.Globalization.CultureInfo.InvariantCulture));

        var process = Process.Start(psi) ?? throw new InvalidOperationException("failed to start the Roslyn language server");
        _log.WriteLine($"roslyn-ls started (pid {process.Id}): {_serverDllPath}");
        var stderrPump = PumpStderrAsync(process);
        return Task.FromResult(new LanguageServerConnection(
            process.StandardInput.BaseStream,
            process.StandardOutput.BaseStream,
            new ProcessLifetime(process, stderrPump, _log)));
    }

    private async Task PumpStderrAsync(Process process)
    {
        try
        {
            while (await process.StandardError.ReadLineAsync().ConfigureAwait(false) is { } line)
            {
                await _log.WriteLineAsync($"[roslyn-ls] {line}").ConfigureAwait(false);
            }
        }
        catch (IOException)
        {
            // Process gone.
        }
        catch (ObjectDisposedException)
        {
            // Process disposed while reading.
        }
    }

    private static string DotnetMuxer()
    {
        var hostPath = Environment.GetEnvironmentVariable("DOTNET_HOST_PATH");
        if (!string.IsNullOrEmpty(hostPath) && File.Exists(hostPath))
        {
            return hostPath;
        }

        var self = Environment.ProcessPath;
        if (self is not null && Path.GetFileNameWithoutExtension(self).Equals("dotnet", StringComparison.OrdinalIgnoreCase))
        {
            return self;
        }

        return "dotnet";
    }

    private sealed class ProcessLifetime(Process process, Task stderrPump, TextWriter log) : IAsyncDisposable
    {
        public async ValueTask DisposeAsync()
        {
            try
            {
                using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
                await process.WaitForExitAsync(timeout.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                log.WriteLine("roslyn-ls did not exit in 5 s; killing it");
                process.Kill(entireProcessTree: true);
            }

            await stderrPump.ConfigureAwait(false);
            process.Dispose();
        }
    }
}
