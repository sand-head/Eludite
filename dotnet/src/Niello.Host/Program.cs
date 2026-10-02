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

        LspProxy? languageServer = null;
        if (!noRoslyn)
        {
            var located = RoslynProcessLauncher.Locate(roslynPath);
            if (located is null)
            {
                await log.WriteLineAsync("Roslyn language server not found (see tools/roslyn-pin); LSP forwarding disabled").ConfigureAwait(false);
            }
            else
            {
                // Brief 0003: legacy projects. NIELLO_LEGACY=0 turns the preparation and the extra environment off.
                if (Environment.GetEnvironmentVariable("NIELLO_LEGACY") == "0")
                {
                    languageServer = new LspProxy(new RoslynProcessLauncher(located, log), log);
                }
                else
                {
                    var legacy = new LegacyDesignTime(log);
                    await log.WriteLineAsync(legacy.Mono is { } mono
                        ? $"[legacy] Mono MSBuild: {mono.MsBuildDll} ({mono.Source})"
                        : "[legacy] Mono MSBuild not found; non-SDK projects load with the .NET SDK's MSBuild").ConfigureAwait(false);
                    languageServer = new LspProxy(
                        new RoslynProcessLauncher(located, log, environment: legacy.RoslynEnvironment()),
                        log,
                        legacy.PrepareAsync);
                }
            }
        }

        var target = new HostRpcTarget(new DotnetCliSdkDiscoverer(), log, languageServer: languageServer);
        await log.WriteLineAsync($"{HostRpcTarget.HostName} {HostRpcTarget.HostVersion} listening on stdio").ConfigureAwait(false);
        return await HostServer.RunAsync(output, input, target).ConfigureAwait(false);
    }
}
