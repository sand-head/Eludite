using Eludite.Host.Legacy;
using Eludite.Host.Lsp;
using Eludite.Host.Rpc;
using Eludite.Host.Sdk;

namespace Eludite.Host;

internal static class Program
{
    private const string Usage = "usage: eludite-host [--stdio] [--roslyn-ls <path to Microsoft.CodeAnalysis.LanguageServer.dll> | --no-roslyn] | --version";

    private static async Task<int> Main(string[] args)
    {
        // stdout carries JSON-RPC only; every diagnostic goes to stderr.
        var log = Console.Error;

        // Brief 0080: the release version (companions.sh publishes with -p:Version=), on stdout because no protocol runs.
        if (args is ["--version"])
        {
            Console.WriteLine($"{HostRpcTarget.HostName} {HostRpcTarget.HostVersion}");
            return 0;
        }

        string? roslynPath = null;
        var noRoslyn = false;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--stdio":
                    break;
                case "--no-roslyn":
                    noRoslyn = true;
                    break;
                case "--roslyn-ls" when i + 1 < args.Length:
                    roslynPath = args[++i];
                    break;
                default:
                    await log.WriteLineAsync(Usage).ConfigureAwait(false);
                    return 2;
            }
        }

        await using var input = Console.OpenStandardInput();
        await using var output = Console.OpenStandardOutput();
        // stdout is protocol only (CLAUDE.md invariant 10). Anything that writes to Console.Out (MSBuild loggers,
        // libraries) lands in the log instead of corrupting the JSON-RPC stream.
        Console.SetOut(log);

        ILanguageServerLauncher? launcher = null;
        ISolutionPreparer? preparer = null;
        if (!noRoslyn)
        {
            // Brief 0080: the argument (the shell's setting languageServers.roslynPath), ELUDITE_ROSLYN_LS, roslyn/ beside
            // the host (the package), then tools/roslyn-pin's output.
            var located = RoslynProcessLauncher.Locate(roslynPath, AppContext.BaseDirectory, Environment.GetEnvironmentVariable, out var source);
            if (located is null)
            {
                await log.WriteLineAsync($"Roslyn language server not found ({source} was the last place looked); LSP forwarding disabled. {RoslynProcessLauncher.NotFoundMessage}").ConfigureAwait(false);
            }
            else if (Environment.GetEnvironmentVariable("ELUDITE_LEGACY") == "0")
            {
                await log.WriteLineAsync($"Roslyn language server ({source}): {located}").ConfigureAwait(false);
                // Brief 0003: ELUDITE_LEGACY=0 turns the legacy preparation and the extra environment off.
                launcher = new RoslynProcessLauncher(located, log);
            }
            else
            {
                await log.WriteLineAsync($"Roslyn language server ({source}): {located}").ConfigureAwait(false);
                var legacy = new LegacyDesignTime(log);
                await log.WriteLineAsync(legacy.Mono is { } mono
                    ? $"[legacy] Mono MSBuild: {mono.MsBuildDll} ({mono.Source})"
                    : "[legacy] Mono MSBuild not found; non-SDK projects load with the .NET SDK's MSBuild").ConfigureAwait(false);
                launcher = new RoslynProcessLauncher(located, log, environment: legacy.RoslynEnvironment());
                preparer = legacy;
            }
        }

        var languageServer = new LspProxy(launcher, log, preparer);
        var target = new HostRpcTarget(new DotnetCliSdkDiscoverer(), log, languageServer: languageServer);
        await log.WriteLineAsync($"{HostRpcTarget.HostName} {HostRpcTarget.HostVersion} listening on stdio").ConfigureAwait(false);
        return await HostServer.RunAsync(output, input, target).ConfigureAwait(false);
    }
}
