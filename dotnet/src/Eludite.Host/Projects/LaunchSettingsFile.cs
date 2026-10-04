using System.Text.Encodings.Web;
using System.Text.Json;
using System.Text.Json.Nodes;
using Eludite.Host.Rpc;

namespace Eludite.Host.Projects;

/// <summary>
/// <c>Properties/launchSettings.json</c> read and written with System.Text.Json's nodes (brief 0049): the profiles keep
/// their order, members Eludite does not edit are kept as they are, an environment variables table keeps its order (new
/// variables are appended), and the file keeps its indentation, line endings, trailing newline and byte order mark.
/// JSON comments are skipped on read and not written back.
/// </summary>
public static class LaunchSettingsFile
{
    /// <summary>The members the Debug page edits, in the order Visual Studio writes a new profile's.</summary>
    public static readonly string[] Known =
    [
        "commandName", "executablePath", "commandLineArgs", "workingDirectory", "launchBrowser", "launchUrl",
        "environmentVariables", "dotnetRunMessages", "applicationUrl", "hotReloadEnabled",
    ];

    private static readonly JsonDocumentOptions ReadOptions = new() { CommentHandling = JsonCommentHandling.Skip, AllowTrailingCommas = true };

    public static string PathFor(string projectPath) =>
        Path.Combine(Path.GetDirectoryName(Path.GetFullPath(projectPath))!, "Properties", "launchSettings.json");

    /// <summary>The profiles in file order; none when the file does not exist.</summary>
    public static (bool Exists, IReadOnlyList<LaunchProfileInfo> Profiles) Read(string projectPath)
    {
        var file = PathFor(projectPath);
        if (!File.Exists(file))
        {
            return (false, []);
        }

        var (root, _) = Load(file);
        return (true, Profiles(root));
    }

    /// <summary>Applies one action and writes the file; returns the profiles after it.</summary>
    public static IReadOnlyList<LaunchProfileInfo> Apply(string projectPath, string action, string profile, string? newName, JsonElement? values, bool web)
    {
        ArgumentException.ThrowIfNullOrEmpty(profile);
        var file = PathFor(projectPath);
        JsonObject root;
        TextFileFormat? format = null;
        if (File.Exists(file))
        {
            (root, format) = Load(file);
        }
        else if (action == "create")
        {
            root = new JsonObject { ["profiles"] = new JsonObject() };
        }
        else
        {
            throw HostErrors.BadParams($"{file} does not exist");
        }

        if (root["profiles"] is not JsonObject profiles)
        {
            profiles = new JsonObject();
            root["profiles"] = profiles;
        }

        switch (action)
        {
            case "create":
                if (profiles.ContainsKey(profile))
                {
                    throw HostErrors.BadParams($"a launch profile named `{profile}` exists");
                }

                var created = new JsonObject { ["commandName"] = "Project" };
                if (web)
                {
                    created["launchBrowser"] = true;
                    created["environmentVariables"] = new JsonObject { ["ASPNETCORE_ENVIRONMENT"] = "Development" };
                }

                profiles[profile] = created;
                SetValues(created, values);
                break;
            case "set":
                SetValues(Profile(profiles, profile, file), values);
                break;
            case "rename":
                if (string.IsNullOrEmpty(newName))
                {
                    throw HostErrors.BadParams("rename needs newName");
                }

                Profile(profiles, profile, file);
                if (!string.Equals(newName, profile, StringComparison.Ordinal) && profiles.ContainsKey(newName))
                {
                    throw HostErrors.BadParams($"a launch profile named `{newName}` exists");
                }

                var entries = profiles.ToList();
                profiles.Clear();
                foreach (var (k, v) in entries)
                {
                    profiles[k == profile ? newName : k] = v;
                }

                break;
            case "delete":
                Profile(profiles, profile, file);
                profiles.Remove(profile);
                break;
            default:
                throw HostErrors.BadParams($"unknown action `{action}` (set, create, rename, delete)");
        }

        Write(file, root, format);
        return Profiles(root);
    }

    private static JsonObject Profile(JsonObject profiles, string name, string file) =>
        profiles[name] as JsonObject ?? throw HostErrors.BadParams($"no launch profile `{name}` in {file}");

    private static (JsonObject Root, TextFileFormat Format) Load(string file)
    {
        var bytes = File.ReadAllBytes(file);
        var format = TextFileFormat.Detect(bytes, out var text);
        try
        {
            return (JsonNode.Parse(text, documentOptions: ReadOptions) as JsonObject ?? throw HostErrors.BadParams($"{file}: not a JSON object"), format);
        }
        catch (JsonException ex)
        {
            throw HostErrors.BadParams($"{file}: {ex.Message}");
        }
    }

    private static void SetValues(JsonObject profile, JsonElement? values)
    {
        if (values is not { ValueKind: JsonValueKind.Object } v)
        {
            return;
        }

        foreach (var member in v.EnumerateObject())
        {
            if (!Known.Contains(member.Name, StringComparer.Ordinal))
            {
                throw HostErrors.BadParams($"`{member.Name}` is not a launch profile member Eludite edits");
            }

            if (member.Value.ValueKind == JsonValueKind.Null)
            {
                profile.Remove(member.Name);
                continue;
            }

            if (member.Name == "environmentVariables")
            {
                SetEnvironment(profile, member.Value);
                continue;
            }

            profile[member.Name] = member.Value.ValueKind switch
            {
                JsonValueKind.True => JsonValue.Create(true),
                JsonValueKind.False => JsonValue.Create(false),
                JsonValueKind.String => JsonValue.Create(member.Value.GetString()),
                _ => throw HostErrors.BadParams($"`{member.Name}` takes a string or a boolean"),
            };
        }
    }

