using System.Xml.Linq;

namespace Eludite.TestBridge;

/// <summary>What the Test Explorer reads from a test project's file.</summary>
/// <param name="TargetFrameworks">In the project file's order (<c>TargetFramework</c> or <c>TargetFrameworks</c>).</param>
/// <param name="AssemblyName">The output's name (<c>AssemblyName</c>, else the project file's name).</param>
/// <param name="IsExe">An executable (<c>OutputType</c> Exe or WinExe; MTP test projects are).</param>
public sealed record TestProjectInfo(TestRunnerProtocol Protocol, IReadOnlyList<string> TargetFrameworks, string AssemblyName, bool IsExe);

/// <summary>
/// Decides which test protocol a project will speak by reading its project file alone. Since brief 0076 the host decides
/// from MSBuild's evaluation (Eludite.Host's <c>TestProjectEvaluator</c>: Directory.Build.props, imports, the evaluated
/// <c>TargetPath</c>); this is its fallback when MSBuild is not available or a project does not evaluate.
/// </summary>
public static class TestProjectInspector
{
    /// <summary>The properties that, set to <c>true</c>, make a project an MTP test application.</summary>
    public static readonly IReadOnlyList<string> MtpProperties =
    [
        "EnableMSTestRunner",
        "EnableNUnitRunner",
        "UseMicrosoftTestingPlatformRunner",
        "TestingPlatformDotnetTestSupport",
    ];

    private static readonly string[] MtpPackagePrefixes = ["Microsoft.Testing.Platform", "xunit.v3"];

    // Brief 0076: TUnit runs on Microsoft.Testing.Platform only.
    private static readonly string[] MtpPackages = ["TUnit", "TUnit.Engine", "TUnit.Core"];

    /// <summary>The package of a VSTest test project.</summary>
    public const string VsTestPackage = "Microsoft.NET.Test.Sdk";

    /// <summary>
    /// Returns <see cref="TestRunnerProtocol.MicrosoftTestingPlatform"/> when an MTP runner property is
    /// <c>true</c>, the project uses <c>MSTest.Sdk</c>, or it references a Microsoft.Testing.Platform or
    /// xunit.v3 package; <see cref="TestRunnerProtocol.VsTest"/> when it only references
    /// <c>Microsoft.NET.Test.Sdk</c> (or sets <c>UseVSTest</c>); null when it is not a test project. A <c>TUnit</c>
    /// package is MTP (brief 0076).
    /// Only this file is inspected; Directory.Build.props and imports are not followed.
    /// </summary>
    /// <exception cref="System.Xml.XmlException">The text is not well-formed XML.</exception>
    public static TestRunnerProtocol? Detect(string csprojXml)
    {
        ArgumentNullException.ThrowIfNull(csprojXml);
        var project = XDocument.Parse(csprojXml).Root;
        return project is null ? null : Detect(project);
    }

    /// <summary>
    /// <see cref="Detect(string)"/> for the project file at <paramref name="projectPath"/>, with its target frameworks,
    /// assembly name and output type; null when it is not a test project or cannot be read.
    /// </summary>
    public static TestProjectInfo? Inspect(string projectPath)
    {
        ArgumentNullException.ThrowIfNull(projectPath);
        XElement? project;
        try
        {
            project = XDocument.Load(projectPath).Root;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or System.Xml.XmlException)
        {
            return null;
        }

        if (project is null || Detect(project) is not { } protocol)
        {
            return null;
        }

        var properties = Properties(project);
        string? Last(string name) => properties.LastOrDefault(p => p.Name.LocalName == name)?.Value.Trim();
        var frameworks = (Last("TargetFrameworks") ?? Last("TargetFramework") ?? string.Empty)
            .Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .Where(f => !f.Contains("$(", StringComparison.Ordinal))
            .ToList();
        var assembly = Last("AssemblyName") is { Length: > 0 } a && !a.Contains("$(", StringComparison.Ordinal)
            ? a
            : Path.GetFileNameWithoutExtension(projectPath);
        var outputType = Last("OutputType");
        var isExe = string.Equals(outputType, "Exe", StringComparison.OrdinalIgnoreCase)
            || string.Equals(outputType, "WinExe", StringComparison.OrdinalIgnoreCase)
            || (outputType is null && protocol == TestRunnerProtocol.MicrosoftTestingPlatform);
        return new TestProjectInfo(protocol, frameworks, assembly, isExe);
    }

    /// <summary>
    /// True for a package that makes a project a Microsoft.Testing.Platform test application: Microsoft.Testing.Platform*,
    /// xunit.v3*, and TUnit (<c>TUnit</c>, <c>TUnit.Engine</c>, <c>TUnit.Core</c>).
    /// </summary>
    public static bool IsMtpPackage(string packageId)
    {
        ArgumentNullException.ThrowIfNull(packageId);
        return MtpPackagePrefixes.Any(prefix => packageId.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
            || MtpPackages.Contains(packageId, StringComparer.OrdinalIgnoreCase);
    }

    /// <summary>True for a .NET Framework moniker (<c>net20</c> to <c>net481</c>): no dot after <c>net</c>.</summary>
    public static bool IsNetFramework(string targetFramework)
    {
        ArgumentNullException.ThrowIfNull(targetFramework);
        return targetFramework.StartsWith("net", StringComparison.OrdinalIgnoreCase)
            && targetFramework.Length > 3
            && targetFramework[3..].All(char.IsAsciiDigit);
    }

    private static List<XElement> Properties(XElement project) =>
        project.Elements()
            .Where(e => e.Name.LocalName == "PropertyGroup")
            .SelectMany(g => g.Elements())
            .ToList();

    private static TestRunnerProtocol? Detect(XElement project)
    {
        var properties = Properties(project);
        var packages = project.Descendants()
            .Where(e => e.Name.LocalName == "PackageReference")
            .Select(e => (string?)e.Attribute("Include"))
            .OfType<string>()
            .ToList();

        if (IsTrue(properties, "UseVSTest"))
        {
            return TestRunnerProtocol.VsTest;
        }

        var usesMSTestSdk = ((string?)project.Attribute("Sdk"))?.StartsWith("MSTest.Sdk", StringComparison.OrdinalIgnoreCase) == true;
        if (usesMSTestSdk
            || MtpProperties.Any(p => IsTrue(properties, p))
            || packages.Any(IsMtpPackage))
        {
            return TestRunnerProtocol.MicrosoftTestingPlatform;
        }

        return packages.Any(p => p.Equals(VsTestPackage, StringComparison.OrdinalIgnoreCase))
            ? TestRunnerProtocol.VsTest
            : null;
    }

    private static bool IsTrue(List<XElement> properties, string name) =>
        properties.Any(p => p.Name.LocalName == name && bool.TryParse(p.Value.Trim(), out var value) && value);
}
