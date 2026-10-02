using System.Text.RegularExpressions;

namespace Niello.Web;

/// <summary>Reads <c>&lt;%@ Register %&gt;</c> directives from WebForms markup.</summary>
public static partial class RegisterDirectiveParser
{
    [GeneratedRegex(@"<%@\s*Register\b(?<attributes>.*?)%>", RegexOptions.Singleline | RegexOptions.IgnoreCase)]
    private static partial Regex Register();

    /// <summary>Returns every <c>Register</c> directive with a <c>TagPrefix</c>, in document order.</summary>
    public static IReadOnlyList<ControlRegistration> Parse(string markup)
    {
        ArgumentNullException.ThrowIfNull(markup);

        var result = new List<ControlRegistration>();
        foreach (Match match in Register().Matches(AspxMarkup.StripServerComments(markup)))
        {
            var a = AspxMarkup.ParseAttributes(match.Groups["attributes"].Value);
            if (!a.TryGetValue("TagPrefix", out var prefix) || prefix.Length == 0)
            {
                continue;
            }

            result.Add(new ControlRegistration(
                prefix,
                a.GetValueOrDefault("Namespace"),
                a.GetValueOrDefault("Assembly"),
                a.GetValueOrDefault("TagName"),
                a.GetValueOrDefault("Src")));
        }

        return result;
    }
}
