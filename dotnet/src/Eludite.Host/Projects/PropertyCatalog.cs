namespace Eludite.Host.Projects;

/// <summary>A choice of an enum property: the MSBuild value and what the list shows.</summary>
public sealed record CatalogValue(string Value, string Label);

/// <summary>
/// One property the project property pages show (brief 0049; host-rpc.md, "Project properties"): its page and section,
/// Visual Studio's label, how the page edits it, whether it is edited per configuration, and the MSBuild property it maps
/// to. <see cref="Target"/> names the target an SDK-style project's build event is written in.
/// </summary>
public sealed record CatalogEntry(string Name, string Page, string Section, string Label, string Description, string Type, bool PerConfiguration)
{
    public IReadOnlyList<CatalogValue>? Values { get; init; }

    public string? TrueValue { get; init; }

    public string? FalseValue { get; init; }

    /// <summary>SDK-style projects: the build event's target (<c>PreBuild</c>, <c>PostBuild</c>).</summary>
    public string? Target { get; init; }

    /// <summary>Read-only off Windows (the Win32 resources).</summary>
    public bool WindowsOnly { get; init; }
}

/// <summary>One property page, in Visual Studio's order. <see cref="State"/> is <c>ready</c>, <c>launchProfiles</c> or <c>notYet</c>.</summary>
public sealed record CatalogPage(string Id, string Title, string State, string? Note = null);

/// <summary>The catalog: Visual Studio's ~30 properties people touch, grouped as its pages.</summary>
public static class PropertyCatalog
{
    public const string String = "string";
    public const string Bool = "bool";
    public const string Enum = "enum";
    public const string List = "list";
    public const string PathType = "path";
    public const string Multiline = "multiline";

    public static IReadOnlyList<CatalogPage> Pages { get; } =
    [
        new("application", "Application", "ready"),
        new("build", "Build", "ready"),
        new("package", "Package", "ready"),
        new("debug", "Debug", "launchProfiles"),
        new("codeAnalysis", "Code Analysis", "ready"),
        new("resources", "Resources", "notYet", "Not yet: the .resx editor comes with a later brief; edit Resources.resx as text meanwhile."),
        new("settings", "Settings", "notYet", "Not yet: the .settings editor comes with a later brief; edit Settings.settings as text meanwhile."),
        new("signing", "Signing", "notYet", "Not yet: ClickOnce manifest signing is not supported; strong naming is on the Build page."),
    ];

    private static CatalogValue[] V(params string[] pairs)
    {
        var values = new CatalogValue[pairs.Length / 2];
        for (var i = 0; i < values.Length; i++)
        {
            values[i] = new CatalogValue(pairs[2 * i], pairs[(2 * i) + 1]);
        }

        return values;
    }

