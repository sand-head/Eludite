using System.Collections.Concurrent;
using NuGet.Common;
using NuGet.Configuration;
using NuGet.Protocol;
using NuGet.Protocol.Core.Types;
using NuGet.Versioning;

namespace Eludite.Host.NuGet;

/// <summary>What one source answered for one package version: its vulnerabilities and deprecation.</summary>
public sealed record VersionMetadata(NuGetVersion Version, IReadOnlyList<NuGetVulnerability> Vulnerabilities, NuGetDeprecation? Deprecation);

/// <summary>
/// The configured sources' resources (search, package versions, registration metadata) through NuGet.Protocol, each
/// answer cached per source for the host's session: a repeated search, version list or metadata lookup asks nothing.
/// Failures are never cached. NuGet's own HTTP cache is bypassed (<see cref="SourceCacheContext.NoCache"/>,
/// <see cref="SourceCacheContext.DirectDownload"/>): this session cache is the cache.
/// </summary>
public sealed class NuGetFeeds
{
    private readonly IEnumerable<Lazy<INuGetResourceProvider>> _providers;
    private readonly ConcurrentDictionary<string, SourceRepository> _repositories = new(StringComparer.Ordinal);
    private readonly ConcurrentDictionary<string, IReadOnlyList<NuGetSearchPackage>> _searches = new(StringComparer.Ordinal);
    private readonly ConcurrentDictionary<string, IReadOnlyList<NuGetVersion>> _versions = new(StringComparer.Ordinal);
    private readonly ConcurrentDictionary<string, IReadOnlyList<VersionMetadata>> _metadata = new(StringComparer.Ordinal);

    /// <param name="extraProviders">Resource providers tried before NuGet's own (tests).</param>
    public NuGetFeeds(IEnumerable<Lazy<INuGetResourceProvider>>? extraProviders = null)
    {
        _providers = [.. extraProviders ?? [], .. global::NuGet.Protocol.Core.Types.Repository.Provider.GetCoreV3()];
        Cache = new SourceCacheContext { NoCache = true, DirectDownload = true };
    }

    public SourceCacheContext Cache { get; }

    /// <summary>Requests made to sources (cache misses), for the "nothing at startup" and cache tests.</summary>
    public int Requests => _requests;

    private int _requests;

    private SourceRepository RepositoryFor(PackageSource source) =>
        _repositories.GetOrAdd($"{source.Name}|{source.Source}", _ => global::NuGet.Protocol.Core.Types.Repository.CreateSource(_providers, source));

    private static string Key(PackageSource source, params object?[] parts) =>
        string.Join('|', new object?[] { source.Name, source.Source }.Concat(parts));

    /// <summary>Searches one source. Returns the packages and whether the cache answered.</summary>
    public async Task<(IReadOnlyList<NuGetSearchPackage> Packages, bool Cached)> SearchAsync(PackageSource source, string query, bool prerelease, int skip, int take, ILogger log, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(source);
        var key = Key(source, query, prerelease, skip, take);
        if (_searches.TryGetValue(key, out var hit))
        {
            return (hit, true);
        }

        Interlocked.Increment(ref _requests);
        var resource = await RepositoryFor(source).GetResourceAsync<PackageSearchResource>(cancellationToken).ConfigureAwait(false)
            ?? throw new InvalidOperationException($"{source.Name} has no search resource");
        var found = await resource.SearchAsync(query, new SearchFilter(prerelease), skip, take, log, cancellationToken).ConfigureAwait(false);
        var list = new List<NuGetSearchPackage>();
        foreach (var m in found)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var versions = (await m.GetVersionsAsync().ConfigureAwait(false) ?? [])
                .Select(v => v.Version)
                .Where(v => prerelease || !v.IsPrerelease)
                .Distinct()
                .OrderByDescending(v => v)
                .Select(v => v.ToNormalizedString())
                .ToList();
            var deprecation = Deprecation(await SafeDeprecationAsync(m).ConfigureAwait(false));
            list.Add(new NuGetSearchPackage(m.Identity.Id, m.Identity.Version.ToNormalizedString(), source.Name)
            {
                Versions = versions.Count > 0 ? versions : [m.Identity.Version.ToNormalizedString()],
                Title = string.IsNullOrEmpty(m.Title) ? null : m.Title,
                Description = string.IsNullOrEmpty(m.Description) ? null : m.Description,
                Authors = string.IsNullOrEmpty(m.Authors) ? null : m.Authors,
                IconUrl = m.IconUrl?.ToString(),
                LicenseUrl = m.LicenseUrl?.ToString(),
                LicenseExpression = m.LicenseMetadata?.License,
                ProjectUrl = m.ProjectUrl?.ToString(),
                Downloads = m.DownloadCount,
                Vulnerabilities = Vulnerabilities(m.Vulnerabilities),
                Deprecation = deprecation,
            });
        }

