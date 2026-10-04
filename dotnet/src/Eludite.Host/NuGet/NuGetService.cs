using System.Collections.Concurrent;
using System.Diagnostics;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;
using NuGet.Configuration;
using NuGet.Protocol.Core.Types;
using NuGet.Versioning;
using StreamJsonRpc;

namespace Eludite.Host.NuGet;

/// <summary>
/// <c>eludite/nuget/*</c> (brief 0048; host-rpc.md, "NuGet"): search, installed, updates, change, sources and restore
/// for the open solution, with the generation rule and cancellation on every call, NuGet's output streamed as
/// <c>eludite/nuget/update</c> notifications, and the shell asked for credentials through <c>eludite/nuget/credentials</c>
/// during interactive calls. Nothing here runs until a call arrives: no source is contacted at startup.
/// </summary>
public sealed class NuGetService
{
    /// <summary>Error code of a NuGet call that failed as a whole.</summary>
    public const int NuGetFailed = -32014;

    /// <summary>Default results per search.</summary>
    public const int DefaultTake = 50;

    private readonly Func<(long Generation, string? Path, CancellationToken GenerationChanged)> _currentSolution;
    private readonly Func<long> _advanceGeneration;
    private readonly TextWriter _log;
    private readonly RestoreRunner _restore;
    private readonly ConcurrentDictionary<long, long> _seq = new();
    private readonly SemaphoreSlim _sendGate = new(1, 1);
    private JsonRpc? _shell;

    /// <param name="currentSolution">The generation, the open solution (or null) and a token canceled when the generation moves on.</param>
    /// <param name="advanceGeneration">Advances the solution generation after a change and returns the new one.</param>
    public NuGetService(Func<(long Generation, string? Path, CancellationToken GenerationChanged)> currentSolution, Func<long> advanceGeneration, TextWriter log, NuGetConfigChain? chain = null, NuGetFeeds? feeds = null, RestoreRunner? restore = null)
    {
        _currentSolution = currentSolution;
        _advanceGeneration = advanceGeneration;
        _log = log;
        Chain = chain ?? new NuGetConfigChain();
        Feeds = feeds ?? new NuGetFeeds();
        _restore = restore ?? new RestoreRunner();
    }

    /// <summary>The NuGet.config chain.</summary>
    public NuGetConfigChain Chain { get; }

    /// <summary>The sources' resources and the session's cache.</summary>
    public NuGetFeeds Feeds { get; }

    /// <summary>Calls received (every <c>eludite/nuget/*</c> request).</summary>
    public int Calls => _calls;

    private int _calls;

    /// <summary>Sends notifications and credential requests on the shell connection. Call before it starts listening.</summary>
    public void Attach(JsonRpc shell) => _shell = shell;

    // ------------------------------------------------------------------ calls

    /// <summary>One call: its generation, the solution, who it is for, a token that the shell's cancel or a new generation cancels.</summary>
    private sealed class Call(NuGetService service, long generation, string solution, long? operation, NuGetCallContext context, CancellationToken token)
    {
        public long Generation { get; } = generation;

        public string Solution { get; } = solution;

        public long? Operation { get; } = operation;

        public NuGetCallContext Context { get; } = context;

        public CancellationToken Token { get; } = token;

        public NuGetLog Logger => new(service._log, line => _ = service.SendAsync(Operation, Generation, "output", text: line + "\n"));

        public Task OutputAsync(string line) => service.SendAsync(Operation, Generation, "output", text: line + "\n");

        public Task ProgressAsync(string message) => service.SendAsync(Operation, Generation, "progress", message: message);
    }

