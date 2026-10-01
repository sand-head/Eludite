using System.Xml.Linq;

namespace Eludite.TestBridge;

/// <summary>Decides which test protocol a project will speak by reading its project file.</summary>
public static class TestProjectInspector
{
    private static readonly string[] MtpProperties =
    [
        "EnableMSTestRunner",
        "EnableNUnitRunner",
        "UseMicrosoftTestingPlatformRunner",
        "TestingPlatformDotnetTestSupport",
    ];

    private static readonly string[] MtpPackagePrefixes = ["Microsoft.Testing.Platform", "xunit.v3"];

    private const string VsTestPackage = "Microsoft.NET.Test.Sdk";

    /// <summary>
    /// Returns <see cref="TestRunnerProtocol.MicrosoftTestingPlatform"/> when an MTP runner property is
    /// <c>true</c>, the project uses <c>MSTest.Sdk</c>, or it references a Microsoft.Testing.Platform or
    /// xunit.v3 package; <see cref="TestRunnerProtocol.VsTest"/> when it only references
    /// <c>Microsoft.NET.Test.Sdk</c> (or sets <c>UseVSTest</c>); null when it is not a test project.
    /// Only this file is inspected; Directory.Build.props and imports are not followed.
    /// </summary>
    /// <exception cref="System.Xml.XmlException">The text is not well-formed XML.</exception>
    public static TestRunnerProtocol? Detect(string csprojXml)
    {
        ArgumentNullException.ThrowIfNull(csprojXml);

        var project = XDocument.Parse(csprojXml).Root;
        if (project is null)
        {
            return null;
        }

        var properties = project.Elements()
            .Where(e => e.Name.LocalName == "PropertyGroup")
            .SelectMany(g => g.Elements())
            .ToList();
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
            || packages.Any(p => MtpPackagePrefixes.Any(prefix => p.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))))
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
