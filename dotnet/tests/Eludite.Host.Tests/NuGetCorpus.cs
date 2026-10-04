using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using NuGet.Frameworks;
using NuGet.Packaging;
using NuGet.Packaging.Core;
using NuGet.Versioning;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0048's NuGet corpus (<c>corpus/nuget</c>: App with Greeter 1.0.0 and a project reference to Shared, Lib with
/// Greeter 1.1.0, the local-feed NuGet.config) copied to a temporary folder, with its feed built there by
/// <see cref="PackageBuilder"/>: Eludite.Corpus.Logging 1.0.0 and Eludite.Corpus.Greeter 1.0.0, 1.1.0 and 2.0.0-beta.1, each
/// depending on Logging. No test touches the network or the machine's NuGet folders.
/// </summary>
internal static class NuGetCorpus
{
    public const string Greeter = "Eludite.Corpus.Greeter";
    public const string Logging = "Eludite.Corpus.Logging";

    /// <summary>The packages of the feed: (id, version, depends on Logging).</summary>
    public static readonly (string Id, string Version, bool UsesLogging)[] Packages =
    [
        (Logging, "1.0.0", false),
        (Greeter, "1.0.0", true),
        (Greeter, "1.1.0", true),
        (Greeter, "2.0.0-beta.1", true),
    ];

    /// <summary>The repository's <c>corpus/nuget</c>, found above the test assembly.</summary>
    public static string Source()
    {
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, "corpus", "nuget");
            if (File.Exists(Path.Combine(candidate, "NuGet.config")))
            {
                return candidate;
            }
        }

        throw new InvalidOperationException("corpus/nuget not found above " + AppContext.BaseDirectory);
    }

    /// <summary>A fresh copy of the corpus with its feed built; delete it with <see cref="TestDirectory.Delete"/>.</summary>
    public static string Create()
    {
        var root = Path.Combine(Path.GetTempPath(), "eludite-nuget-" + Guid.NewGuid().ToString("N")[..12]);
        Copy(Source(), root);
        BuildFeed(Path.Combine(root, "feed"));
        return root;
    }

    public static string Project(string root, string name) => Path.Combine(root, name, name + ".csproj");

    /// <summary>Writes the feed's packages into <paramref name="feed"/>.</summary>
    public static void BuildFeed(string feed)
    {
        Directory.CreateDirectory(feed);
        var payload = Path.Combine(feed, "_payload");
        File.WriteAllText(payload, string.Empty);
        foreach (var (id, version, usesLogging) in Packages)
        {
            var builder = new PackageBuilder
            {
                Id = id,
                Version = NuGetVersion.Parse(version),
                Description = id == Greeter ? "Greets people, for Eludite's NuGet tests." : "A one-method logging package for Eludite's NuGet tests.",
            };
            builder.Authors.Add("The Eludite Authors");
            builder.DependencyGroups.Add(new PackageDependencyGroup(
                NuGetFramework.Parse("net10.0"),
                usesLogging ? [new PackageDependency(Logging, VersionRange.Parse("1.0.0"))] : []));
            builder.Files.Add(new PhysicalPackageFile { SourcePath = payload, TargetPath = "lib/net10.0/_._" });
            using var stream = File.Create(Path.Combine(feed, $"{id}.{version}.nupkg"));
            builder.Save(stream);
        }

        File.Delete(payload);
    }

    private static void Copy(string from, string to)
    {
        Directory.CreateDirectory(to);
        foreach (var file in Directory.EnumerateFiles(from))
        {
            File.Copy(file, Path.Combine(to, Path.GetFileName(file)));
        }

        foreach (var dir in Directory.EnumerateDirectories(from))
        {
            var name = Path.GetFileName(dir);
            if (name is "feed" or ".packages" or "bin" or "obj" or "packages")
            {
                continue;
            }

            Copy(dir, Path.Combine(to, name));
        }
    }
}

/// <summary>
/// A NuGet V3 feed on loopback (service index, search, flat container versions and inline registration pages), with
/// optional basic authentication, a delay on searches, and registration data carrying vulnerabilities and deprecation.
/// </summary>
internal sealed class FakeV3Feed : IAsyncDisposable
{
    private readonly HttpListener _listener = new();
    private readonly Task _loop;
    private readonly CancellationTokenSource _stop = new();
    private readonly Dictionary<string, string[]> _packages;
    private int _requests;
    private int _unauthorized;
    private int _icons;

