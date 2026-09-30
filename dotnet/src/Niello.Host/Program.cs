using Niello.Host.Rpc;
using Niello.Host.Sdk;

namespace Niello.Host;

internal static class Program
{
    private static async Task<int> Main(string[] args)
    {
        // stdout carries JSON-RPC only; every diagnostic goes to stderr.
        var log = Console.Error;

        if (args.Any(a => a != "--stdio"))
        {
            await log.WriteLineAsync("usage: niello-host [--stdio]").ConfigureAwait(false);
            return 2;
        }

        await using var input = Console.OpenStandardInput();
        await using var output = Console.OpenStandardOutput();

        var target = new HostRpcTarget(new DotnetCliSdkDiscoverer(), log);
        await log.WriteLineAsync($"{HostRpcTarget.HostName} {HostRpcTarget.HostVersion} listening on stdio").ConfigureAwait(false);
        return await HostServer.RunAsync(output, input, target).ConfigureAwait(false);
    }
}