    /// <summary>The whole table, in order: variables already in the file keep their place, new ones are appended.</summary>
    private static void SetEnvironment(JsonObject profile, JsonElement table)
    {
        if (table.ValueKind != JsonValueKind.Array)
        {
            throw HostErrors.BadParams("environmentVariables takes an array of { name, value }");
        }

        var wanted = new List<(string Name, string Value)>();
        foreach (var row in table.EnumerateArray())
        {
            var name = row.TryGetProperty("name", out var n) ? n.GetString() : null;
            var value = row.TryGetProperty("value", out var val) ? val.GetString() : null;
            if (string.IsNullOrEmpty(name) || value is null)
            {
                throw HostErrors.BadParams("an environment variable needs a name and a value");
            }

            wanted.Add((name, value));
        }

        if (wanted.Count == 0)
        {
            profile.Remove("environmentVariables");
            return;
        }

        if (profile["environmentVariables"] is not JsonObject env)
        {
            env = new JsonObject();
            profile["environmentVariables"] = env;
        }

        foreach (var key in env.Select(e => e.Key).ToList())
        {
            if (!wanted.Any(w => w.Name == key))
            {
                env.Remove(key);
            }
        }

        foreach (var (name, value) in wanted)
        {
            if (env[name] is JsonValue existing && existing.TryGetValue<string>(out var s) && s == value)
            {
                continue;
            }

            env[name] = value;
        }
    }

    private static List<LaunchProfileInfo> Profiles(JsonObject root)
    {
        var list = new List<LaunchProfileInfo>();
        if (root["profiles"] is not JsonObject profiles)
        {
            return list;
        }

        foreach (var (name, node) in profiles)
        {
            if (node is not JsonObject p)
            {
                continue;
            }

            var command = Str(p, "commandName") ?? string.Empty;
            var env = new List<EnvironmentVariable>();
            if (p["environmentVariables"] is JsonObject e)
            {
                foreach (var (k, v) in e)
                {
                    env.Add(new EnvironmentVariable(k, v is JsonValue jv && jv.TryGetValue<string>(out var s) ? s : v?.ToJsonString() ?? string.Empty));
                }
            }

            var unknown = p.Select(m => m.Key).Where(k => !Known.Contains(k, StringComparer.Ordinal)).ToList();
            var iis = command is "IISExpress" or "IIS";
            list.Add(new LaunchProfileInfo(name, command, env)
            {
                CommandLineArgs = Str(p, "commandLineArgs"),
                WorkingDirectory = Str(p, "workingDirectory"),
                LaunchBrowser = Bool(p, "launchBrowser"),
                LaunchUrl = Str(p, "launchUrl"),
                ApplicationUrl = Str(p, "applicationUrl"),
                DotnetRunMessages = Bool(p, "dotnetRunMessages"),
                HotReloadEnabled = Bool(p, "hotReloadEnabled"),
                ExecutablePath = Str(p, "executablePath"),
                ReadOnly = iis && !OperatingSystem.IsWindows(),
                Unknown = unknown.Count > 0 ? unknown : null,
            });
        }

        return list;
    }

    private static string? Str(JsonObject o, string name) =>
        o[name] is JsonValue v && v.TryGetValue<string>(out var s) ? s : null;

    private static bool? Bool(JsonObject o, string name) =>
        o[name] is JsonValue v && v.TryGetValue<bool>(out var b) ? b : null;

    private static void Write(string file, JsonObject root, TextFileFormat? format)
    {
        var (indent, indentSize) = format is null ? (' ', 2) : Indentation(File.ReadAllText(file));
        var options = new JsonWriterOptions
        {
            Indented = true,
            IndentCharacter = indent,
            IndentSize = indentSize,
            NewLine = "\n",
            Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
        };
        using var stream = new MemoryStream();
        using (var writer = new Utf8JsonWriter(stream, options))
        {
            root.WriteTo(writer);
        }

        var json = System.Text.Encoding.UTF8.GetString(stream.ToArray());
        byte[] bytes;
        if (format is null)
        {
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            var text = json + "\n";
            bytes = System.Text.Encoding.UTF8.GetBytes(Environment.NewLine == "\r\n" ? text.Replace("\n", "\r\n", StringComparison.Ordinal) : text);
        }
        else
        {
            bytes = format.Encode(json);
        }

        TextFileFormat.WriteAtomically(file, bytes);
    }

    /// <summary>The file's indentation: the first indented line's leading tabs or spaces (2 spaces by default).</summary>
    internal static (char Character, int Size) Indentation(string text)
    {
        foreach (var line in text.Split('\n'))
        {
            var l = line.TrimEnd('\r');
            if (l.Length == 0 || !char.IsWhiteSpace(l[0]))
            {
                continue;
            }

            var c = l[0];
            var n = 0;
            while (n < l.Length && l[n] == c)
            {
                n++;
            }

            if (n < l.Length && (c == ' ' || c == '\t'))
            {
                return (c, Math.Clamp(n, 1, 8));
            }
        }

        return (' ', 2);
    }
}
