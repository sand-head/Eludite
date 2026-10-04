using System.Text.Json;
using Eludite.Host.Projects;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0049: <c>launchSettings.json</c> read and written with System.Text.Json's nodes on <c>corpus/projects</c>: the
/// profiles' order and unknown members survive, an edit changes only its member, the environment table keeps its order,
/// New, Rename and Delete, IIS Express read-only off Windows, and the file created when missing.
/// </summary>
public sealed class LaunchProfilesTests : IDisposable
{
    private readonly ProjectCorpus _corpus = new();

    public void Dispose() => _corpus.Dispose();

    private static JsonElement Values(object values) => JsonSerializer.SerializeToElement(values);

    private IReadOnlyList<LaunchProfileInfo> Apply(string project, string action, string profile, object? values = null, string? newName = null, bool web = false) =>
        LaunchSettingsFile.Apply(_corpus.Path(project), action, profile, newName, values is null ? null : Values(values), web);

    [Fact]
    public void Read_KeepsTheFilesOrder_UnknownMembers_AndMarksIisExpress()
    {
        var (exists, profiles) = LaunchSettingsFile.Read(_corpus.Path("Web/Web.csproj"));

        Assert.True(exists);
        Assert.Equal(["http", "https", "IIS Express"], profiles.Select(p => p.Name));
        var https = profiles[1];
        Assert.Equal(("Project", true, true, "swagger", "https://localhost:7080;http://localhost:5080"), (https.CommandName, https.LaunchBrowser, https.DotnetRunMessages, https.LaunchUrl, https.ApplicationUrl));
        Assert.Equal([new EnvironmentVariable("ASPNETCORE_ENVIRONMENT", "Development")], https.EnvironmentVariables);
        Assert.Equal(!OperatingSystem.IsWindows(), profiles[2].ReadOnly);

        var console = LaunchSettingsFile.Read(_corpus.Path("Console/Console.csproj")).Profiles;
        Assert.Equal(["ZETA", "ALPHA", "MIDDLE"], console[0].EnvironmentVariables.Select(e => e.Name));
        Assert.Equal(["x-corpus-note"], console[0].Unknown);
        Assert.Equal(("Executable", "/usr/bin/env", "$(ProjectDir)"), (console[1].CommandName, console[1].ExecutablePath, console[1].WorkingDirectory));

        Assert.False(LaunchSettingsFile.Read(_corpus.Path("Empty/Empty.csproj")).Exists);
    }

    [Fact]
    public void Set_ChangesOnlyItsMember_AndKeepsTheRestOfTheFileByteForByte()
    {
        var before = _corpus.Text("Web/Properties/launchSettings.json");
        Apply("Web/Web.csproj", "set", "https", new { commandLineArgs = "--urls http://+:9000" });

        var expected = before.Replace(
            "      \"applicationUrl\": \"https://localhost:7080;http://localhost:5080\",\n      \"environmentVariables\": {\n        \"ASPNETCORE_ENVIRONMENT\": \"Development\"\n      }\n    },",
            "      \"applicationUrl\": \"https://localhost:7080;http://localhost:5080\",\n      \"environmentVariables\": {\n        \"ASPNETCORE_ENVIRONMENT\": \"Development\"\n      },\n      \"commandLineArgs\": \"--urls http://+:9000\"\n    },",
            StringComparison.Ordinal);
        Assert.Equal(expected, _corpus.Text("Web/Properties/launchSettings.json"));

        // Setting a member to its value again, and removing what was added, gives the original file back.
        Apply("Web/Web.csproj", "set", "https", new { commandLineArgs = (string?)null, launchUrl = "swagger" });
        Assert.Equal(before, _corpus.Text("Web/Properties/launchSettings.json"));
    }

