using System.Collections.Concurrent;
using System.Net;
using NuGet.Configuration;
using NuGet.Credentials;
using NuGet.Protocol;
using NuGet.Protocol.Plugins;

namespace Eludite.Host.NuGet;

/// <summary>Who a NuGet call is for, flowing with it (an <see cref="AsyncLocal{T}"/>) into the credential service.</summary>
public sealed class NuGetCallContext
{
    /// <param name="interactive">The shell may be asked (the person's call).</param>
    /// <param name="ask">Asks the shell (<c>eludite/nuget/credentials</c>); null when no shell is attached.</param>
    public NuGetCallContext(long? operation, bool interactive, Func<NuGetCredentialsParams, CancellationToken, Task<NuGetCredentialsAnswer?>>? ask, Func<Uri, string?> sourceName)
    {
        Operation = operation;
        Interactive = interactive;
        Ask = ask;
        SourceName = sourceName;
    }

    public long? Operation { get; }

    public bool Interactive { get; }

    public Func<NuGetCredentialsParams, CancellationToken, Task<NuGetCredentialsAnswer?>>? Ask { get; }

    /// <summary>The configured source's name for a source address.</summary>
    public Func<Uri, string?> SourceName { get; }

    /// <summary>The host whose credentials were required and not given during this call, if any.</summary>
    public string? CredentialsRequired { get; set; }

    /// <summary>Hosts this call already answered (a second request for one is a retry: the answer was refused).</summary>
    internal ConcurrentDictionary<string, bool> Answered { get; } = new(StringComparer.OrdinalIgnoreCase);
}

/// <summary>
/// The host's NuGet credential service (host-rpc.md, "NuGet", Credentials), installed once per process as
/// <see cref="HttpHandlerResourceV3.CredentialService"/>: the answers kept for this session first, then NuGet's plugin
/// credential providers (found as NuGet finds them: the plugin folders and <c>NUGET_PLUGIN_PATHS</c>; the plugin
/// protocol over stdio), then, for an interactive call, the shell's prompt. Nothing is written to disk.
/// </summary>
public sealed class NuGetCredentials : ICredentialService
{
    private static readonly AsyncLocal<NuGetCallContext?> CurrentCall = new();
    private static readonly Lazy<NuGetCredentials> Shared = new(() =>
    {
        var service = new NuGetCredentials();
        HttpHandlerResourceV3.CredentialService = new Lazy<ICredentialService>(() => service);
        return service;
    });

    private readonly ConcurrentDictionary<string, NetworkCredential> _session = new(StringComparer.OrdinalIgnoreCase);
    private readonly Lazy<Task<IReadOnlyList<ICredentialProvider>>> _plugins;

    private NuGetCredentials()
    {
        _plugins = new Lazy<Task<IReadOnlyList<ICredentialProvider>>>(BuildPluginsAsync);
    }

    /// <summary>The process's service, installed in NuGet on first use.</summary>
    public static NuGetCredentials Instance => Shared.Value;

    /// <summary>The call the current async flow runs for.</summary>
    public static NuGetCallContext? Current
    {
        get => CurrentCall.Value;
        set => CurrentCall.Value = value;
    }

    /// <summary>Plugin providers to use instead of discovering them (tests).</summary>
    public IReadOnlyList<ICredentialProvider>? ProvidersOverride { get; set; }

    public bool HandlesDefaultCredentials => false;

    /// <summary>Forget every answer (tests).</summary>
    public void Clear() => _session.Clear();

    /// <summary>The session's credentials as NuGet's <c>NuGetPackageSourceCredentials_&lt;source&gt;</c> variables, for restores.</summary>
    public IReadOnlyDictionary<string, string> RestoreEnvironment(IEnumerable<PackageSource> sources)
    {
        ArgumentNullException.ThrowIfNull(sources);
        var env = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var s in sources)
        {
            if (s.TrySourceAsUri is { } uri && _session.TryGetValue(Key(uri), out var c))
            {
                var name = new string([.. s.Name.Select(ch => char.IsAsciiLetterOrDigit(ch) || ch == '_' ? ch : '_')]);
                env[$"NuGetPackageSourceCredentials_{name}"] = $"Username={c.UserName};Password={c.Password}";
            }
        }

        return env;
    }

    public bool TryGetLastKnownGoodCredentialsFromCache(Uri uri, bool isProxy, out ICredentials credentials)
    {
        ArgumentNullException.ThrowIfNull(uri);
        if (_session.TryGetValue(Key(uri), out var c))
        {
            credentials = c;
            return true;
        }

        credentials = null!;
        return false;
    }

    public async Task<ICredentials?> GetCredentialsAsync(Uri uri, IWebProxy? proxy, CredentialRequestType type, string message, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(uri);
        var key = Key(uri);
        var call = Current;
        var isRetry = call?.Answered.ContainsKey(key) ?? false;
        if (!isRetry && _session.TryGetValue(key, out var kept))
        {
            call?.Answered.TryAdd(key, true);
            return kept;
        }

        _session.TryRemove(key, out _);
        foreach (var provider in ProvidersOverride ?? await _plugins.Value.ConfigureAwait(false))
        {
            CredentialResponse response;
            try
            {
                response = await provider.GetAsync(uri, proxy!, type, message, isRetry, nonInteractive: true, cancellationToken).ConfigureAwait(false);
            }
            catch (Exception ex) when (ex is not OperationCanceledException)
            {
                await Console.Error.WriteLineAsync($"[nuget] credential provider {provider.Id} failed: {ex.Message}").ConfigureAwait(false);
                continue;
            }

            if (response.Status == CredentialStatus.Success && response.Credentials is { } found)
            {
                call?.Answered.TryAdd(key, true);
                if (found.GetCredential(uri, "Basic") is { } basic)
                {
                    _session[key] = basic;
                }

                return found;
            }
        }

        var host = uri.IsDefaultPort ? uri.Host : $"{uri.Host}:{uri.Port}";
        if (call is not { Interactive: true, Ask: { } ask })
        {
            if (call is not null)
            {
                call.CredentialsRequired = host;
            }

            return null;
        }

        var answer = await ask(new NuGetCredentialsParams(call.SourceName(uri) ?? host, uri.ToString(), host, type == CredentialRequestType.Proxy, isRetry)
        {
            Operation = call.Operation,
            Message = string.IsNullOrEmpty(message) ? null : message,
        }, cancellationToken).ConfigureAwait(false);
        if (answer is null || answer.Canceled == true || answer.Username is null)
        {
            call.CredentialsRequired = host;
            return null;
        }

        var credential = new NetworkCredential(answer.Username, answer.Password ?? string.Empty);
        call.Answered[key] = true;
        // Kept for the session: the next call, and restores, use it without asking (nothing is written).
        _session[key] = credential;
        return credential;
    }

    /// <summary>The cache key of a source address: its scheme, host and port.</summary>
    private static string Key(Uri uri) => uri.GetLeftPart(UriPartial.Authority);

    private static async Task<IReadOnlyList<ICredentialProvider>> BuildPluginsAsync()
    {
        try
        {
            var builder = new SecurePluginCredentialProviderBuilder(PluginManager.Instance, canShowDialog: false, new NuGetLog(Console.Error));
            return [.. await builder.BuildAllAsync().ConfigureAwait(false)];
        }
        catch (Exception ex) when (ex is not OutOfMemoryException)
        {
            await Console.Error.WriteLineAsync($"[nuget] credential providers unavailable: {ex.Message}").ConfigureAwait(false);
            return [];
        }
    }
}