    public static IReadOnlyList<CatalogEntry> Entries { get; } =
    [
        // Application
        new("AssemblyName", "application", "General", "Assembly name", "The name of the output file that will hold the assembly manifest.", String, false),
        new("RootNamespace", "application", "General", "Default namespace", "The namespace added to new files.", String, false),
        new("TargetFramework", "application", "General", "Target framework", "The version of .NET the application targets.", String, false),
        new("TargetFrameworks", "application", "General", "Target frameworks", "The frameworks a multi-targeted project builds for, ';'-separated.", List, false),
        new("OutputType", "application", "General", "Output type", "The type of application to build.", Enum, false)
        {
            Values = V("Exe", "Console Application", "WinExe", "Windows Application", "Library", "Class Library"),
        },
        new("StartupObject", "application", "General", "Startup object", "The type that contains the entry point called when the application starts.", String, false),
        new("Nullable", "application", "General", "Nullable", "The project-wide C# nullable context.", Enum, false)
        {
            Values = V("disable", "Disable", "enable", "Enable", "warnings", "Warnings", "annotations", "Annotations"),
        },
        new("ImplicitUsings", "application", "General", "Implicit global usings", "Enable a set of global usings for the project type.", Bool, false)
        {
            TrueValue = "enable",
            FalseValue = "disable",
        },
        new("LangVersion", "application", "General", "Language version", "The version of the C# language the compiler accepts.", Enum, false)
        {
            Values = V("default", "Default", "latest", "Latest", "latestMajor", "Latest major", "preview", "Preview", "14.0", "C# 14.0", "13.0", "C# 13.0", "12.0", "C# 12.0", "11.0", "C# 11.0", "10.0", "C# 10.0", "9.0", "C# 9.0", "8.0", "C# 8.0", "7.3", "C# 7.3"),
        },
        new("ApplicationIcon", "application", "Win32 resources", "Icon", "The .ico file embedded as the application's icon.", PathType, false) { WindowsOnly = true },
        new("ApplicationManifest", "application", "Win32 resources", "Manifest", "The application manifest embedded in the output.", PathType, false) { WindowsOnly = true },
        new("Win32Resource", "application", "Win32 resources", "Resource file", "A Win32 .res file embedded in the output.", PathType, false) { WindowsOnly = true },

        // Build
        new("DefineConstants", "build", "General", "Conditional compilation symbols", "Symbols on which to perform conditional compilation, ';'-separated.", List, true),
        new("Optimize", "build", "General", "Optimize code", "Enable compiler optimizations for smaller, faster and more efficient output.", Bool, true),
        new("WarningLevel", "build", "Errors and warnings", "Warning level", "The level of warnings the compiler reports.", Enum, true)
        {
            Values = V("0", "0", "1", "1", "2", "2", "3", "3", "4", "4", "5", "5", "6", "6", "7", "7", "8", "8", "9", "9", "9999", "9999"),
        },
        new("TreatWarningsAsErrors", "build", "Errors and warnings", "Treat warnings as errors", "Report all warnings as errors.", Bool, true),
        new("WarningsAsErrors", "build", "Errors and warnings", "Treat specific warnings as errors", "Warnings reported as errors, ';'-separated.", List, true),
        new("NoWarn", "build", "Errors and warnings", "Suppress specific warnings", "Warnings the compiler does not report, ';'-separated.", List, true),
        new("GenerateDocumentationFile", "build", "Output", "Documentation file", "Generate a file containing API documentation.", Bool, true),
        new("DocumentationFile", "build", "Output", "XML documentation file path", "Where the XML documentation file is written.", PathType, true),
        new("OutputPath", "build", "Output", "Output path", "Where the build writes its output.", PathType, true),
        new("BaseOutputPath", "build", "Output", "Base output path", "The folder under which per-configuration output folders are created.", PathType, false),
        new("PreBuildEvent", "build", "Events", "Pre-build event", "Commands that run before the build starts.", Multiline, false) { Target = "PreBuild" },
        new("PostBuildEvent", "build", "Events", "Post-build event", "Commands that run after the build finishes.", Multiline, false) { Target = "PostBuild" },
        new("SignAssembly", "build", "Strong naming", "Sign the assembly", "Sign the output assembly to give it a strong name.", Bool, false),
        new("AssemblyOriginatorKeyFile", "build", "Strong naming", "Strong name key file", "The key file used to sign the assembly.", PathType, false),
        new("Deterministic", "build", "Advanced", "Deterministic", "Produce identical compilation output for identical inputs.", Bool, false),
        new("DebugType", "build", "Advanced", "Debug symbols", "The kind of debug information the compiler emits.", Enum, true)
        {
            Values = V("portable", "PDB file, portable across platforms", "embedded", "Embedded in DLL/EXE, portable across platforms", "full", "PDB file, current platform", "pdbonly", "PDB file, current platform (pdbonly)", "none", "No symbols are emitted"),
        },

        // Package
        new("GeneratePackageOnBuild", "package", "General", "Generate NuGet package on build", "Produce a package file during build operations.", Bool, false),
        new("PackageId", "package", "General", "Package ID", "The case-insensitive package identifier, unique across the package source.", String, false),
        new("Version", "package", "General", "Version", "The package version (major.minor.patch[-suffix]).", String, false),
        new("Authors", "package", "General", "Authors", "The package's authors, ';'-separated.", String, false),
        new("Description", "package", "General", "Description", "A description of the package for UI display.", String, false),
        new("PackageProjectUrl", "package", "General", "Project URL", "The URL of the package's home page.", String, false),
        new("RepositoryUrl", "package", "General", "Repository URL", "The URL of the repository the package is built from.", String, false),
        new("PackageTags", "package", "General", "Tags", "Tags and keywords that describe the package, ';'-separated.", String, false),
        new("PackageLicenseExpression", "package", "License", "License expression", "An SPDX license expression (MIT, Apache-2.0 OR MIT).", String, false),
        new("PackageReadmeFile", "package", "License", "README", "The readme file in the package (path inside the package).", PathType, false),

        // Code Analysis
        new("EnforceCodeStyleInBuild", "codeAnalysis", "General", "Enforce code style on build", "Report code style violations as build diagnostics.", Bool, false),
        new("AnalysisLevel", "codeAnalysis", "General", "Analysis level", "The set of analyzers that run on the project.", Enum, false)
        {
            Values = V("latest", "Latest", "latest-minimum", "Latest minimum", "latest-recommended", "Latest recommended", "latest-all", "Latest all", "preview", "Preview", "none", "None", "10.0", "10.0", "9.0", "9.0", "8.0", "8.0", "7.0", "7.0", "6.0", "6.0", "5.0", "5.0"),
        },
    ];

    private static readonly Dictionary<string, CatalogEntry> ByName =
        Entries.ToDictionary(e => e.Name, StringComparer.OrdinalIgnoreCase);

    /// <summary>The entry named <paramref name="name"/> (case-insensitive), or null.</summary>
    public static CatalogEntry? Find(string name) => ByName.GetValueOrDefault(name);
}
