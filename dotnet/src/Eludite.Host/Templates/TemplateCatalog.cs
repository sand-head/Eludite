using System.Globalization;
using System.IO.Compression;
using System.Text.Json;

namespace Eludite.Host.Templates;

/// <summary>A template as read from its package, with what <c>eludite/templates/create</c> needs beyond the listing.</summary>
/// <param name="Info">The listing's record.</param>
/// <param name="SourceName">The template's <c>sourceName</c>, replaced by <c>--name</c> in its outputs.</param>
/// <param name="PrimaryOutputs">The <c>primaryOutputs</c> paths as the template writes them (before <see cref="SourceName"/> is replaced).</param>
/// <param name="OptionNames">Each parameter symbol's name to the option <c>dotnet new</c> takes for it (<c>dotnetcli.host.json</c>'s <c>longName</c>, else the name).</param>
public sealed record TemplateEntry(TemplateInfo Info, string? SourceName, IReadOnlyList<string> PrimaryOutputs, IReadOnlyDictionary<string, string> OptionNames)
{
    /// <summary>The <c>--option</c> for a parameter given by its option name or its symbol name (any case); as given when unknown.</summary>
    public string Option(string parameter)
    {
        ArgumentNullException.ThrowIfNull(parameter);
        if (parameter.StartsWith('-'))
        {
            return parameter;
        }

        var name = OptionNames.Values.FirstOrDefault(v => string.Equals(v, parameter, StringComparison.Ordinal))
            ?? OptionNames.FirstOrDefault(p => string.Equals(p.Key, parameter, StringComparison.OrdinalIgnoreCase)).Value
            ?? OptionNames.Values.FirstOrDefault(v => string.Equals(v, parameter, StringComparison.OrdinalIgnoreCase))
            ?? parameter;
        return "--" + name;
    }
}

/// <summary>One reading of the package set.</summary>
public sealed record TemplateSet(string? TemplatesFolder, IReadOnlyList<TemplateEntry> Entries, IReadOnlyList<string> Skipped);

/// <summary>
/// The installed <c>dotnet new</c> templates (brief 0072; host-rpc.md, "Templates and items"), read from the template
/// packages themselves: the SDK's (<c>&lt;dotnet root&gt;/templates/&lt;version&gt;/*.nupkg</c>, the highest version
/// folder whose major is the newest SDK's, else the highest) and the user's (<c>~/.templateengine/packages/*.nupkg</c>),
/// each a zip whose <c>**/.template.config/template.json</c> files are the templates. The reading is cached per package
/// set (each package's path, size and time); a package that cannot be read is skipped with why.
/// </summary>
public sealed class TemplateCatalog
{
    private static readonly JsonDocumentOptions JsonOptions = new() { CommentHandling = JsonCommentHandling.Skip, AllowTrailingCommas = true };

    private readonly string? _userPackages;
    private readonly TextWriter _log;
    private readonly Lock _lock = new();
    private string? _cacheKey;
    private TemplateSet? _cache;

