using System.Text.RegularExpressions;

namespace Eludite.Web;

/// <summary>Finds server controls in WebForms markup without building a full HTML tree.</summary>
public static partial class ServerControlScanner
{
    // A start tag (or self-closing tag). Quoted attribute values may contain '>' and data-binding
    // expressions such as Text='<%# Eval("Name") %>'.
    [GeneratedRegex("""<(?:(?<prefix>[A-Za-z_][\w\-]*):)?(?<name>[A-Za-z_][\w\-.]*)(?<attributes>(?:\s+[^\s=/>]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*/?>""")]
    private static partial Regex StartTag();

    /// <summary>Returns, in document order, every element marked <c>runat="server"</c> that has an <c>ID</c>.</summary>
    public static IReadOnlyList<ServerControl> Scan(string markup)
    {
        ArgumentNullException.ThrowIfNull(markup);

        var controls = new List<ServerControl>();
        foreach (Match match in StartTag().Matches(AspxMarkup.StripServerComments(markup)))
        {
            var attributes = AspxMarkup.ParseAttributes(match.Groups["attributes"].Value);
            if (!attributes.TryGetValue("runat", out var runat)
                || !runat.Equals("server", StringComparison.OrdinalIgnoreCase)
                || !attributes.TryGetValue("ID", out var id)
                || string.IsNullOrWhiteSpace(id))
            {
                continue;
            }

            var prefix = match.Groups["prefix"].Success ? match.Groups["prefix"].Value : null;
            controls.Add(new ServerControl(prefix, match.Groups["name"].Value, id));
        }

        return controls;
    }
}