    private async Task<T> RunAsync<T>(long? requested, long? operation, bool interactive, CancellationToken cancellationToken, Func<Call, Task<T>> body)
    {
        Interlocked.Increment(ref _calls);
        if (requested is not { } generation)
        {
            throw HostErrors.BadParams("params.generation (a non-negative integer) is required");
        }

        var (current, solution, changed) = _currentSolution();
        if (generation != current)
        {
            throw HostErrors.Stale(generation, current);
        }

        if (solution is null)
        {
            throw HostErrors.BadParams("no solution is open");
        }

        // The first call installs the credential service in NuGet (process-wide).
        _ = NuGetCredentials.Instance;
        using var linked = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken, changed);
        var sources = new Lazy<IReadOnlyList<PackageSource>>(() => Chain.Enabled(SolutionDir(solution)));
        var context = new NuGetCallContext(operation, interactive, AskShellAsync, uri => sources.Value.FirstOrDefault(s => s.TrySourceAsUri is { } u && u.Authority == uri.Authority && uri.AbsoluteUri.StartsWith(u.GetLeftPart(UriPartial.Authority), StringComparison.OrdinalIgnoreCase))?.Name);
        NuGetCredentials.Current = context;
        // A fresh NuGet activity per call: NuGet keeps its authentication retries per activity, so a call refused for
        // want of credentials (an agent's) does not block the next one (the person's).
        global::NuGet.Common.ActivityCorrelationId.StartNew();
        try
        {
            return await Task.Run(() => body(new Call(this, generation, solution, operation, context, linked.Token)), CancellationToken.None).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested && changed.IsCancellationRequested)
        {
            throw HostErrors.Stale(generation, _currentSolution().Generation);
        }
        finally
        {
            NuGetCredentials.Current = null;
            _seq.TryRemove(operation ?? 0, out _);
        }
    }

    private static string SolutionDir(string solution) => Path.GetDirectoryName(Path.GetFullPath(solution))!;

    private static LocalRpcException Failed(string message, string reason, string? source = null, string? host = null, string? package = null) =>
        new(message) { ErrorCode = NuGetFailed, ErrorData = new { reason, source, host, package } };

    /// <summary>The solution's projects, or the ones named (each must be one of them).</summary>
    private static IReadOnlyList<string> Projects(string solution, IReadOnlyList<string>? named)
    {
        var all = SolutionProjects.Read(solution);
        if (named is null)
        {
            return all;
        }

        var chosen = new List<string>();
        foreach (var n in named)
        {
            var full = Path.GetFullPath(n);
            var match = all.FirstOrDefault(p => string.Equals(p, full, OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal))
                ?? throw HostErrors.BadParams($"{n} is not a project of the open solution");
            if (!chosen.Contains(match))
            {
                chosen.Add(match);
            }
        }

        return chosen;
    }

    /// <summary>The enabled sources, or the one named.</summary>
    private IReadOnlyList<PackageSource> Sources(Call call, string? named)
    {
        var enabled = Chain.Enabled(SolutionDir(call.Solution));
        if (named is null)
        {
            return enabled;
        }

        var s = enabled.FirstOrDefault(x => string.Equals(x.Name, named, StringComparison.OrdinalIgnoreCase))
            ?? throw HostErrors.BadParams($"no enabled package source is named {named}");
        return [s];
    }

    private static string Describe(Exception ex, PackageSource source, NuGetCallContext context)
    {
        if (context.CredentialsRequired is { } host && source.TrySourceAsUri is { } uri && string.Equals(uri.IsDefaultPort ? uri.Host : $"{uri.Host}:{uri.Port}", host, StringComparison.OrdinalIgnoreCase))
        {
            return $"credentials_required: {host} asked for credentials and none were given";
        }

        var inner = ex;
        while (inner.InnerException is not null && inner is global::NuGet.Protocol.Core.Types.FatalProtocolException)
        {
            inner = inner.InnerException;
        }

        return inner == ex ? ex.Message : $"{ex.Message} ({inner.Message})";
    }

    /// <summary>Runs <paramref name="work"/> on every source side by side; a source that fails gets its error row.</summary>
    private async Task<(List<(PackageSource Source, T Value)> Answers, List<NuGetSourceResult> Rows)> EachSourceAsync<T>(Call call, IReadOnlyList<PackageSource> sources, Func<PackageSource, Task<(T Value, int Count, bool Cached)>> work)
    {
        var tasks = sources.Select(async s =>
        {
            var sw = Stopwatch.StartNew();
            try
            {
                var (value, count, cached) = await work(s).ConfigureAwait(false);
                return (s, Value: value, Row: new NuGetSourceResult(s.Name, s.Source, count) { ElapsedMs = Math.Round(sw.Elapsed.TotalMilliseconds, 1), Cached = cached }, Ok: true);
            }
            catch (Exception ex) when (ex is not OperationCanceledException || !call.Token.IsCancellationRequested)
            {
                var why = Describe(ex, s, call.Context);
                await _log.WriteLineAsync($"[nuget] {s.Name}: {why}").ConfigureAwait(false);
                return (s, Value: default(T)!, Row: new NuGetSourceResult(s.Name, s.Source, 0) { ElapsedMs = Math.Round(sw.Elapsed.TotalMilliseconds, 1), Error = why }, Ok: false);
            }
        }).ToList();
        var done = await Task.WhenAll(tasks).ConfigureAwait(false);
        call.Token.ThrowIfCancellationRequested();
        return ([.. done.Where(d => d.Ok).Select(d => (d.s, d.Value))], [.. done.Select(d => d.Row)]);
    }

    /// <summary>Fails the call when every source failed (and there was at least one).</summary>
    private static void ThrowIfAllFailed(Call call, IReadOnlyList<NuGetSourceResult> rows)
    {
        if (rows.Count == 0 || rows.Any(r => r.Error is null))
        {
            return;
        }

        if (call.Context.CredentialsRequired is { } host)
        {
            throw Failed($"credentials_required: {host} asked for credentials and none were given", "credentialsRequired", rows.FirstOrDefault(r => r.Error!.StartsWith("credentials_required", StringComparison.Ordinal))?.Name, host);
        }

        throw Failed("every package source failed: " + string.Join("; ", rows.Select(r => $"{r.Name}: {r.Error}")), "sourceFailed", rows[0].Name);
    }

    // ------------------------------------------------------------------ search

    /// <summary><c>eludite/nuget/search</c>.</summary>
    public Task<NuGetSearchResult> SearchAsync(NuGetSearchParams? parameters, CancellationToken cancellationToken)
    {
        var p = parameters ?? new NuGetSearchParams((long?)null);
        return RunAsync(p.Generation, p.Operation, p.Interactive ?? false, cancellationToken, async call =>
        {
            var sw = Stopwatch.StartNew();
            var take = Math.Clamp(p.Take ?? DefaultTake, 1, 1000);
            var skip = Math.Max(p.Skip ?? 0, 0);
            var query = p.Query ?? string.Empty;
            var sources = Sources(call, p.Source);
            if (sources.Count == 0)
            {
                throw Failed("no package source is enabled (Tools > NuGet Package Manager > Package Sources)", "noSources");
            }

            var (answers, rows) = await EachSourceAsync(call, sources, async s =>
            {
                var (found, cached) = await Feeds.SearchAsync(s, query, p.Prerelease ?? false, skip, take, call.Logger, call.Token).ConfigureAwait(false);
                return (found, found.Count, cached);
            }).ConfigureAwait(false);
            ThrowIfAllFailed(call, rows);
            var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            var results = answers.SelectMany(a => a.Value).Where(r => seen.Add(r.Id)).Take(take).ToList();
            return new NuGetSearchResult(call.Generation, results, rows, Math.Round(sw.Elapsed.TotalMilliseconds, 1));
        });
    }

    // ------------------------------------------------------------------ installed

    /// <summary><c>eludite/nuget/installed</c>.</summary>
    public Task<NuGetInstalledResult> InstalledAsync(NuGetInstalledParams? parameters, CancellationToken cancellationToken)
    {
        var p = parameters ?? new NuGetInstalledParams((long?)null);
        return RunAsync(p.Generation, p.Operation, p.Interactive ?? false, cancellationToken, async call =>
        {
            var sw = Stopwatch.StartNew();
            var projects = Projects(call.Solution, p.Projects)
                .Select(path => InstalledReader.Read(path, p.IncludeTransitive ?? false))
                .ToList();
            call.Token.ThrowIfCancellationRequested();
            IReadOnlyList<NuGetSourceResult>? rows = null;
            if (p.Metadata ?? false)
            {
                (projects, rows) = await WithMetadataAsync(call, projects).ConfigureAwait(false);
            }

            return new NuGetInstalledResult(call.Generation, projects, Math.Round(sw.Elapsed.TotalMilliseconds, 1)) { Sources = rows };
        });
    }

    /// <summary>Adds the sources' vulnerability and deprecation data to every listed version, and sends it as a <c>metadata</c> update.</summary>
    private async Task<(List<NuGetInstalledProject>, IReadOnlyList<NuGetSourceResult>)> WithMetadataAsync(Call call, List<NuGetInstalledProject> projects)
    {
        var ids = projects.SelectMany(pr => pr.Packages).Where(pk => pk.Version is not null).Select(pk => pk.Id).Distinct(StringComparer.OrdinalIgnoreCase).ToList();
        var sources = Sources(call, null);
        var gate = new SemaphoreSlim(8);
        var (answers, rows) = await EachSourceAsync(call, sources, async s =>
        {
            var found = new ConcurrentDictionary<string, IReadOnlyList<VersionMetadata>>(StringComparer.OrdinalIgnoreCase);
            var cachedAll = true;
            await Task.WhenAll(ids.Select(async id =>
            {
                await gate.WaitAsync(call.Token).ConfigureAwait(false);
                try
                {
                    var (versions, cached) = await Feeds.MetadataAsync(s, id, call.Logger, call.Token).ConfigureAwait(false);
                    cachedAll &= cached;
                    found[id] = versions;
                }
                catch (Exception ex) when (ex is not OperationCanceledException && !IsSourceWide(ex))
                {
                    // Not on this source: other sources may have it.
                }
                finally
                {
                    gate.Release();
                }
            })).ConfigureAwait(false);
            return (found, found.Count, cachedAll && ids.Count > 0);
        }).ConfigureAwait(false);
        var metadata = new List<NuGetPackageMetadata>();
        NuGetInstalledPackage Annotate(NuGetInstalledPackage pk)
        {
            if (pk.Version is null || !NuGetVersion.TryParse(pk.Version, out var v))
            {
                return pk;
            }

            // Every source that lists the version adds what it knows (a folder feed knows nothing; a registration does).
            var listed = answers
                .Select(a => a.Value.TryGetValue(pk.Id, out var versions) ? versions.FirstOrDefault(m => m.Version == v) : null)
                .OfType<VersionMetadata>()
                .ToList();
            if (listed.Count == 0)
            {
                return pk;
            }

            var vulns = (pk.Vulnerabilities ?? []).Concat(listed.SelectMany(m => m.Vulnerabilities)).DistinctBy(x => x.AdvisoryUrl).ToList();
            var annotated = pk with
            {
                Vulnerabilities = vulns.Count > 0 ? vulns : null,
                Deprecation = listed.Select(m => m.Deprecation).FirstOrDefault(d => d is not null) ?? pk.Deprecation,
            };
            if (annotated.Vulnerabilities is not null || annotated.Deprecation is not null)
            {
                metadata.Add(new NuGetPackageMetadata(pk.Id, pk.Version) { Vulnerabilities = annotated.Vulnerabilities, Deprecation = annotated.Deprecation });
            }

            return annotated;
        }

        var result = projects.Select(pr => pr with { Packages = [.. pr.Packages.Select(Annotate)] }).ToList();
        if (metadata.Count > 0)
        {
            await SendAsync(call.Operation, call.Generation, "metadata", packages: [.. metadata.DistinctBy(m => (m.Id.ToLowerInvariant(), m.Version))]).ConfigureAwait(false);
        }

        return (result, rows);
    }

    /// <summary>A failure of the source as a whole (unreachable, unauthorized), not of one package.</summary>
    private static bool IsSourceWide(Exception ex) =>
        ex is global::NuGet.Protocol.Core.Types.FatalProtocolException { InnerException: HttpRequestException } || ex is HttpRequestException;

    // ------------------------------------------------------------------ updates

    /// <summary><c>eludite/nuget/updates</c>.</summary>
    public Task<NuGetUpdatesResult> UpdatesAsync(NuGetUpdatesParams? parameters, CancellationToken cancellationToken)
    {
        var p = parameters ?? new NuGetUpdatesParams((long?)null);
        return RunAsync(p.Generation, p.Operation, p.Interactive ?? false, cancellationToken, async call =>
        {
            var sw = Stopwatch.StartNew();
            var prerelease = p.Prerelease ?? false;
            var projects = Projects(call.Solution, p.Projects).Select(path => InstalledReader.Read(path, false)).Where(pr => pr.Format == "packageReference").ToList();
            var ids = projects.SelectMany(pr => pr.Packages).Where(pk => !pk.AutoReferenced).Select(pk => pk.Id).Distinct(StringComparer.OrdinalIgnoreCase).ToList();
            var (latest, rows) = await LatestAsync(call, Sources(call, p.Source), ids, prerelease).ConfigureAwait(false);
            ThrowIfAllFailed(call, rows);
            var updates = new List<NuGetUpdateRow>();
            foreach (var pr in projects)
            {
                foreach (var pk in pr.Packages.Where(x => !x.AutoReferenced))
                {
                    if (!latest.TryGetValue(pk.Id, out var best) || InstalledVersion(pk) is not { } installed || best.Version <= installed)
                    {
                        continue;
                    }

                    updates.Add(new NuGetUpdateRow(pr.Path, pk.Id, installed.ToNormalizedString(), best.Version.ToNormalizedString(), best.Source)
                    {
                        Requested = pk.Requested,
                        Versions = best.All,
                        Vulnerabilities = pk.Vulnerabilities,
                        Deprecation = pk.Deprecation,
                    });
                }
            }

            return new NuGetUpdatesResult(call.Generation, updates, rows, Math.Round(sw.Elapsed.TotalMilliseconds, 1));
        });
    }

    /// <summary>The installed version: the resolved one, else the requested range's lower bound.</summary>
    private static NuGetVersion? InstalledVersion(NuGetInstalledPackage pk)
    {
        if (pk.Version is not null && NuGetVersion.TryParse(pk.Version, out var v))
        {
            return v;
        }

        return pk.Requested is not null && VersionRange.TryParse(pk.Requested, out var range) ? range.MinVersion : null;
    }

    /// <summary>For each id, the newest version any source lists (prerelease per <paramref name="prerelease"/>), the source that lists it and every version.</summary>
    private async Task<(Dictionary<string, (NuGetVersion Version, string Source, IReadOnlyList<string> All)> Latest, IReadOnlyList<NuGetSourceResult> Rows)> LatestAsync(Call call, IReadOnlyList<PackageSource> sources, IReadOnlyList<string> ids, bool prerelease)
    {
        var gate = new SemaphoreSlim(8);
        var (answers, rows) = await EachSourceAsync(call, sources, async s =>
        {
            var found = new ConcurrentDictionary<string, IReadOnlyList<NuGetVersion>>(StringComparer.OrdinalIgnoreCase);
            var cachedAll = true;
            Exception? sourceWide = null;
            await Task.WhenAll(ids.Select(async id =>
            {
                await gate.WaitAsync(call.Token).ConfigureAwait(false);
                try
                {
                    var (versions, cached) = await Feeds.VersionsAsync(s, id, call.Logger, call.Token).ConfigureAwait(false);
                    cachedAll &= cached;
                    if (versions.Count > 0)
                    {
                        found[id] = versions;
                    }
                }
                catch (Exception ex) when (ex is not OperationCanceledException)
                {
                    if (IsSourceWide(ex))
                    {
                        sourceWide = ex;
                    }
                }
                finally
                {
                    gate.Release();
                }
            })).ConfigureAwait(false);
            if (sourceWide is not null && found.IsEmpty)
            {
                throw sourceWide;
            }

            return (found, found.Count, cachedAll && ids.Count > 0);
        }).ConfigureAwait(false);
        var latest = new Dictionary<string, (NuGetVersion, string, IReadOnlyList<string>)>(StringComparer.OrdinalIgnoreCase);
        foreach (var id in ids)
        {
            var all = answers.SelectMany(a => a.Value.TryGetValue(id, out var vs) ? vs.Select(v => (v, a.Source.Name)) : [])
                .Where(x => prerelease || !x.v.IsPrerelease)
                .OrderByDescending(x => x.v)
                .ToList();
            if (all.Count > 0)
            {
                latest[id] = (all[0].v, all[0].Name, [.. all.Select(x => x.v.ToNormalizedString()).Distinct()]);
            }
        }

        return (latest, rows);
    }

    // ------------------------------------------------------------------ change

    /// <summary><c>eludite/nuget/change</c>.</summary>
    public Task<NuGetChangeResult> ChangeAsync(NuGetChangeParams? parameters, CancellationToken cancellationToken)
    {
        var p = parameters ?? new NuGetChangeParams(null, null, null);
        return RunAsync(p.Generation, p.Operation, p.Interactive ?? false, cancellationToken, async call =>
        {
            var sw = Stopwatch.StartNew();
            var action = p.Action ?? throw HostErrors.BadParams("params.action is required");
            if (action is not ("install" or "uninstall" or "update" or "consolidate"))
            {
                throw HostErrors.BadParams($"unknown action {action}");
            }

            if (p.Packages is not { Count: > 0 } packages || packages.Any(x => string.IsNullOrWhiteSpace(x.Id)))
            {
                throw HostErrors.BadParams("params.packages lists at least one package id");
            }

            var all = Projects(call.Solution, null);
            var installed = all.ToDictionary(path => path, path => InstalledReader.Read(path, includeTransitive: true));
            IReadOnlyList<string> Referencing(string id, bool transitiveToo) =>
                [.. installed.Where(kv => kv.Value.Format == "packageReference" && kv.Value.Packages.Any(pk => string.Equals(pk.Id, id, StringComparison.OrdinalIgnoreCase) && (transitiveToo || !pk.Transitive) && !pk.AutoReferenced)).Select(kv => kv.Key)];

            IReadOnlyList<string>? named = p.Projects is null ? null : Projects(call.Solution, p.Projects);
            if (action == "install" && named is null)
            {
                named = all.Count == 1 ? all : throw HostErrors.BadParams("install names the projects (params.projects) when the solution has more than one");
            }

            foreach (var n in named ?? [])
            {
                if (installed[n].Format == "packagesConfig")
                {
                    throw Failed($"{Path.GetFileName(n)} uses packages.config, which is listed read-only: {InstalledReader.PackagesConfigNote}", "invalidProject");
                }
            }

            // Resolve each package's version.
            var resolved = new List<NuGetPackageArg>();
            var plan = new Dictionary<string, List<PackageEdit>>(StringComparer.Ordinal);
            var needLatest = action is "install" or "update" ? packages.Where(x => x.Version is null).Select(x => x.Id).ToList() : [];
            var needCheck = action is "install" or "update" ? packages.Where(x => x.Version is not null).Select(x => x.Id).ToList() : [];
            Dictionary<string, (NuGetVersion Version, string Source, IReadOnlyList<string> All)> latest = new(StringComparer.OrdinalIgnoreCase);
            var sourceList = needLatest.Count + needCheck.Count > 0 ? Sources(call, p.Source) : [];
            if (needLatest.Count + needCheck.Count > 0)
            {
                await call.ProgressAsync("Resolving package versions").ConfigureAwait(false);
                // Every version (prerelease too), so a named prerelease version is found; the default follows `prerelease`.
                (latest, var rows) = await LatestAsync(call, sourceList, [.. needLatest.Concat(needCheck)], prerelease: true).ConfigureAwait(false);
                ThrowIfAllFailed(call, rows);
            }

            foreach (var pkg in packages)
            {
                var id = pkg.Id.Trim();
                var targets = action switch
                {
                    "install" => named!,
                    _ => named ?? Referencing(id, (p.IncludeTransitive ?? false) && action != "uninstall"),
                };
                string? version = null;
                switch (action)
                {
                    case "install" or "update":
                        if (!latest.TryGetValue(id, out var best))
                        {
                            throw Failed($"Unable to find package {id} on the package sources", "notFound", package: id);
                        }

                        if (pkg.Version is { } wanted)
                        {
                            if (!NuGetVersion.TryParse(wanted, out var w) || !best.All.Contains(w.ToNormalizedString(), StringComparer.OrdinalIgnoreCase))
                            {
                                throw Failed($"Unable to find package {id} {wanted} on the package sources", "notFound", package: id);
                            }

                            version = w.ToNormalizedString();
                        }
                        else
                        {
                            version = best.All.Select(NuGetVersion.Parse).FirstOrDefault(v => (p.Prerelease ?? false) || !v.IsPrerelease)?.ToNormalizedString()
                                ?? throw Failed($"{id} has only prerelease versions; include prerelease to install one", "notFound", package: id);
                        }

                        var preferred = PreferredId(installed.Values, id);
                        id = preferred != id || installed.Values.Any(pr => pr.Packages.Any(pk => pk.Id == id))
                            ? preferred
                            : await Feeds.CanonicalIdAsync(sourceList.OrderBy(s => s.Name == best.Source ? 0 : 1), id, NuGetVersion.Parse(version), call.Logger, call.Token).ConfigureAwait(false) ?? id;
                        break;
                    case "consolidate":
                        var versions = targets.SelectMany(t => installed[t].Packages.Where(pk => string.Equals(pk.Id, id, StringComparison.OrdinalIgnoreCase)))
                            .Select(InstalledVersion).OfType<NuGetVersion>().ToList();
                        version = pkg.Version ?? versions.DefaultIfEmpty().Max()?.ToNormalizedString()
                            ?? throw Failed($"{id} is not installed in the solution", "notFound", package: id);
                        id = PreferredId(installed.Values, id);
                        break;
                }

                resolved.Add(new NuGetPackageArg(id) { Version = version });
                foreach (var t in targets)
                {
                    if (!plan.TryGetValue(t, out var edits))
                    {
                        plan[t] = edits = [];
                    }

                    edits.Add(new PackageEdit(id, version, action == "uninstall"));
                }
            }

            var verb = action switch { "install" => "Installing", "uninstall" => "Uninstalling", "update" => "Updating", _ => "Consolidating" };
            var edited = new List<NuGetEditedFile>();
            var changedProjects = new List<string>();
            foreach (var (project, edits) in plan)
            {
                call.Token.ThrowIfCancellationRequested();
                var name = Path.GetFileNameWithoutExtension(project);
                foreach (var e in edits)
                {
                    await call.OutputAsync(e.Remove ? $"{verb} NuGet package {e.Id} from {name}." : $"{verb} NuGet package {e.Id} {e.Version} in {name}.").ConfigureAwait(false);
                }

                ProjectEditResult result;
                try
                {
                    result = ProjectEditor.Apply(project, edits);
                }
                catch (Exception ex) when (ex is InvalidOperationException or IOException or UnauthorizedAccessException or System.Xml.XmlException)
                {
                    throw Failed($"{Path.GetFileName(project)} could not be edited: {ex.Message}", "invalidProject");
                }

                foreach (var f in result.Edited)
                {
                    foreach (var line in f.Changes)
                    {
                        await call.OutputAsync($"  {Path.GetFileName(f.Path)}: {line}").ConfigureAwait(false);
                    }

                    var existing = edited.FindIndex(x => x.Path == f.Path);
                    if (existing >= 0)
                    {
                        edited[existing] = edited[existing] with { Changes = [.. edited[existing].Changes, .. f.Changes] };
                    }
                    else
                    {
                        edited.Add(f);
                    }
                }

                if (result.Edited.Count > 0)
                {
                    changedProjects.Add(project);
                }
            }

            if (edited.Count == 0)
            {
                var why = action == "uninstall" ? "the package is not referenced by those projects" : "the projects already reference that version";
                await call.OutputAsync($"No change: {why}.").ConfigureAwait(false);
                await call.OutputAsync("========== Finished ==========").ConfigureAwait(false);
                return new NuGetChangeResult(call.Generation, action, resolved, [], [], Math.Round(sw.Elapsed.TotalMilliseconds, 1)) { Message = $"Nothing changed: {why}." };
            }

            NuGetRestoreOutcome? restore = null;
            if (p.Restore ?? true)
            {
                restore = await RestoreCoreAsync(call, call.Solution, all, p.LockFiles, changed: true, force: false).ConfigureAwait(false);
            }

            foreach (var pkg in resolved)
            {
                var name = string.Join(", ", changedProjects.Select(Path.GetFileNameWithoutExtension));
                await call.OutputAsync(action == "uninstall"
                    ? $"Successfully uninstalled '{pkg.Id}' from {name}"
                    : $"Successfully installed '{pkg.Id} {pkg.Version}' to {name}").ConfigureAwait(false);
            }

            await call.OutputAsync($"Time Elapsed: {sw.Elapsed:hh\\:mm\\:ss\\.fff}").ConfigureAwait(false);
            await call.OutputAsync("========== Finished ==========").ConfigureAwait(false);
            // The projects changed: what was computed under this generation (the tree, IntelliSense) is stale.
            var generation = _advanceGeneration();
            return new NuGetChangeResult(generation, action, resolved, changedProjects, edited, Math.Round(sw.Elapsed.TotalMilliseconds, 1)) { Restore = restore };
        });
    }

    /// <summary>The id as a project already writes it, else as asked.</summary>
    private static string PreferredId(IEnumerable<NuGetInstalledProject> projects, string id) =>
        projects.SelectMany(pr => pr.Packages).FirstOrDefault(pk => string.Equals(pk.Id, id, StringComparison.OrdinalIgnoreCase))?.Id ?? id;

    // ------------------------------------------------------------------ sources

    /// <summary><c>eludite/nuget/sources</c>.</summary>
    public Task<NuGetSourcesResult> SourcesAsync(NuGetSourcesParams? parameters, CancellationToken cancellationToken)
    {
        var p = parameters ?? new NuGetSourcesParams((long?)null);
        return RunAsync(p.Generation, p.Operation, false, cancellationToken, call =>
        {
            var root = SolutionDir(call.Solution);
            var action = p.Action ?? "list";
            string Name() => string.IsNullOrWhiteSpace(p.Name) ? throw HostErrors.BadParams($"{action} names the source (params.name)") : p.Name.Trim();
            try
            {
                switch (action)
                {
                    case "list":
                        return Task.FromResult(Chain.List(root));
                    case "add":
                        if (string.IsNullOrWhiteSpace(p.Url))
                        {
                            throw HostErrors.BadParams("add gives the source's address (params.url)");
                        }

                        Chain.Add(Name(), p.Url.Trim());
                        break;
                    case "remove":
                        Chain.Remove(root, Name());
                        break;
                    case "enable":
                    case "disable":
                        Chain.SetEnabled(root, Name(), action == "enable");
                        break;
                    default:
                        throw HostErrors.BadParams($"unknown action {action}");
                }
            }
            catch (InvalidOperationException ex)
            {
                throw HostErrors.BadParams(ex.Message);
            }

            _log.WriteLine($"[nuget] sources: {action} {p.Name} in {Chain.UserConfig}");
            return Task.FromResult(Chain.List(root, changed: true));
        });
    }

    // ------------------------------------------------------------------ restore

    /// <summary><c>eludite/nuget/restore</c>.</summary>
    public Task<NuGetRestoreResult> RestoreAsync(NuGetRestoreParams? parameters, CancellationToken cancellationToken)
    {
        var p = parameters ?? new NuGetRestoreParams((long?)null);
        return RunAsync(p.Generation, p.Operation, p.Interactive ?? false, cancellationToken, async call =>
        {
            var projects = Projects(call.Solution, p.Projects);
            var target = p.Projects is { Count: 1 } ? projects[0] : call.Solution;
            var outcome = await RestoreCoreAsync(call, target, projects, p.LockFiles, changed: false, force: p.Force ?? false).ConfigureAwait(false);
            await call.OutputAsync("========== Finished ==========").ConfigureAwait(false);
            return NuGetRestoreResult.From(call.Generation, outcome);
        });
    }

    private async Task<NuGetRestoreOutcome> RestoreCoreAsync(Call call, string target, IReadOnlyList<string> projects, string? lockFiles, bool changed, bool force)
    {
        await call.ProgressAsync($"Restoring {Path.GetFileName(target)}").ConfigureAwait(false);
        var env = NuGetCredentials.Instance.RestoreEnvironment(Chain.Enabled(SolutionDir(call.Solution)));
        var request = new RestoreRequest(target, projects, lockFiles ?? "respect", changed, force, env);
        var outcome = await _restore.RunAsync(request, call.OutputAsync, call.Token).ConfigureAwait(false);
        var errors = outcome.Diagnostics.Count(d => d.Severity == "error");
        await call.OutputAsync(outcome.Result switch
        {
            "succeeded" => $"Restore succeeded in {outcome.ElapsedMs / 1000:0.0} s.",
            "canceled" => "Restore canceled.",
            _ => $"Restore failed with {errors} error{(errors == 1 ? string.Empty : "s")}.",
        }).ConfigureAwait(false);
        call.Token.ThrowIfCancellationRequested();
        return outcome;
    }

    // ------------------------------------------------------------------ the shell

    private async Task SendAsync(long? operation, long generation, string kind, string? text = null, string? message = null, IReadOnlyList<NuGetPackageMetadata>? packages = null)
    {
        if (operation is not { } op || _shell is not { } shell)
        {
            return;
        }

        await _sendGate.WaitAsync(CancellationToken.None).ConfigureAwait(false);
        try
        {
            var seq = _seq.AddOrUpdate(op, 0, (_, s) => s + 1);
            await shell.NotifyWithParameterObjectAsync("eludite/nuget/update", new NuGetUpdate(op, generation, seq, kind) { Text = text, Message = message, Packages = packages }).ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is ConnectionLostException or ObjectDisposedException)
        {
            // The shell went away; nothing to tell.
        }
        finally
        {
            _sendGate.Release();
        }
    }

    private async Task<NuGetCredentialsAnswer?> AskShellAsync(NuGetCredentialsParams parameters, CancellationToken cancellationToken)
    {
        if (_shell is not { } shell)
        {
            return null;
        }

        try
        {
            return await shell.InvokeWithParameterObjectAsync<NuGetCredentialsAnswer?>("eludite/nuget/credentials", parameters, cancellationToken).ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is RemoteInvocationException or ConnectionLostException or ObjectDisposedException)
        {
            await _log.WriteLineAsync($"[nuget] eludite/nuget/credentials: {ex.Message}").ConfigureAwait(false);
            return null;
        }
    }
}
