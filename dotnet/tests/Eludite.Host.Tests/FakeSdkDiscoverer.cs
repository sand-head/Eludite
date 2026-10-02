using Niello.Host.Sdk;

namespace Niello.Host.Tests;

internal sealed class FakeSdkDiscoverer(params DotnetSdk[] sdks) : ISdkDiscoverer
{
    public int Calls { get; private set; }

    public Task<IReadOnlyList<DotnetSdk>> DiscoverAsync(CancellationToken cancellationToken)
    {
        Calls++;
        return Task.FromResult<IReadOnlyList<DotnetSdk>>(sdks);
    }
}
