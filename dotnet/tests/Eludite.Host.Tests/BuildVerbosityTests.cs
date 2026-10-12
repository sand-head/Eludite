using System.Text.Json;
using Eludite.Host.Build;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0075: <c>eludite/build/start</c>'s <c>verbosity</c> becomes MSBuild's <c>-v:</c> switch on both the
/// <c>dotnet build</c> path and the located-MSBuild path, and a build diagnostic's <c>helpLink</c> is on the wire only
/// when it has one.
/// </summary>
public sealed class BuildVerbosityTests
{
    private static readonly BuildToolchains NoLegacy = new(null, null, null);

    private static readonly BuildToolchains Mono = new(
        new MonoInstallation("/opt/mono", "/opt/mono/bin/mono", "/opt/mono/lib/mono/msbuild/Current/bin/MSBuild.dll", "test", new Dictionary<string, string>()),
        null,
        null);

    [Fact]
    public void Plan_DefaultsToMinimal()
    {
        var plan = BuildPlan.Create(BuildTargets.Build, "/s/A.slnx", "Debug", null, null, hasLegacy: false, NoLegacy);
        Assert.Contains("-v:m", plan.Arguments);
        Assert.Single(plan.Arguments, a => a.StartsWith("-v:", StringComparison.Ordinal));
    }

    [Theory]
    [InlineData(null, "-v:m")]
    [InlineData("quiet", "-v:q")]
    [InlineData("minimal", "-v:m")]
    [InlineData("normal", "-v:n")]
    [InlineData("detailed", "-v:d")]
    [InlineData("diagnostic", "-v:diag")]
    public void Plan_MapsVerbosityToTheSwitch_OnBothToolchains(string? verbosity, string expected)
    {
        Assert.True(BuildVerbosity.IsValid(verbosity));
        var dotnet = BuildPlan.Create(BuildTargets.Rebuild, "/s/A.slnx", "Debug", null, null, hasLegacy: false, NoLegacy, verbosity);
        Assert.Equal("dotnet", dotnet.Toolchain.Kind);
        Assert.Equal(expected, Assert.Single(dotnet.Arguments, a => a.StartsWith("-v:", StringComparison.Ordinal)));

        var mono = BuildPlan.Create(BuildTargets.Build, "/s/A.sln", "Debug", null, null, hasLegacy: true, Mono, verbosity);
        Assert.Equal("mono", mono.Toolchain.Kind);
        Assert.Equal(expected, Assert.Single(mono.Arguments, a => a.StartsWith("-v:", StringComparison.Ordinal)));
    }

    [Theory]
    [InlineData("verbose")]
    [InlineData("Minimal")]
    [InlineData("m")]
    [InlineData("")]
    public void Verbosity_RejectsAnythingButTheFiveNames(string verbosity) => Assert.False(BuildVerbosity.IsValid(verbosity));

    [Fact]
    public void Diagnostic_HelpLink_IsCamelCase_AndOmittedWhenNull()
    {
        var with = JsonSerializer.SerializeToElement(
            new BuildDiagnostic("error", "CS0103", "m") { HelpLink = "https://learn.microsoft.com/dotnet/csharp/misc/cs0103" },
            HostServer.SerializerOptions);
        Assert.Equal("https://learn.microsoft.com/dotnet/csharp/misc/cs0103", with.GetProperty("helpLink").GetString());

        var without = JsonSerializer.SerializeToElement(new BuildDiagnostic("error", "CS0103", "m"), HostServer.SerializerOptions);
        Assert.False(without.TryGetProperty("helpLink", out _));
    }

    [Fact]
    public void StartParams_ReadVerbosity()
    {
        var p = JsonSerializer.Deserialize<BuildStartParams>("""{"target":"build","verbosity":"detailed"}""", HostServer.SerializerOptions);
        Assert.Equal("detailed", p!.Verbosity);
    }
}