    /// <summary>Icon downloads served.</summary>
    public int Icons => _icons;

    public FakeV3Feed(Dictionary<string, string[]> packages, (string User, string Password)? basic = null)
    {
        _packages = new Dictionary<string, string[]>(packages, StringComparer.OrdinalIgnoreCase);
        Basic = basic;
        var probe = new TcpListener(IPAddress.Loopback, 0);
        probe.Start();
        var port = ((IPEndPoint)probe.LocalEndpoint).Port;
        probe.Stop();
        Base = $"http://127.0.0.1:{port}";
        _listener.Prefixes.Add(Base + "/");
        _listener.Start();
        _loop = Task.Run(LoopAsync);
    }

    public string Base { get; }

    public string Url => Base + "/v3/index.json";

    public string Authority => new Uri(Base).Authority;

    public (string User, string Password)? Basic { get; }

    /// <summary>How long each search waits before answering.</summary>
    public TimeSpan QueryDelay { get; set; }

    /// <summary>Registration data per <c>id/version</c> (lowercase id): (vulnerability severity 0 to 3 and advisory, deprecation message).</summary>
    public Dictionary<string, (int Severity, string Advisory)> Vulnerable { get; } = new(StringComparer.OrdinalIgnoreCase);

    public Dictionary<string, string> Deprecated { get; } = new(StringComparer.OrdinalIgnoreCase);

    public int Requests => _requests;

    public int Unauthorized => _unauthorized;

    private async Task LoopAsync()
    {
        while (!_stop.IsCancellationRequested)
        {
            HttpListenerContext context;
            try
            {
                context = await _listener.GetContextAsync();
            }
            catch (Exception) when (_stop.IsCancellationRequested)
            {
                return;
            }
            catch (HttpListenerException)
            {
                return;
            }

            _ = Task.Run(() => ServeAsync(context));
        }
    }

    private async Task ServeAsync(HttpListenerContext context)
    {
        Interlocked.Increment(ref _requests);
        var response = context.Response;
        try
        {
            if (Basic is { } b)
            {
                var expected = "Basic " + Convert.ToBase64String(Encoding.UTF8.GetBytes($"{b.User}:{b.Password}"));
                if (context.Request.Headers["Authorization"] != expected)
                {
                    Interlocked.Increment(ref _unauthorized);
                    response.StatusCode = 401;
                    response.AddHeader("WWW-Authenticate", "Basic realm=\"eludite-test\"");
                    response.Close();
                    return;
                }
            }

            var path = context.Request.Url!.AbsolutePath;
            if (path == "/icon.png")
            {
                Interlocked.Increment(ref _icons);
                byte[] png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
                response.ContentType = "image/png";
                response.ContentLength64 = png.Length;
                await response.OutputStream.WriteAsync(png);
                response.Close();
                return;
            }

            object? body = null;
            if (path == "/v3/index.json")
            {
                body = new
                {
                    version = "3.0.0",
                    resources = new object[]
                    {
                        new Dictionary<string, string> { ["@id"] = Base + "/v3/query", ["@type"] = "SearchQueryService/3.4.0" },
                        new Dictionary<string, string> { ["@id"] = Base + "/v3/registration/", ["@type"] = "RegistrationsBaseUrl/3.6.0" },
                        new Dictionary<string, string> { ["@id"] = Base + "/v3/flat/", ["@type"] = "PackageBaseAddress/3.0.0" },
                    },
                };
            }
            else if (path == "/v3/query")
            {
                await Task.Delay(QueryDelay, _stop.Token);
                var q = context.Request.QueryString["q"] ?? string.Empty;
                var prerelease = string.Equals(context.Request.QueryString["prerelease"], "true", StringComparison.OrdinalIgnoreCase);
                var data = _packages.Where(p => p.Key.Contains(q, StringComparison.OrdinalIgnoreCase))
                    .Select(p =>
                    {
                        var versions = p.Value.Where(v => prerelease || !NuGetVersion.Parse(v).IsPrerelease).ToList();
                        return new Dictionary<string, object>
                        {
                            ["@id"] = $"{Base}/v3/registration/{p.Key.ToLowerInvariant()}/index.json",
                            ["@type"] = "Package",
                            ["id"] = p.Key,
                            ["version"] = versions.Last(),
                            ["description"] = $"{p.Key} from the fake feed",
                            ["authors"] = new[] { "Fake" },
                            ["totalDownloads"] = 42,
                            ["versions"] = versions.Select(v => new Dictionary<string, object> { ["version"] = v, ["downloads"] = 1, ["@id"] = $"{Base}/v3/registration/{p.Key.ToLowerInvariant()}/{v}.json" }).ToList(),
                        };
                    }).ToList();
                body = new { totalHits = data.Count, data };
            }
            else if (path.StartsWith("/v3/flat/", StringComparison.Ordinal) && path.EndsWith("/index.json", StringComparison.Ordinal))
            {
                var id = path["/v3/flat/".Length..^"/index.json".Length];
                if (_packages.TryGetValue(id, out var versions))
                {
                    body = new { versions = versions.Select(v => v.ToLowerInvariant()).ToList() };
                }
            }
            else if (path.StartsWith("/v3/registration/", StringComparison.Ordinal) && path.EndsWith("/index.json", StringComparison.Ordinal))
            {
                var id = path["/v3/registration/".Length..^"/index.json".Length];
                if (_packages.FirstOrDefault(p => string.Equals(p.Key, id, StringComparison.OrdinalIgnoreCase)) is { Key: not null } pkg)
                {
                    body = Registration(pkg.Key, pkg.Value);
                }
            }

            if (body is null)
            {
                response.StatusCode = 404;
                response.Close();
                return;
            }

            var bytes = JsonSerializer.SerializeToUtf8Bytes(body);
            response.ContentType = "application/json";
            response.ContentLength64 = bytes.Length;
            await response.OutputStream.WriteAsync(bytes);
            response.Close();
        }
        catch (Exception ex) when (ex is HttpListenerException or ObjectDisposedException or OperationCanceledException or IOException)
        {
            // The client went away (a canceled request).
        }
    }

