using Eludite.Host.Sdk;

namespace Eludite.Host.Tests;

public sealed class DotnetCliSdkDiscovererTests
{
    [Fact]
    public void Parse_ReadsVersionAndPathFromEachLine()
    {
        const string output = "8.0.130 [/usr/share/dotnet/sdk]\n10.0.302 [/usr/share/dotnet/sdk]\n";

        var sdks = DotnetCliSdkDiscoverer.Parse(output);

        Assert.Equal(
            [new DotnetSdk("8.0.130", "/usr/share/dotnet/sdk"), new DotnetSdk("10.0.302", "/usr/share/dotnet/sdk")],
            sdks);
    }

    [Fact]
    public void Parse_HandlesWindowsPathsWithSpacesAndCrLf()
    {
        const string output = "9.0.100-rc.1.24452.12 [C:\\Program Files\\dotnet\\sdk]\r\n";

        var sdk = Assert.Single(DotnetCliSdkDiscoverer.Parse(output));

        Assert.Equal("9.0.100-rc.1.24452.12", sdk.Version);
        Assert.Equal("C:\\Program Files\\dotnet\\sdk", sdk.Path);
    }

    [Theory]
    [InlineData("")]
    [InlineData("\n\n")]
    [InlineData("No .NET SDKs were found.")]
    [InlineData("[/usr/share/dotnet/sdk]")]
    [InlineData("10.0.302 /usr/share/dotnet/sdk")]
    public void Parse_SkipsBlankAndMalformedLines(string output)
    {
        Assert.Empty(DotnetCliSdkDiscoverer.Parse(output));
    }
}