        _searches[key] = list;
        return (list, false);
    }

    /// <summary>Every version one source lists for <paramref name="id"/>, newest first.</summary>
    public async Task<(IReadOnlyList<NuGetVersion> Versions, bool Cached)> VersionsAsync(PackageSource source, string id, ILogger log, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(source);
        ArgumentNullException.ThrowIfNull(id);
        var key = Key(source, id.ToLowerInvariant());
        if (_versions.TryGetValue(key, out var hit))
        {
            return (hit, true);
        }

        Interlocked.Increment(ref _requests);
        var resource = await RepositoryFor(source).GetResourceAsync<FindPackageByIdResource>(cancellationToken).ConfigureAwait(false)
            ?? throw new InvalidOperationException($"{source.Name} has no package versions resource");
        var versions = (await resource.GetAllVersionsAsync(id, Cache, log, cancellationToken).ConfigureAwait(false) ?? [])
            .Distinct()
            .OrderByDescending(v => v)
            .ToList();
        _versions[key] = versions;
        return (versions, false);
    }

    /// <summary>The package's id as its package writes it (the first source that has the version), or null.</summary>
    public async Task<string?> CanonicalIdAsync(IEnumerable<PackageSource> sources, string id, NuGetVersion version, ILogger log, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(sources);
        foreach (var source in sources)
        {
            try
            {
                var resource = await RepositoryFor(source).GetResourceAsync<FindPackageByIdResource>(cancellationToken).ConfigureAwait(false);
                if (resource is not null && await resource.GetDependencyInfoAsync(id, version, Cache, log, cancellationToken).ConfigureAwait(false) is { } info)
                {
                    return info.PackageIdentity.Id;
                }
            }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                // Another source may have it.
            }
        }

        return null;
    }

    /// <summary>The registration data of every version of <paramref name="id"/> one source lists.</summary>
    public async Task<(IReadOnlyList<VersionMetadata> Versions, bool Cached)> MetadataAsync(PackageSource source, string id, ILogger log, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(source);
        ArgumentNullException.ThrowIfNull(id);
        var key = Key(source, id.ToLowerInvariant());
        if (_metadata.TryGetValue(key, out var hit))
        {
            return (hit, true);
        }

        Interlocked.Increment(ref _requests);
        var resource = await RepositoryFor(source).GetResourceAsync<PackageMetadataResource>(cancellationToken).ConfigureAwait(false)
            ?? throw new InvalidOperationException($"{source.Name} has no registration resource");
        var all = await resource.GetMetadataAsync(id, includePrerelease: true, includeUnlisted: false, Cache, log, cancellationToken).ConfigureAwait(false) ?? [];
        var list = new List<VersionMetadata>();
        foreach (var m in all)
        {
            list.Add(new VersionMetadata(m.Identity.Version, Vulnerabilities(m.Vulnerabilities) ?? [], Deprecation(await SafeDeprecationAsync(m).ConfigureAwait(false))));
        }

        _metadata[key] = list;
        return (list, false);
    }

    private static async Task<PackageDeprecationMetadata?> SafeDeprecationAsync(IPackageSearchMetadata m)
    {
        try
        {
            return await m.GetDeprecationMetadataAsync().ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is NotImplementedException or NotSupportedException or InvalidOperationException)
        {
            return null;
        }
    }

    /// <summary>NuGet's severities (0 low to 3 critical) as the protocol names them.</summary>
    public static string Severity(int severity) => severity switch
    {
        0 => "low",
        1 => "moderate",
        2 => "high",
        _ => "critical",
    };

    private static IReadOnlyList<NuGetVulnerability>? Vulnerabilities(IEnumerable<PackageVulnerabilityMetadata>? found)
    {
        var list = found?.Select(v => new NuGetVulnerability(Severity(v.Severity), v.AdvisoryUrl?.ToString() ?? string.Empty)).ToList();
        return list is { Count: > 0 } ? list : null;
    }

    private static NuGetDeprecation? Deprecation(PackageDeprecationMetadata? d)
    {
        if (d is null)
        {
            return null;
        }

        var reasons = (d.Reasons ?? []).Select(r => r.ToLowerInvariant() switch
        {
            "legacy" => "legacy",
            "criticalbugs" => "criticalBugs",
            _ => "other",
        }).Distinct().ToList();
        return new NuGetDeprecation(reasons.Count > 0 ? reasons : ["other"])
        {
            Message = string.IsNullOrEmpty(d.Message) ? null : d.Message,
            AlternatePackage = d.AlternatePackage is { PackageId: { Length: > 0 } alt } ? new NuGetAlternatePackage(alt) { Range = d.AlternatePackage.Range?.OriginalString } : null,
        };
    }
}
