namespace Eludite.TestBridge.Tests;

public sealed class TestProjectInspectorTests
{
    [Fact]
    public void Detect_XunitV3_IsMicrosoftTestingPlatform()
    {
        const string csproj = """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup><TargetFramework>net10.0</TargetFramework><OutputType>Exe</OutputType></PropertyGroup>
              <ItemGroup><PackageReference Include="xunit.v3" Version="3.2.2" /></ItemGroup>
            </Project>
            """;

        Assert.Equal(TestRunnerProtocol.MicrosoftTestingPlatform, TestProjectInspector.Detect(csproj));
    }

    [Theory]
    [InlineData("EnableMSTestRunner")]
    [InlineData("UseMicrosoftTestingPlatformRunner")]
    [InlineData("TestingPlatformDotnetTestSupport")]
    public void Detect_MtpRunnerProperty_IsMicrosoftTestingPlatformEvenWithTestSdk(string property)
    {
        var csproj = $"""
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFramework>net10.0</TargetFramework>
                <{property}> True </{property}>
              </PropertyGroup>
              <ItemGroup>
                <PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.12.0" />
                <PackageReference Include="MSTest" Version="3.6.4" />
              </ItemGroup>
            </Project>
            """;

        Assert.Equal(TestRunnerProtocol.MicrosoftTestingPlatform, TestProjectInspector.Detect(csproj));
    }

    [Fact]
    public void Detect_MicrosoftTestingPlatformPackage_IsMicrosoftTestingPlatform()
    {
        const string csproj = """
            <Project Sdk="Microsoft.NET.Sdk">
              <ItemGroup>
                <PackageReference Include="TUnit.Core" />
                <PackageReference Include="Microsoft.Testing.Platform.MSBuild" />
              </ItemGroup>
            </Project>
            """;

        Assert.Equal(TestRunnerProtocol.MicrosoftTestingPlatform, TestProjectInspector.Detect(csproj));
    }

    [Fact]
    public void Detect_MSTestSdk_IsMicrosoftTestingPlatform()
    {
        Assert.Equal(
            TestRunnerProtocol.MicrosoftTestingPlatform,
            TestProjectInspector.Detect("""<Project Sdk="MSTest.Sdk/3.6.4"><PropertyGroup><TargetFramework>net9.0</TargetFramework></PropertyGroup></Project>"""));
    }

    [Fact]
    public void Detect_Xunit2WithTestSdk_IsVsTest()
    {
        const string csproj = """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFramework>net8.0</TargetFramework>
                <IsPackable>false</IsPackable>
                <EnableMSTestRunner>false</EnableMSTestRunner>
              </PropertyGroup>
              <ItemGroup>
                <PackageReference Include="Microsoft.NET.Test.Sdk" Version="17.8.0" />
                <PackageReference Include="xunit" Version="2.6.2" />
                <PackageReference Include="xunit.runner.visualstudio" Version="2.5.4" />
              </ItemGroup>
            </Project>
            """;

        Assert.Equal(TestRunnerProtocol.VsTest, TestProjectInspector.Detect(csproj));
    }

    [Fact]
    public void Detect_LegacyNamespacedProjectWithTestSdk_IsVsTest()
    {
        const string csproj = """
            <?xml version="1.0" encoding="utf-8"?>
            <Project ToolsVersion="15.0" xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
              <ItemGroup>
                <PackageReference Include="Microsoft.NET.Test.Sdk"><Version>16.11.0</Version></PackageReference>
                <PackageReference Include="NUnit"><Version>3.13.3</Version></PackageReference>
              </ItemGroup>
            </Project>
            """;

        Assert.Equal(TestRunnerProtocol.VsTest, TestProjectInspector.Detect(csproj));
    }

    [Fact]
    public void Detect_UseVSTestOverridesMSTestSdk()
    {
        Assert.Equal(
            TestRunnerProtocol.VsTest,
            TestProjectInspector.Detect("""<Project Sdk="MSTest.Sdk/3.6.4"><PropertyGroup><UseVSTest>true</UseVSTest></PropertyGroup></Project>"""));
    }

    [Fact]
    public void Detect_NonTestProject_ReturnsNull()
    {
        const string csproj = """
            <Project Sdk="Microsoft.NET.Sdk.Web">
              <ItemGroup><PackageReference Include="Serilog.AspNetCore" Version="8.0.0" /></ItemGroup>
            </Project>
            """;

        Assert.Null(TestProjectInspector.Detect(csproj));
    }
}
