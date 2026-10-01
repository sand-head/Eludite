using System.Text.RegularExpressions;

namespace Niello.Web;

/// <summary>A server control that gets a designer field, with its start-tag attributes.</summary>
public sealed record DesignerControl(string? TagPrefix, string TagName, string Id, IReadOnlyDictionary<string, string> Attributes);

/// <summary>
/// Finds the server controls that the ASP.NET page parser turns into designer fields. Unlike
/// <see cref="ServerControlScanner"/> it tracks element nesting, so it can skip controls inside
/// multi-instance templates (<c>ItemTemplate</c> and friends) and <c>asp:Content</c> wrappers, which get no field.
/// </summary>
/// <remarks>
/// Spike approximation (brief 0003): a template is any element whose name ends in <c>Template</c>, except
/// <c>ContentTemplate</c> (UpdatePanel, single-instance). The real parser asks the control's
/// <c>TemplateInstanceAttribute</c>; that needs type information and is follow-up work.
/// </remarks>
public static partial class DesignerControlScanner
{
    [GeneratedRegex("""<(?<close>/)?(?:(?<prefix>[A-Za-z_][\w\-]*):)?(?<name>[A-Za-z_][\w\-.]*)(?<attributes>(?:\s+[^\s=/>]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*(?<self>/)?>""")]
    private static partial Regex Tag();

    [GeneratedRegex("<%(?!--).*?%>", RegexOptions.Singleline)]
    private static partial Regex CodeBlock();

    private static readonly HashSet<string> VoidElements = new(StringComparer.OrdinalIgnoreCase)
    {
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr",
    };

    /// <summary>Returns, in document order, the controls that get a designer field.</summary>
    public static IReadOnlyList<DesignerControl> Scan(string markup)
    {
        ArgumentNullException.ThrowIfNull(markup);

        // Blank out code blocks and comments but keep offsets; data-binding inside attribute values is left alone
        // because the tag regex consumes quoted values whole.
        var text = CodeBlock().Replace(AspxMarkup.StripServerComments(markup), m => new string(' ', m.Length));
        var result = new List<DesignerControl>();
        var stack = new List<(string Key, bool Template)>();
        var templateDepth = 0;

        foreach (Match match in Tag().Matches(text))
        {
            var prefix = match.Groups["prefix"].Success ? match.Groups["prefix"].Value : null;
            var name = match.Groups["name"].Value;
            var key = (prefix is null ? string.Empty : prefix + ":") + name;

            if (match.Groups["close"].Success)
            {
                var index = stack.FindLastIndex(e => e.Key.Equals(key, StringComparison.OrdinalIgnoreCase));
                if (index >= 0)
                {
                    for (var i = stack.Count - 1; i >= index; i--)
                    {
                        if (stack[i].Template)
                        {
                            templateDepth--;
                        }
                    }

                    stack.RemoveRange(index, stack.Count - index);
                }

                continue;
            }

            var isTemplate = name.EndsWith("Template", StringComparison.OrdinalIgnoreCase)
                && !name.Equals("ContentTemplate", StringComparison.OrdinalIgnoreCase);
            var attributes = AspxMarkup.ParseAttributes(match.Groups["attributes"].Value);
            var isServer = attributes.TryGetValue("runat", out var runat) && runat.Equals("server", StringComparison.OrdinalIgnoreCase);

            if (isServer
                && templateDepth == 0
                && attributes.TryGetValue("ID", out var id)
                && !string.IsNullOrWhiteSpace(id)
                && !(prefix is not null && name.Equals("Content", StringComparison.OrdinalIgnoreCase)))
            {
                result.Add(new DesignerControl(prefix, name, id, attributes));
            }

            var selfClosing = match.Groups["self"].Success || (prefix is null && VoidElements.Contains(name));
            if (!selfClosing)
            {
                stack.Add((key, isTemplate));
                if (isTemplate)
                {
                    templateDepth++;
                }
            }
        }

        return result;
    }
}
