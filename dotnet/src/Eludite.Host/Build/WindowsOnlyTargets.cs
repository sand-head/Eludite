using System.Globalization;
using System.Text.RegularExpressions;

namespace Eludite.Host.Build;

/// <summary>
/// Brief 0003's Windows-only targets (report section 4): off Windows, a build failure caused by one of them is
/// replaced by <b>one diagnostic per project</b> with the brief 0003 code and message, which the Output window also
/// shows, instead of MSBuild's raw errors and a task's stack trace.
/// </summary>
public static partial class WindowsOnlyTargets
{
    public const string ComReference = "ELUDITE0101";
    public const string WebPublishing = "ELUDITE0102";
    public const string WebApplicationTargets = "ELUDITE0103";
    public const string BuildEvent = "ELUDITE0108";
    public const string SGen = "ELUDITE0109";
    public const string AspNetCompiler = "ELUDITE0110";
    public const string NoLegacyMsBuild = "ELUDITE0111";

    /// <summary>The OS name the messages use.</summary>
    public static string ThisOs => OperatingSystem.IsMacOS() ? "macOS" : "Linux";

    [GeneratedRegex(@"exited with code (?<code>-?\d+)")]
    private static partial Regex ExitCode();

    [GeneratedRegex(@"(?:COM reference|type library|wrapper assembly for (?:the )?type library)\s+""(?<name>[^""]+)""", RegexOptions.IgnoreCase)]
    private static partial Regex ComName();

    /// <summary>
    /// The brief 0003 code a raw diagnostic belongs to, or null. <paramref name="target"/> is the MSBuild target that
    /// reported it, when known (from the binary log).
    /// </summary>
    public static string? Classify(BuildDiagnostic diagnostic, string? target)
    {
        ArgumentNullException.ThrowIfNull(diagnostic);
        if (OperatingSystem.IsWindows())
        {
            return null;
        }

        var code = diagnostic.Code;
        var text = diagnostic.Message;
        bool Has(string s) => text.Contains(s, StringComparison.OrdinalIgnoreCase);
        if (code is "MSB4019" or "MSB4022" or "MSB4062" or "MSB4036" && (Has("Microsoft.Web.Publishing") || Has("WebPublishingTasks") || Has("Microsoft.Web.Publishing.Tasks")))
        {
            return WebPublishing;
        }

        if (code is "MSB4019" && Has("Microsoft.WebApplication.targets"))
        {
            return WebApplicationTargets;
        }

        if (target is "ResolveComReference" || code is "MSB3283" or "MSB3290" or "MSB3303" || Has("AxImp.exe") || Has("TlbImp.exe") || Has("ResolveComReference"))
        {
            return ComReference;
        }

        if (target is "GenerateSerializationAssemblies" || Has("sgen.exe") || code is "MSB3091" && Has("sgen"))
        {
            return SGen;
        }

        if (target is "MvcBuildViews" or "AspNetPreCompile" || Has("aspnet_compiler"))
        {
            return AspNetCompiler;
        }

        if (code is "MSB3073" or "MSB3075")
        {
            if (target is "PreBuildEvent" or "PostBuildEvent")
            {
                return BuildEvent;
            }

            // Without the log: a Windows command that /bin/sh could not find exits with 127 (9009 is cmd.exe's own).
            var m = ExitCode().Match(text);
            if (target is null && m.Success && m.Groups["code"].Value is "127" or "9009")
            {
                return BuildEvent;
            }
        }

        return null;
    }

