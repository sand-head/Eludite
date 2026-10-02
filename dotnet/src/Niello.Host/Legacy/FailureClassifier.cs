namespace Niello.Host.Legacy;

/// <summary>Buckets evaluation failures and diagnostics into the classes the brief 0003 report uses.</summary>
public static class FailureClassifier
{
    public const string MissingTargets = "missing targets";
    public const string Com = "COM";
    public const string Packages = "packages";
    public const string WebTargets = "web targets";
    public const string Other = "other";

    /// <summary>Classifies one diagnostic by code and message.</summary>
    public static string Classify(string? code, string message)
    {
        ArgumentNullException.ThrowIfNull(message);
        var m = message.Replace('\\', '/');
        if (code is "MSB3283" or "MSB3284" or "MSB3290" or "MSB3303" or "MSB4803" || m.Contains("COMReference", StringComparison.OrdinalIgnoreCase)
            || m.Contains("ResolveComReference", StringComparison.OrdinalIgnoreCase) || m.Contains("tlbimp", StringComparison.OrdinalIgnoreCase)
            || m.Contains("AxImp", StringComparison.OrdinalIgnoreCase))
        {
            return Com;
        }

        if (m.Contains("WebApplication", StringComparison.OrdinalIgnoreCase) || m.Contains("/WebApplications/", StringComparison.OrdinalIgnoreCase)
            || m.Contains("Microsoft.Web.Publishing", StringComparison.OrdinalIgnoreCase) || m.Contains("WebPublishing", StringComparison.OrdinalIgnoreCase)
            || m.Contains("WebSite", StringComparison.OrdinalIgnoreCase))
        {
            return WebTargets;
        }

        if ((code?.StartsWith("NU", StringComparison.Ordinal) ?? false) || m.Contains("/packages/", StringComparison.OrdinalIgnoreCase)
            || m.Contains("packages.config", StringComparison.OrdinalIgnoreCase) || m.Contains("project.assets.json", StringComparison.OrdinalIgnoreCase)
            || m.Contains("NuGet", StringComparison.OrdinalIgnoreCase))
        {
            return Packages;
        }

        if (code is "MSB4019" or "MSB4057" or "MSB4226" or "MSB4278" || m.Contains(".targets", StringComparison.OrdinalIgnoreCase)
            || m.Contains("does not contain 'Compile' target", StringComparison.OrdinalIgnoreCase))
        {
            return MissingTargets;
        }

        return Other;
    }
}
