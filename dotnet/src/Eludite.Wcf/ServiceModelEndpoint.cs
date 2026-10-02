namespace Niello.Wcf;

/// <summary>Where in <c>system.serviceModel</c> an endpoint was declared.</summary>
public enum EndpointSource
{
    /// <summary>Under <c>client</c>: an endpoint this application calls.</summary>
    Client,

    /// <summary>Under <c>services/service</c>: an endpoint this application hosts.</summary>
    Service,
}

/// <summary>An <c>&lt;endpoint&gt;</c> element from <c>system.serviceModel</c>.</summary>
/// <param name="Source">Client or service endpoint.</param>
/// <param name="ServiceName">For service endpoints, the owning <c>service/@name</c>; null for client endpoints.</param>
/// <param name="Name">The endpoint <c>name</c>, if any.</param>
/// <param name="Address">The endpoint <c>address</c>, if any (service endpoints often use relative or empty addresses).</param>
/// <param name="Binding">The <c>binding</c>, e.g. <c>basicHttpBinding</c>.</param>
/// <param name="Contract">The <c>contract</c>, e.g. <c>IMetadataExchange</c> or a service interface.</param>
public sealed record ServiceModelEndpoint(
    EndpointSource Source,
    string? ServiceName,
    string? Name,
    string? Address,
    string? Binding,
    string? Contract);