    /// <summary>The brief 0003 message for <paramref name="code"/> in project <paramref name="projectName"/>.</summary>
    public static string Message(string code, string projectName, IReadOnlyList<BuildDiagnostic> raw, string? target)
    {
        ArgumentNullException.ThrowIfNull(raw);
        var os = ThisOs;
        switch (code)
        {
            case ComReference:
                var name = raw.Select(d => ComName().Match(d.Message)).FirstOrDefault(m => m.Success)?.Groups["name"].Value;
                return name is null
                    ? $"COM references of {projectName} were skipped: resolving COM type libraries (ResolveComReference) runs only on Windows. Code that uses them shows errors here. Build on Windows, or reference a checked-in interop assembly."
                    : $"COM reference '{name}' was skipped: resolving COM type libraries (ResolveComReference) runs only on Windows. Code that uses it shows errors here. Build on Windows, or reference a checked-in interop assembly.";
            case WebPublishing:
                return $"{projectName} needs Visual Studio's web publishing targets (Microsoft.Web.Publishing.targets), which are not available on {os}. Build it on Windows with Build Tools' web workload, or remove the publishing import.";
            case WebApplicationTargets:
                return "Web application targets (Microsoft.WebApplication.targets) not found. Install Mono (Linux/macOS) or Build Tools (Windows) for full web project support.";
            case BuildEvent:
                var which = target switch
                {
                    "PreBuildEvent" => "The pre-build event",
                    "PostBuildEvent" => "The post-build event",
                    _ => "A build event",
                };
                return $"{which} of {projectName} is a Windows command script (cmd.exe) and cannot run on {os}.";
            case SGen:
                return "XML serialization assemblies (SGen) can only be generated on Windows; skipped.";
            case AspNetCompiler:
                return "ASP.NET precompilation (aspnet_compiler) runs only on Windows; skipped.";
            default:
                return string.Create(CultureInfo.InvariantCulture, $"{code}: Windows-only target in {projectName}.");
        }
    }

    /// <summary>brief 0003's severity for <paramref name="code"/>; an error when it replaces raw errors.</summary>
    public static string Severity(string code, IReadOnlyList<BuildDiagnostic> raw)
    {
        ArgumentNullException.ThrowIfNull(raw);
        if (raw.Any(d => d.Severity == "error"))
        {
            return "error";
        }

        return code is WebPublishing or BuildEvent ? "error" : "warning";
    }

    /// <summary>
    /// Replaces every classified raw diagnostic with one diagnostic per project and code. <paramref name="targetOf"/>
    /// gives the reporting target of a raw diagnostic, when known. Returns the new list (order kept: each replacement
    /// stands where its first raw diagnostic was) and the replacements.
    /// </summary>
    public static (IReadOnlyList<BuildDiagnostic> Diagnostics, IReadOnlyList<BuildDiagnostic> Replacements) Replace(
        IReadOnlyList<BuildDiagnostic> diagnostics, Func<BuildDiagnostic, string?> targetOf)
    {
        ArgumentNullException.ThrowIfNull(diagnostics);
        ArgumentNullException.ThrowIfNull(targetOf);
        var groups = new Dictionary<(string? Project, string Code), (List<BuildDiagnostic> Raw, string? Target)>();
        var classified = new List<(BuildDiagnostic Diagnostic, string? Code)>();
        foreach (var d in diagnostics)
        {
            var target = targetOf(d);
            var code = Classify(d, target);
            classified.Add((d, code));
            if (code is null)
            {
                continue;
            }

            var key = (d.Project, code);
            if (!groups.TryGetValue(key, out var g))
            {
                g = ([], target);
                groups[key] = g;
            }

            g.Raw.Add(d);
        }

        var result = new List<BuildDiagnostic>();
        var replacements = new List<BuildDiagnostic>();
        var emitted = new HashSet<(string?, string)>();
        foreach (var (d, code) in classified)
        {
            if (code is null)
            {
                result.Add(d);
                continue;
            }

            var key = (d.Project, code);
            if (!emitted.Add(key))
            {
                continue;
            }

            var (raw, target) = groups[key];
            var name = d.Project is null ? "the project" : Path.GetFileNameWithoutExtension(d.Project);
            var replacement = new BuildDiagnostic(Severity(code, raw), code, Message(code, name, raw, target))
            {
                File = d.Project,
                Project = d.Project,
            };
            result.Add(replacement);
            replacements.Add(replacement);
        }

        return (result, replacements);
    }
}
