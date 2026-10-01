using Niello.Host.Legacy;
using Niello.Host.Lsp;
using Niello.Host.Rpc;
using Niello.Host.Sdk;

namespace Niello.Host;

internal static class Program
{
    private const string Usage = "usage: niello-host [--stdio] [--roslyn-ls <path to Microsoft.CodeAnalysis.LanguageServer.dll> | --no-roslyn]";

    private static async Task<int> Main(string[] args)
    {
        // stdout carries JSON-RPC only; every diagnostic goes to stderr.
        var log = Console.Error;

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
            var located = RoslynProcessLauncher.Locate(roslynPath);
            if (located is null)
            {
                await log.WriteLineAsync("Roslyn language server not found (see tools/roslyn-pin); LSP forwarding disabled").ConfigureAwait(false);
            }
            else if (Environment.GetEnvironmentVariable("NIELLO_LEGACY") == "0")
            {
                // Brief 0003: NIELLO_LEGACY=0 turns the legacy preparation and the extra environment off.
                launcher = new RoslynProcessLauncher(located, log);
            }
            else
            {
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
