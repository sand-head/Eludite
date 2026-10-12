using System.Xml.Linq;

namespace Eludite.TestBridge;

/// <summary>The code coverage collector a test project references (brief 0076; host-rpc.md, "Tests", Coverage).</summary>
public enum CoverageCollector
{
    /// <summary>None: the project runs without coverage.</summary>
    None,

    /// <summary>Microsoft.Testing.Extensions.CodeCoverage: the test application's <c>--coverage</c>.</summary>
    Mtp,

    /// <summary>coverlet.collector: VSTest's <c>XPlat Code Coverage</c> data collector.</summary>
    Coverlet,

    /// <summary>Microsoft.CodeCoverage (through Microsoft.NET.Test.Sdk): VSTest's <c>Code Coverage</c> data collector.</summary>
    Microsoft,
}

/// <summary>
/// How a run collects Cobertura coverage (brief 0076): the switches an MTP test application gets, the run settings a
/// VSTest run gets, and where the reports are found afterwards. Every report of a container is written under the
/// container's <see cref="TestContainer.CoverageDirectory"/>.
/// </summary>
public static class CoverageCollection
{
    /// <summary>The report an MTP application writes into its coverage directory.</summary>
    public const string MtpReportName = "coverage.cobertura.xml";

    /// <summary>The wire name of a collector (<c>mtp</c>, <c>coverlet</c>, <c>microsoft</c>, <c>none</c>).</summary>
    public static string Name(CoverageCollector collector) => collector switch
    {
        CoverageCollector.Mtp => "mtp",
        CoverageCollector.Coverlet => "coverlet",
        CoverageCollector.Microsoft => "microsoft",
        _ => "none",
    };

    /// <summary>True when a run of <paramref name="container"/> collects coverage.</summary>
    public static bool Collects(TestContainer container)
    {
        ArgumentNullException.ThrowIfNull(container);
        return container.CoverageDirectory is { Length: > 0 }
            && container.Coverage switch
            {
                CoverageCollector.Mtp => container.Protocol == TestRunnerProtocol.MicrosoftTestingPlatform,
                CoverageCollector.Coverlet or CoverageCollector.Microsoft => container.Protocol == TestRunnerProtocol.VsTest,
                _ => false,
            };
    }

    /// <summary>
    /// The MTP switches of a coverage run: <c>--coverage --coverage-output-format cobertura --coverage-output &lt;file&gt;</c>,
    /// the file an absolute path in the container's coverage directory (Microsoft.Testing.Extensions.CodeCoverage writes
    /// an absolute <c>--coverage-output</c> where it says, a bare name under <c>--results-directory</c>).
    /// </summary>
    public static IReadOnlyList<string> MtpArguments(TestContainer container)
    {
        ArgumentNullException.ThrowIfNull(container);
        if (!Collects(container) || container.Coverage != CoverageCollector.Mtp)
        {
            return [];
        }

        return ["--coverage", "--coverage-output-format", "cobertura", "--coverage-output", Path.Combine(container.CoverageDirectory!, MtpReportName)];
    }

    /// <summary>
    /// <paramref name="runSettings"/> with the container's VSTest data collector writing Cobertura into its coverage
    /// directory: <c>RunConfiguration/ResultsDirectory</c>, the collector's folder added to
    /// <c>RunConfiguration/TestAdaptersPaths</c> (vstest.console does not look in the packages by itself), and
    /// <c>DataCollectionRunSettings/DataCollectors/DataCollector</c> (<c>XPlat Code Coverage</c> with <c>Format</c>
    /// <c>cobertura</c> for coverlet, <c>Code Coverage</c> with <c>Format</c> <c>Cobertura</c> for Microsoft's), kept with
    /// its configuration when the settings already name it. Settings that are not XML are returned as they are.
    /// </summary>
    public static string VsTestRunSettings(TestContainer container, string runSettings)
    {
        ArgumentNullException.ThrowIfNull(container);
        ArgumentNullException.ThrowIfNull(runSettings);
        if (!Collects(container) || container.Protocol != TestRunnerProtocol.VsTest)
        {
            return runSettings;
        }

        XDocument document;
        try
        {
            document = XDocument.Parse(runSettings);
        }
        catch (System.Xml.XmlException)
        {
            return runSettings;
        }

        if (document.Root is not { } root)
        {
            return runSettings;
        }

        var run = Child(root, "RunConfiguration");
        Child(run, "ResultsDirectory").Value = container.CoverageDirectory!;
        if (container.CoverageAdapterPath is { Length: > 0 } adapters)
        {
            var paths = Child(run, "TestAdaptersPaths");
            var list = paths.Value.Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).ToList();
            if (!list.Contains(adapters, StringComparer.OrdinalIgnoreCase))
            {
                list.Add(adapters);
            }

            paths.Value = string.Join(';', list);
        }

        var (friendlyName, format) = container.Coverage == CoverageCollector.Coverlet
            ? ("XPlat Code Coverage", "cobertura")
            : ("Code Coverage", "Cobertura");
        var collectors = Child(Child(root, "DataCollectionRunSettings"), "DataCollectors");
        var collector = collectors.Elements("DataCollector")
            .FirstOrDefault(c => string.Equals((string?)c.Attribute("friendlyName"), friendlyName, StringComparison.OrdinalIgnoreCase));
        if (collector is null)
        {
            collector = new XElement("DataCollector", new XAttribute("friendlyName", friendlyName));
            collectors.Add(collector);
        }

        Child(Child(collector, "Configuration"), "Format").Value = format;
        return document.ToString(SaveOptions.DisableFormatting);
    }

    /// <summary>The Cobertura reports under <paramref name="directory"/> (<c>*.cobertura.xml</c>, any depth), oldest first.</summary>
    public static IReadOnlyList<string> Reports(string? directory)
    {
        if (directory is null || !Directory.Exists(directory))
        {
            return [];
        }

        try
        {
            return Directory.EnumerateFiles(directory, "*.cobertura.xml", SearchOption.AllDirectories)
                .OrderBy(File.GetLastWriteTimeUtc)
                .ThenBy(f => f, StringComparer.Ordinal)
                .ToList();
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            return [];
        }
    }

    private static XElement Child(XElement parent, string name)
    {
        if (parent.Element(name) is { } existing)
        {
            return existing;
        }

        var created = new XElement(name);
        parent.Add(created);
        return created;
    }
}