    /// <param name="dotnetRoot">The .NET install folder; when null, <see cref="LocateDotnetRoot"/>.</param>
    /// <param name="userPackages">The user's package folder; when null, <c>~/.templateengine/packages</c>.</param>
    public TemplateCatalog(string? dotnetRoot = null, string? userPackages = null, TextWriter? log = null)
    {
        DotnetRoot = dotnetRoot ?? LocateDotnetRoot();
        _userPackages = userPackages ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".templateengine", "packages");
        _log = log ?? TextWriter.Null;
    }

    /// <summary>The .NET install folder (holding <c>sdk/</c> and <c>templates/</c>), or null when none was found.</summary>
    public string? DotnetRoot { get; }

    /// <summary>Times the package set was read (the cache's tests).</summary>
    public int Reads { get; private set; }

    /// <summary>The <c>dotnet</c> executable of <see cref="DotnetRoot"/>, else <c>dotnet</c> (found on PATH).</summary>
    public string DotnetExecutable =>
        DotnetRoot is { } root && File.Exists(Path.Combine(root, ExecutableName)) ? Path.Combine(root, ExecutableName) : "dotnet";

    private static string ExecutableName => OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet";

    /// <summary>
    /// The .NET install folder: <c>DOTNET_HOST_PATH</c>'s folder, <c>DOTNET_ROOT</c>, the <c>dotnet</c> on PATH
    /// resolved through links, then the usual install locations; the first holding an <c>sdk</c> folder.
    /// </summary>
    public static string? LocateDotnetRoot()
    {
        var candidates = new List<string>();
        if (Environment.GetEnvironmentVariable("DOTNET_HOST_PATH") is { Length: > 0 } host)
        {
            candidates.Add(Path.GetDirectoryName(Resolve(host)) ?? host);
        }

        if (Environment.GetEnvironmentVariable("DOTNET_ROOT") is { Length: > 0 } root)
        {
            candidates.Add(root);
        }

        foreach (var dir in (Environment.GetEnvironmentVariable("PATH") ?? string.Empty).Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries))
        {
            var exe = Path.Combine(dir, ExecutableName);
            if (File.Exists(exe) && Path.GetDirectoryName(Resolve(exe)) is { } resolved)
            {
                candidates.Add(resolved);
            }
        }

        if (OperatingSystem.IsWindows())
        {
            candidates.Add(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "dotnet"));
        }
        else
        {
            candidates.AddRange(["/usr/share/dotnet", "/usr/lib/dotnet", "/usr/local/share/dotnet", "/usr/lib64/dotnet", "/opt/dotnet"]);
        }

        return candidates.Select(c => Path.GetFullPath(c)).FirstOrDefault(c => Directory.Exists(Path.Combine(c, "sdk")));
    }

    private static string Resolve(string path)
    {
        try
        {
            return new FileInfo(path).ResolveLinkTarget(returnFinalTarget: true)?.FullName ?? path;
        }
        catch (IOException)
        {
            return path;
        }
    }

    /// <summary>The SDK's template folder: the highest version folder under <c>templates/</c> whose major is the newest SDK's, else the highest.</summary>
    public string? TemplatesFolder()
    {
        if (DotnetRoot is null || !Directory.Exists(Path.Combine(DotnetRoot, "templates")))
        {
            return null;
        }

        var folders = Directory.EnumerateDirectories(Path.Combine(DotnetRoot, "templates"))
            .Select(d => (Path: d, Version: ParseVersion(Path.GetFileName(d))))
            .Where(d => d.Version is not null)
            .OrderByDescending(d => d.Version)
            .ToList();
        if (folders.Count == 0)
        {
            return null;
        }

        var sdkDir = Path.Combine(DotnetRoot, "sdk");
        var sdkMajor = Directory.Exists(sdkDir)
            ? Directory.EnumerateDirectories(sdkDir).Select(d => ParseVersion(Path.GetFileName(d))).OfType<Version>().OrderDescending().FirstOrDefault()?.Major
            : null;
        return folders.Where(f => f.Version!.Major == sdkMajor).Select(f => f.Path).FirstOrDefault() ?? folders[0].Path;
    }

    private static Version? ParseVersion(string name)
    {
        var dash = name.IndexOf('-', StringComparison.Ordinal);
        return Version.TryParse(dash >= 0 ? name[..dash] : name, out var version) ? version : null;
    }

    /// <summary>The templates of the current package set, from the cache when the set is unchanged and <paramref name="refresh"/> is false.</summary>
    public TemplateSet Read(bool refresh = false, CancellationToken cancellationToken = default)
    {
        var folder = TemplatesFolder();
        var packages = new List<(string Path, string Source)>();
        if (folder is not null)
        {
            packages.AddRange(Directory.EnumerateFiles(folder, "*.nupkg").Order(StringComparer.Ordinal).Select(p => (p, "sdk")));
        }

        if (_userPackages is not null && Directory.Exists(_userPackages))
        {
            packages.AddRange(Directory.EnumerateFiles(_userPackages, "*.nupkg").Order(StringComparer.Ordinal).Select(p => (p, "user")));
        }

        var key = string.Join('\n', packages.Select(p =>
        {
            var info = new FileInfo(p.Path);
            return string.Create(CultureInfo.InvariantCulture, $"{p.Source}|{p.Path}|{info.Length}|{info.LastWriteTimeUtc.Ticks}");
        }));
        lock (_lock)
        {
            if (!refresh && _cache is not null && _cacheKey == key)
            {
                return _cache;
            }

            var entries = new List<TemplateEntry>();
            var skipped = new List<string>();
            foreach (var (path, source) in packages)
            {
                cancellationToken.ThrowIfCancellationRequested();
                try
                {
                    entries.AddRange(ReadPackage(path, source, skipped));
                }
                catch (Exception ex) when (ex is IOException or InvalidDataException or UnauthorizedAccessException)
                {
                    skipped.Add($"{path}: {ex.Message}");
                    _log.WriteLine($"[templates] skipped {path}: {ex.Message}");
                }
            }

            Reads++;
            _cache = new TemplateSet(folder, entries, skipped);
            _cacheKey = key;
            return _cache;
        }
    }

    private List<TemplateEntry> ReadPackage(string path, string source, List<string> skipped)
    {
        var entries = new List<TemplateEntry>();
        using var zip = ZipFile.OpenRead(path);
        foreach (var file in zip.Entries.Where(e => e.FullName.Replace('\\', '/').EndsWith(".template.config/template.json", StringComparison.OrdinalIgnoreCase)).OrderBy(e => e.FullName, StringComparer.Ordinal))
        {
            try
            {
                using var stream = file.Open();
                using var document = JsonDocument.Parse(stream, JsonOptions);
                var hostName = file.FullName[..^"template.json".Length] + "dotnetcli.host.json";
                using var host = zip.GetEntry(hostName) is { } hostEntry ? ReadHost(hostEntry) : null;
                if (Parse(document.RootElement, source, host?.RootElement) is { } entry)
                {
                    entries.Add(entry);
                }
                else
                {
                    skipped.Add($"{path}!{file.FullName}: no identity, name or shortName");
                }
            }
            catch (Exception ex) when (ex is JsonException or IOException or InvalidDataException)
            {
                skipped.Add($"{path}!{file.FullName}: {ex.Message}");
                _log.WriteLine($"[templates] skipped {path}!{file.FullName}: {ex.Message}");
            }
        }

        return entries;
    }

    /// <summary>The template's <c>dotnetcli.host.json</c>, or null when it does not parse.</summary>
    private static JsonDocument? ReadHost(ZipArchiveEntry entry)
    {
        try
        {
            using var stream = entry.Open();
            return JsonDocument.Parse(stream, JsonOptions);
        }
        catch (JsonException)
        {
            return null;
        }
    }

    /// <summary>
    /// A template from its <c>template.json</c> and its <c>dotnetcli.host.json</c> (<paramref name="host"/>: a
    /// parameter's <c>longName</c> is its name, <c>isHidden</c> ones are left out); null when it has no identity, name
    /// or short name.
    /// </summary>
    public static TemplateEntry? Parse(JsonElement root, string source, JsonElement? host = null)
    {
        if (root.ValueKind != JsonValueKind.Object || String(root, "identity") is not { } identity || String(root, "name") is not { } name)
        {
            return null;
        }

        var shortNames = new List<string>();
        if (root.TryGetProperty("shortName", out var shortName))
        {
            if (shortName.ValueKind == JsonValueKind.String && shortName.GetString() is { Length: > 0 } one)
            {
                shortNames.Add(one);
            }
            else if (shortName.ValueKind == JsonValueKind.Array)
            {
                shortNames.AddRange(shortName.EnumerateArray().Where(s => s.ValueKind == JsonValueKind.String).Select(s => s.GetString()).OfType<string>().Where(s => s.Length > 0));
            }
        }

        if (shortNames.Count == 0)
        {
            return null;
        }

        string? language = null;
        string? type = null;
        if (root.TryGetProperty("tags", out var tags) && tags.ValueKind == JsonValueKind.Object)
        {
            language = String(tags, "language");
            type = String(tags, "type");
        }

        var symbolInfo = host is { ValueKind: JsonValueKind.Object } h && h.TryGetProperty("symbolInfo", out var info) && info.ValueKind == JsonValueKind.Object
            ? info
            : (JsonElement?)null;
        var parameters = new List<TemplateParameter>();
        var optionNames = new Dictionary<string, string>(StringComparer.Ordinal);
        if (root.TryGetProperty("symbols", out var symbols) && symbols.ValueKind == JsonValueKind.Object)
        {
            foreach (var symbol in symbols.EnumerateObject())
            {
                if (symbol.Value.ValueKind != JsonValueKind.Object || String(symbol.Value, "type") != "parameter")
                {
                    continue;
                }

                var hostSymbol = symbolInfo is { } si && si.TryGetProperty(symbol.Name, out var hs) && hs.ValueKind == JsonValueKind.Object ? hs : (JsonElement?)null;
                var option = hostSymbol is { } o && String(o, "longName") is { Length: > 0 } longName ? longName : symbol.Name;
                optionNames[symbol.Name] = option;
                if (hostSymbol is { } hidden && Scalar(hidden, "isHidden") is { } isHidden && string.Equals(isHidden, "true", StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }

                parameters.Add(Parameter(option, symbol.Value));
            }
        }

        var classifications = root.TryGetProperty("classifications", out var c) && c.ValueKind == JsonValueKind.Array
            ? c.EnumerateArray().Where(x => x.ValueKind == JsonValueKind.String).Select(x => x.GetString()!).ToList()
            : [];
        var primaryOutputs = root.TryGetProperty("primaryOutputs", out var outputs) && outputs.ValueKind == JsonValueKind.Array
            ? outputs.EnumerateArray().Where(o => o.ValueKind == JsonValueKind.Object).Select(o => String(o, "path")).OfType<string>().ToList()
            : [];
        var sourceName = String(root, "sourceName");
        var template = new TemplateInfo(identity, name, shortNames, type ?? "other", source, parameters)
        {
            GroupIdentity = String(root, "groupIdentity"),
            Language = language,
            Classifications = classifications.Count > 0 ? classifications : null,
            Author = String(root, "author"),
            Description = String(root, "description"),
            DefaultName = String(root, "defaultName") ?? sourceName,
        };
        return new TemplateEntry(template, sourceName, primaryOutputs, optionNames);
    }

    private static TemplateParameter Parameter(string name, JsonElement symbol)
    {
        var dataType = (String(symbol, "datatype") ?? String(symbol, "dataType") ?? "text").ToLowerInvariant();
        List<TemplateChoice>? choices = null;
        if (symbol.TryGetProperty("choices", out var list) && list.ValueKind == JsonValueKind.Array)
        {
            choices = list.EnumerateArray()
                .Where(x => x.ValueKind == JsonValueKind.Object && String(x, "choice") is not null)
                .Select(x => new TemplateChoice(String(x, "choice")!) { DisplayName = String(x, "displayName"), Description = String(x, "description") })
                .ToList();
        }

        bool? required = symbol.TryGetProperty("isRequired", out var r)
            ? r.ValueKind switch
            {
                JsonValueKind.True => true,
                JsonValueKind.False => false,
                JsonValueKind.String => string.Equals(r.GetString(), "true", StringComparison.OrdinalIgnoreCase),
                _ => null,
            }
            : null;
        return new TemplateParameter(name, dataType)
        {
            DisplayName = String(symbol, "displayName"),
            Description = String(symbol, "description"),
            DefaultValue = Scalar(symbol, "defaultValue"),
            Choices = choices,
            Required = required,
        };
    }

    private static string? String(JsonElement element, string name) =>
        element.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.String ? value.GetString() : null;

    /// <summary>A string, number or boolean property as text (a bool's <c>true</c>/<c>false</c>).</summary>
    private static string? Scalar(JsonElement element, string name) =>
        element.TryGetProperty(name, out var value)
            ? value.ValueKind switch
            {
                JsonValueKind.String => value.GetString(),
                JsonValueKind.True => "true",
                JsonValueKind.False => "false",
                JsonValueKind.Number => value.GetRawText(),
                _ => null,
            }
            : null;
}
