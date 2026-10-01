using System.Text.RegularExpressions;

namespace Niello.Web;

/// <summary>Finds and parses the main <c>Page</c>, <c>Control</c> or <c>Master</c> directive of WebForms markup.</summary>
public static partial class AspxDirectiveParser
{
    [GeneratedRegex(@"<%@\s*(?<name>\w+)(?<attributes>.*?)%>", RegexOptions.Singleline)]
    private static partial Regex Directive();

    /// <summary>
    /// Returns the first <c>Page</c>, <c>Control</c> or <c>Master</c> directive, skipping other directives
    /// (<c>Register</c>, <c>Import</c>, ...) and server comments, or null if there is none.
    /// </summary>
    public static AspxDirective? Parse(string markup)
    {
        ArgumentNullException.ThrowIfNull(markup);

        foreach (Match match in Directive().Matches(AspxMarkup.StripServerComments(markup)))
        {
            if (!Enum.TryParse<DirectiveKind>(match.Groups["name"].Value, ignoreCase: true, out var kind)
                || !Enum.IsDefined(kind))
            {
                continue;
            }

            var attributes = AspxMarkup.ParseAttributes(match.Groups["attributes"].Value);
            return new AspxDirective(
                kind,
                Take(attributes, "Language"),
                Take(attributes, "CodeBehind"),
                Take(attributes, "CodeFile"),
                Take(attributes, "Inherits"),
                bool.TryParse(Take(attributes, "AutoEventWireup"), out var wireup) ? wireup : null,
                attributes);
        }

        return null;
    }

    private static string? Take(Dictionary<string, string> attributes, string name) =>
        attributes.Remove(name, out var value) ? value : null;
}
