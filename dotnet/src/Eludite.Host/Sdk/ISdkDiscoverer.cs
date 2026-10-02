namespace Eludite.Host.Sdk;

/// <summary>Finds the .NET SDKs installed on this machine.</summary>
public interface ISdkDiscoverer
{
    Task<IReadOnlyList<DotnetSdk>> DiscoverAsync(CancellationToken cancellationToken);
}