    private object Registration(string id, string[] versions)
    {
        var lower = id.ToLowerInvariant();
        var leaves = versions.Select(v =>
        {
            var entry = new Dictionary<string, object>
            {
                ["@id"] = $"{Base}/v3/catalog/{lower}.{v}.json",
                ["id"] = id,
                ["version"] = v,
                ["listed"] = true,
                ["published"] = "2026-01-01T00:00:00+00:00",
                ["description"] = $"{id} from the fake feed",
                ["authors"] = "Fake",
                ["dependencyGroups"] = Array.Empty<object>(),
            };
            if (Vulnerable.TryGetValue($"{lower}/{v}", out var vuln))
            {
                entry["vulnerabilities"] = new[] { new Dictionary<string, object> { ["advisoryUrl"] = vuln.Advisory, ["severity"] = vuln.Severity.ToString(System.Globalization.CultureInfo.InvariantCulture) } };
            }

            if (Deprecated.TryGetValue($"{lower}/{v}", out var message))
            {
                entry["deprecation"] = new Dictionary<string, object> { ["@id"] = $"{Base}/v3/catalog/{lower}.{v}.json#deprecation", ["reasons"] = new[] { "Legacy" }, ["message"] = message };
            }

            return new Dictionary<string, object>
            {
                ["@id"] = $"{Base}/v3/registration/{lower}/{v}.json",
                ["catalogEntry"] = entry,
                ["packageContent"] = $"{Base}/v3/flat/{lower}/{v}/{lower}.{v}.nupkg",
            };
        }).ToList();
        return new Dictionary<string, object>
        {
            ["@id"] = $"{Base}/v3/registration/{lower}/index.json",
            ["count"] = 1,
            ["items"] = new[]
            {
                new Dictionary<string, object>
                {
                    ["@id"] = $"{Base}/v3/registration/{lower}/index.json#page/{versions.First()}/{versions.Last()}",
                    ["count"] = leaves.Count,
                    ["lower"] = versions.First(),
                    ["upper"] = versions.Last(),
                    ["items"] = leaves,
                },
            },
        };
    }

    public async ValueTask DisposeAsync()
    {
        await _stop.CancelAsync();
        _listener.Stop();
        _listener.Close();
        try
        {
            await _loop.WaitAsync(TimeSpan.FromSeconds(5));
        }
        catch (TimeoutException)
        {
        }

        _stop.Dispose();
    }
}
