using System.Text.RegularExpressions;

namespace Niello.Web;

/// <summary>Regexes shared by the ASPX scanners.</summary>
internal static partial class AspxMarkup
{
    /// <summary>A single attribute: <c>name="v"</c>, <c>name='v'</c> or <c>name=v</c>.</summary>
    [GeneratedRegex("""(?<name>[\w:.\-]+)\s*=\s*(?:"(?<value>[^"]*)"|'(?<value>[^']*)'|(?<value>[^\s"'>]+))""")]
    public static partial Regex Attribute();

    /// <summary>A server-side comment, <c>&lt;%-- ... --%&gt;</c>, whose contents ASP.NET ignores entirely.</summary>
    [GeneratedRegex("<%--.*?--%>", RegexOptions.Singleline)]
    private static partial Regex ServerComment();

    public static Dictionary<string, string> ParseAttributes(string text)
    {
        var attributes = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        foreach (Match match in Attribute().Matches(text))
        {
            // ASP.NET rejects duplicates; keep the first so behaviour is deterministic.
            attributes.TryAdd(match.Groups["name"].Value, match.Groups["value"].Value);
        }

        return attributes;
    }

    public static string StripServerComments(string markup) => ServerComment().Replace(markup, " ");
}
