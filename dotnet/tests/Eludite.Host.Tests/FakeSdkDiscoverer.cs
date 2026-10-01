using Eludite.Host.Sdk;

namespace Eludite.Host.Tests;

internal sealed class FakeSdkDiscoverer(params DotnetSdk[] sdks) : ISdkDiscoverer
{
    public int Calls { get; private set; }

    public Task<IReadOnlyList<DotnetSdk>> DiscoverAsync(CancellationToken cancellationToken)
    {
        Calls++;
        return Task.FromResult<IReadOnlyList<DotnetSdk>>(sdks);
    }
}