    [Fact]
    public void TheEnvironmentTable_KeepsItsOrder_AppendsNewVariables_AndDropsRemovedOnes()
    {
        var profiles = Apply("Console/Console.csproj", "set", "Console", new
        {
            environmentVariables = new[]
            {
                new { name = "ALPHA", value = "changed" },
                new { name = "NEW", value = "added" },
                new { name = "ZETA", value = "last" },
            },
        });

        Assert.Equal([new("ZETA", "last"), new("ALPHA", "changed"), new EnvironmentVariable("NEW", "added")], profiles[0].EnvironmentVariables);
        var text = _corpus.Text("Console/Console.csproj".Replace("Console.csproj", "Properties/launchSettings.json", StringComparison.Ordinal));
        Assert.Contains("\"ZETA\": \"last\",\n        \"ALPHA\": \"changed\",\n        \"NEW\": \"added\"\n", text, StringComparison.Ordinal);
        Assert.Contains("\"x-corpus-note\": \"an unknown member Eludite keeps\"", text, StringComparison.Ordinal);

        profiles = Apply("Console/Console.csproj", "set", "Console", new { environmentVariables = Array.Empty<object>() });
        Assert.Empty(profiles[0].EnvironmentVariables);
        Assert.DoesNotContain("environmentVariables", _corpus.Text("Console/Properties/launchSettings.json").Split("\"Tool\"")[0], StringComparison.Ordinal);
    }

    [Fact]
    public void New_Rename_AndDelete_KeepTheOrder()
    {
        var profiles = Apply("Web/Web.csproj", "create", "Profile 1", web: true);
        Assert.Equal(["http", "https", "IIS Express", "Profile 1"], profiles.Select(p => p.Name));
        Assert.Equal(("Project", true), (profiles[3].CommandName, profiles[3].LaunchBrowser));
        Assert.Equal([new EnvironmentVariable("ASPNETCORE_ENVIRONMENT", "Development")], profiles[3].EnvironmentVariables);
        Assert.Throws<LocalRpcException>(() => Apply("Web/Web.csproj", "create", "http"));

        profiles = Apply("Web/Web.csproj", "rename", "http", newName: "plain http");
        Assert.Equal(["plain http", "https", "IIS Express", "Profile 1"], profiles.Select(p => p.Name));
        Assert.Throws<LocalRpcException>(() => Apply("Web/Web.csproj", "rename", "https", newName: "Profile 1"));

        profiles = Apply("Web/Web.csproj", "delete", "IIS Express");
        Assert.Equal(["plain http", "https", "Profile 1"], profiles.Select(p => p.Name));
        Assert.Contains("\"iisSettings\": {", _corpus.Text("Web/Properties/launchSettings.json"), StringComparison.Ordinal);
        Assert.Throws<LocalRpcException>(() => Apply("Web/Web.csproj", "set", "missing", new { launchUrl = "x" }));
        Assert.Throws<LocalRpcException>(() => Apply("Web/Web.csproj", "set", "https", new { unknownMember = "x" }));
    }

    [Fact]
    public void Create_MakesTheFileWhenMissing()
    {
        var profiles = Apply("Empty/Empty.csproj", "create", "Empty", new { commandLineArgs = "a b" });

        Assert.Single(profiles);
        Assert.Equal("a b", profiles[0].CommandLineArgs);
        var text = _corpus.Text("Empty/Properties/launchSettings.json").Replace("\r\n", "\n", StringComparison.Ordinal);
        Assert.Equal("{\n  \"profiles\": {\n    \"Empty\": {\n      \"commandName\": \"Project\",\n      \"commandLineArgs\": \"a b\"\n    }\n  }\n}\n", text);
        Assert.Throws<LocalRpcException>(() => LaunchSettingsFile.Apply(_corpus.Path("Multi/Multi.csproj"), "set", "x", null, null, false));
    }

    [Fact]
    public void Indentation_IsTheFilesOwn()
    {
        Assert.Equal((' ', 2), LaunchSettingsFile.Indentation("{\n  \"a\": 1\n}"));
        Assert.Equal((' ', 4), LaunchSettingsFile.Indentation("{\r\n    \"a\": 1\r\n}"));
        Assert.Equal(('\t', 1), LaunchSettingsFile.Indentation("{\n\t\"a\": 1\n}"));
        var file = _corpus.Path("Console/Properties/launchSettings.json");
        File.WriteAllText(file, File.ReadAllText(file).Replace("\n", "\r\n", StringComparison.Ordinal).Replace("  ", "    ", StringComparison.Ordinal));
        var before = File.ReadAllText(file);
        Apply("Console/Console.csproj", "set", "Tool", new { commandLineArgs = "x" });
        Assert.Equal(before.Replace("\"workingDirectory\": \"$(ProjectDir)\"\r\n", "\"workingDirectory\": \"$(ProjectDir)\",\r\n            \"commandLineArgs\": \"x\"\r\n", StringComparison.Ordinal), File.ReadAllText(file));
    }
}
