using System.Xml;
using System.Xml.Linq;

namespace Niello.Web;

/// <summary>Reads <c>system.web/pages/controls/add</c> registrations from a <c>web.config</c>.</summary>
public static class WebConfigControls
{
    /// <summary>
    /// Returns the tag-prefix registrations in <paramref name="webConfigXml"/>, including those under
    /// <c>&lt;location&gt;</c>. Malformed XML yields an empty list (the caller reports it as a diagnostic).
    /// </summary>
    public static IReadOnlyList<ControlRegistration> Read(string webConfigXml)
    {
        ArgumentNullException.ThrowIfNull(webConfigXml);

        XDocument doc;
        try
        {
            using var reader = XmlReader.Create(new StringReader(webConfigXml), new XmlReaderSettings { DtdProcessing = DtdProcessing.Prohibit });
            doc = XDocument.Load(reader);
        }
        catch (XmlException)
        {
            return [];
        }

        var result = new List<ControlRegistration>();
        foreach (var controls in doc.Descendants().Where(e => e.Name.LocalName == "controls" && e.Parent?.Name.LocalName == "pages" && e.Parent.Parent?.Name.LocalName == "system.web"))
        {
            foreach (var add in controls.Elements().Where(e => e.Name.LocalName == "add"))
            {
                var prefix = (string?)add.Attribute("tagPrefix");
                if (string.IsNullOrEmpty(prefix))
                {
                    continue;
                }

                result.Add(new ControlRegistration(
                    prefix,
                    (string?)add.Attribute("namespace"),
                    (string?)add.Attribute("assembly"),
                    (string?)add.Attribute("tagName"),
                    (string?)add.Attribute("src")));
            }
        }

        return result;
    }
}
